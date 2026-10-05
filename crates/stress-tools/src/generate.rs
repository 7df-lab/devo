//! Synthetic DEVO_HOME corpus: many root sessions, one hot root with dense
//! subagent fan-out and parent↔child message traffic.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use clap::Parser;
use devo_core::{ROLLOUT_FORMAT_VERSION, SessionPersistenceExtras, session_line, turn_line};
use devo_protocol::SessionTitleState;
use devo_protocol::native::ids::{ItemId, SessionId, TurnId};
use devo_protocol::native::item::{
    Item, ItemEnvelope, ItemState, SpawnedWorkState, UserInput, UserMessageEntry,
};
use devo_protocol::native::model::{ModelBinding, PermissionProfile};
use devo_protocol::native::session::{
    Session, SessionActivity, SessionParent, SessionSettings, SessionStatus,
};
use devo_protocol::native::turn::{Turn, TurnKind, TurnStatus};
use devo_protocol::native::usage::{SessionUsage, UsageTotals};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Parser)]
pub struct GenerateArgs {
    /// Output DEVO_HOME root (creates `sessions/` + `session-artifacts/`).
    #[arg(long)]
    pub out: PathBuf,
    /// Number of user-visible root sessions.
    #[arg(long, default_value_t = 200)]
    pub roots: u32,
    /// Index of the root that receives the heavy subagent + traffic load.
    #[arg(long, default_value_t = 0)]
    pub hot_root_index: u32,
    /// Durable subagents nested under the hot root.
    #[arg(long, default_value_t = 100)]
    pub subagents: u32,
    /// Completed turns per root (and per subagent).
    #[arg(long, default_value_t = 20)]
    pub turns: u32,
    /// User+assistant message pairs per turn.
    #[arg(long, default_value_t = 4)]
    pub messages_per_turn: u32,
    /// Extra parent↔child communication rounds on the hot root (each round
    /// appends parent tool-style chatter + child reply turns).
    #[arg(long, default_value_t = 50)]
    pub comm_rounds: u32,
    /// Approximate payload size for each assistant message body (bytes).
    #[arg(long, default_value_t = 256)]
    pub message_bytes: usize,
    /// Absolute cwd written into SessionMeta (resume identity).
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    /// Delete and recreate `--out` if it already exists.
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

#[derive(Debug, Serialize)]
struct Manifest {
    schema_version: u32,
    roots: u32,
    hot_root_id: String,
    hot_root_index: u32,
    subagents: u32,
    turns: u32,
    messages_per_turn: u32,
    comm_rounds: u32,
    message_bytes: usize,
    child_session_ids: Vec<String>,
    sessions_dir: String,
    artifacts_dir: String,
}

pub fn run(args: GenerateArgs) -> Result<()> {
    if args.roots == 0 {
        bail!("--roots must be >= 1");
    }
    if args.hot_root_index >= args.roots {
        bail!(
            "--hot-root-index {} is out of range for --roots {}",
            args.hot_root_index,
            args.roots
        );
    }
    if args.out.exists() {
        if !args.force {
            bail!(
                "output {} already exists; pass --force to replace",
                args.out.display()
            );
        }
        fs::remove_dir_all(&args.out)
            .with_context(|| format!("remove existing {}", args.out.display()))?;
    }

    let cwd = args
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let sessions_dir = args.out.join("sessions");
    let artifacts_dir = args.out.join("session-artifacts");
    fs::create_dir_all(&sessions_dir)
        .with_context(|| format!("create {}", sessions_dir.display()))?;
    fs::create_dir_all(&artifacts_dir)
        .with_context(|| format!("create {}", artifacts_dir.display()))?;

    let payload = padded_text("stress-payload", args.message_bytes);
    let mut hot_root_id = None;
    let mut child_session_ids = Vec::new();

    for root_index in 0..args.roots {
        let root_id = SessionId::new();
        let is_hot = root_index == args.hot_root_index;
        let title = if is_hot {
            format!("stress-hot-root-{root_index}")
        } else {
            format!("stress-root-{root_index:04}")
        };
        let root_path = sessions_dir.join(format!("{root_id}.jsonl"));
        let mut writer = jsonl_writer(&root_path)?;
        let base = Utc::now() - Duration::seconds(i64::from(args.roots - root_index) * 60);
        write_session_transcript(
            &mut writer,
            SessionTranscriptSpec {
                session_id: root_id,
                parent: None,
                cwd: &cwd,
                title: &title,
                turns: args.turns,
                messages_per_turn: args.messages_per_turn,
                payload: &payload,
                base,
            },
        )?;

        if is_hot {
            hot_root_id = Some(root_id);
            let nest_root = artifacts_dir.join(root_id.to_string());
            fs::create_dir_all(&nest_root)
                .with_context(|| format!("create {}", nest_root.display()))?;

            let mut children = Vec::new();
            for child_index in 0..args.subagents {
                let child_id = SessionId::new();
                let sub_dir = create_unique_sub_dir(&nest_root)?;
                let child_path = sub_dir.join(format!("{child_id}.jsonl"));
                let mut child_writer = jsonl_writer(&child_path)?;
                let child_base = base + Duration::seconds(i64::from(child_index) + 1);
                let role = format!("worker-{child_index:04}");
                write_session_transcript(
                    &mut child_writer,
                    SessionTranscriptSpec {
                        session_id: child_id,
                        parent: Some((root_id, Some(role.clone()))),
                        cwd: &cwd,
                        title: &format!("stress-child-{child_index:04}"),
                        turns: args.turns,
                        messages_per_turn: args.messages_per_turn,
                        payload: &payload,
                        base: child_base,
                    },
                )?;
                children.push(ChildSpec { id: child_id, role });
                child_session_ids.push(child_id.to_string());
            }

            // Append parent↔child communication + SubAgent link items onto the
            // already-written hot root (re-open for append).
            drop(writer);
            let append = File::options()
                .append(true)
                .open(&root_path)
                .with_context(|| format!("re-open {}", root_path.display()))?;
            let mut append = BufWriter::new(append);
            write_hot_parent_traffic(
                &mut append,
                root_id,
                &children,
                args.comm_rounds,
                &payload,
                base + Duration::hours(1),
            )?;
            append.flush()?;

            // Mirror communication rounds into each child transcript.
            for (child_index, child) in children.iter().enumerate() {
                let child_path = find_child_rollout(&nest_root, child.id)?;
                let child_append = File::options()
                    .append(true)
                    .open(&child_path)
                    .with_context(|| format!("re-open {}", child_path.display()))?;
                let mut child_append = BufWriter::new(child_append);
                write_child_comm_rounds(
                    &mut child_append,
                    child.id,
                    root_id,
                    child_index as u32,
                    args.comm_rounds,
                    &payload,
                    base + Duration::hours(1) + Duration::seconds(i64::from(child_index as u32)),
                )?;
                child_append.flush()?;
            }
        }
    }

    let hot_root_id = hot_root_id.context("hot root was not generated")?;
    let manifest = Manifest {
        schema_version: 1,
        roots: args.roots,
        hot_root_id: hot_root_id.to_string(),
        hot_root_index: args.hot_root_index,
        subagents: args.subagents,
        turns: args.turns,
        messages_per_turn: args.messages_per_turn,
        comm_rounds: args.comm_rounds,
        message_bytes: args.message_bytes,
        child_session_ids,
        sessions_dir: sessions_dir.display().to_string(),
        artifacts_dir: artifacts_dir.display().to_string(),
    };
    let manifest_path = args.out.join("stress-manifest.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).context("serialize manifest")?,
    )
    .with_context(|| format!("write {}", manifest_path.display()))?;

    println!(
        "Wrote stress corpus to {}\n  roots={} subagents={} hot_root={}\n  manifest={}",
        args.out.display(),
        args.roots,
        args.subagents,
        hot_root_id,
        manifest_path.display()
    );
    Ok(())
}

struct ChildSpec {
    id: SessionId,
    role: String,
}

fn jsonl_writer(path: &Path) -> Result<BufWriter<File>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    Ok(BufWriter::new(file))
}

fn write_line(writer: &mut impl Write, line: &devo_core::RolloutLine) -> Result<()> {
    serde_json::to_writer(&mut *writer, line).context("serialize rollout line")?;
    writer.write_all(b"\n")?;
    Ok(())
}

fn model_binding() -> ModelBinding {
    ModelBinding {
        provider: "stress".into(),
        model: "stress-model".into(),
        variant: None,
        reasoning_effort: None,
    }
}

fn base_session(
    id: SessionId,
    parent: Option<(SessionId, Option<String>)>,
    cwd: &Path,
    title: &str,
    at: chrono::DateTime<Utc>,
) -> Session {
    Session {
        id,
        version: 1,
        cwd: cwd.to_path_buf(),
        additional_directories: Vec::new(),
        parent: parent.map(|(parent_id, role)| SessionParent::Agent {
            session_id: parent_id,
            role,
        }),
        fork_from_id: None,
        at_turn_id: None,
        ephemeral: false,
        created_at: at,
        status: SessionStatus::Idle,
        activity: SessionActivity::Idle,
        flags: Vec::new(),
        archived: false,
        active_turn_id: None,
        queued_count: 0,
        title: Some(title.to_string()),
        title_state: SessionTitleState::Final(
            devo_protocol::SessionTitleFinalSource::ExplicitCreate,
        ),
        model: model_binding(),
        settings: SessionSettings {
            permission_profile: PermissionProfile::AutoReview,
            reasoning_effort: None,
            mode: Some("build".into()),
            sandbox_profile: None,
            effective_context_window: None,
            auto_refine_enabled: None,
            auto_refine_turn_interval: None,
            python_cell_first_wait_ms: None,
        },
        git_info: None,
        preview: title.to_string(),
        last_activity_at: at,
        transcript_size_bytes: None,
        message_count: None,
        summary: None,
        task_state: Some("idle".into()),
        usage: SessionUsage {
            total: UsageTotals::default(),
            by_purpose: Vec::new(),
            legacy: None,
            updated_at: at,
        },
    }
}

struct SessionTranscriptSpec<'a> {
    session_id: SessionId,
    parent: Option<(SessionId, Option<String>)>,
    cwd: &'a Path,
    title: &'a str,
    turns: u32,
    messages_per_turn: u32,
    payload: &'a str,
    base: chrono::DateTime<Utc>,
}

fn write_session_transcript(
    writer: &mut impl Write,
    spec: SessionTranscriptSpec<'_>,
) -> Result<()> {
    let SessionTranscriptSpec {
        session_id,
        parent,
        cwd,
        title,
        turns,
        messages_per_turn,
        payload,
        base,
    } = spec;
    let session = base_session(session_id, parent, cwd, title, base);
    let extras = SessionPersistenceExtras {
        session_context: None,
        cli_version: "stress".into(),
        source: "stress".into(),
        collaboration_mode: None,
        permission_preset: None,
        kernel_snapshot_path: None,
    };
    write_line(writer, &session_line(session, Some(extras), base))?;

    let mut seq = 1u64;
    for turn_index in 0..turns {
        let turn_id = TurnId::new();
        let started = base + Duration::seconds(i64::from(turn_index) * 5);
        let completed = started + Duration::seconds(2);
        let turn = Turn {
            id: turn_id,
            session_id,
            sequence: turn_index + 1,
            kind: TurnKind::Regular,
            status: TurnStatus::Completed,
            model: model_binding(),
            collaboration_mode: None,
            started_at: started,
            completed_at: Some(completed),
            error: None,
            usage: None,
        };
        write_line(writer, &turn_line(turn, None, started))?;

        for msg_index in 0..messages_per_turn {
            let user_at = started + Duration::milliseconds(i64::from(msg_index) * 20);
            write_line(
                writer,
                &item_line(
                    session_id,
                    turn_id,
                    seq,
                    user_at,
                    ItemState::Completed,
                    Item::UserMessage {
                        client_user_message_id: None,
                        content: vec![UserInput::Text {
                            text: format!(
                                "stress user turn={turn_index} msg={msg_index} session={session_id}"
                            ),
                        }],
                        entry: UserMessageEntry::TurnStart,
                    },
                ),
            )?;
            seq += 1;

            let assistant_at = user_at + Duration::milliseconds(5);
            write_line(
                writer,
                &item_line(
                    session_id,
                    turn_id,
                    seq,
                    assistant_at,
                    ItemState::Completed,
                    Item::AssistantMessage {
                        text: format!(
                            "stress assistant turn={turn_index} msg={msg_index}\n{payload}"
                        ),
                    },
                ),
            )?;
            seq += 1;
        }
    }

    Ok(())
}

fn write_hot_parent_traffic(
    writer: &mut impl Write,
    parent_id: SessionId,
    children: &[ChildSpec],
    comm_rounds: u32,
    payload: &str,
    base: chrono::DateTime<Utc>,
) -> Result<()> {
    if children.is_empty() {
        return Ok(());
    }
    let turn_id = TurnId::new();
    let started = base;
    let turn = Turn {
        id: turn_id,
        session_id: parent_id,
        sequence: 10_000,
        kind: TurnKind::Regular,
        status: TurnStatus::Completed,
        model: model_binding(),
        collaboration_mode: None,
        started_at: started,
        completed_at: Some(started + Duration::seconds(30)),
        error: None,
        usage: None,
    };
    write_line(writer, &turn_line(turn, None, started))?;

    let mut seq = 100_000u64;
    for (child_index, child) in children.iter().enumerate() {
        write_line(
            writer,
            &item_line(
                parent_id,
                turn_id,
                seq,
                started + Duration::milliseconds(i64::from(child_index as u32)),
                ItemState::Completed,
                Item::SubAgent {
                    origin_call_id: Some(format!("spawn-{child_index}")),
                    agent_session_id: child.id,
                    parent_session_id: parent_id,
                    role: Some(child.role.clone()),
                    task: format!("stress task for {}", child.role),
                    state: SpawnedWorkState::Completed,
                },
            ),
        )?;
        seq += 1;
    }

    for round in 0..comm_rounds {
        let child = &children[round as usize % children.len()];
        let at = started + Duration::seconds(i64::from(round) + 1);
        write_line(
            writer,
            &item_line(
                parent_id,
                turn_id,
                seq,
                at,
                ItemState::Completed,
                Item::AssistantMessage {
                    text: format!(
                        "parent→{} round={round} ask for status\n{payload}",
                        child.role
                    ),
                },
            ),
        )?;
        seq += 1;
        write_line(
            writer,
            &item_line(
                parent_id,
                turn_id,
                seq,
                at + Duration::milliseconds(2),
                ItemState::Completed,
                Item::UserMessage {
                    client_user_message_id: None,
                    content: vec![UserInput::Text {
                        text: format!(
                            "relay from {} round={round}: acknowledged parent request",
                            child.role
                        ),
                    }],
                    entry: UserMessageEntry::Steer,
                },
            ),
        )?;
        seq += 1;
    }
    Ok(())
}

fn write_child_comm_rounds(
    writer: &mut impl Write,
    child_id: SessionId,
    parent_id: SessionId,
    child_index: u32,
    comm_rounds: u32,
    payload: &str,
    base: chrono::DateTime<Utc>,
) -> Result<()> {
    let turn_id = TurnId::new();
    let started = base;
    let turn = Turn {
        id: turn_id,
        session_id: child_id,
        sequence: 10_000,
        kind: TurnKind::Regular,
        status: TurnStatus::Completed,
        model: model_binding(),
        collaboration_mode: None,
        started_at: started,
        completed_at: Some(started + Duration::seconds(20)),
        error: None,
        usage: None,
    };
    write_line(writer, &turn_line(turn, None, started))?;

    // Each child gets every Nth round plus a dense local slice so resume/items
    // paths stay large without cloning the entire parent fan-out into every file.
    let mut seq = 50_000u64;
    let local_rounds = comm_rounds.clamp(1, 40);
    for round in 0..local_rounds {
        if round % 3 != child_index % 3 && round > 5 {
            continue;
        }
        let at = started + Duration::seconds(i64::from(round) + 1);
        write_line(
            writer,
            &item_line(
                child_id,
                turn_id,
                seq,
                at,
                ItemState::Completed,
                Item::UserMessage {
                    client_user_message_id: None,
                    content: vec![UserInput::Text {
                        text: format!("from parent {parent_id} round={round} child={child_index}"),
                    }],
                    entry: UserMessageEntry::TurnStart,
                },
            ),
        )?;
        seq += 1;
        write_line(
            writer,
            &item_line(
                child_id,
                turn_id,
                seq,
                at + Duration::milliseconds(3),
                ItemState::Completed,
                Item::AssistantMessage {
                    text: format!("child reply round={round}\n{payload}"),
                },
            ),
        )?;
        seq += 1;
    }
    Ok(())
}

fn item_line(
    session_id: SessionId,
    turn_id: TurnId,
    seq: u64,
    at: chrono::DateTime<Utc>,
    state: ItemState,
    item: Item,
) -> devo_core::RolloutLine {
    devo_core::RolloutLine::Item {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp: at,
        item: ItemEnvelope {
            id: ItemId::new(),
            session_id,
            turn_id,
            seq,
            revision: 1,
            created_at: at,
            updated_at: at,
            state,
            item,
            parent_id: None,
        },
    }
}

fn padded_text(prefix: &str, bytes: usize) -> String {
    if bytes <= prefix.len() {
        return prefix.chars().take(bytes).collect();
    }
    let mut out = String::with_capacity(bytes);
    out.push_str(prefix);
    out.push('\n');
    while out.len() < bytes {
        out.push('x');
    }
    out.truncate(bytes);
    out
}

fn create_unique_sub_dir(parent_dir: &Path) -> Result<PathBuf> {
    for _ in 0..100 {
        let name = format!("sub-{}", &Uuid::new_v4().simple().to_string()[..8]);
        let child_dir = parent_dir.join(name);
        match fs::create_dir(&child_dir) {
            Ok(()) => return Ok(child_dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).context(format!("create {}", child_dir.display()));
            }
        }
    }
    bail!(
        "unable to create unique subagent dir under {}",
        parent_dir.display()
    )
}

fn find_child_rollout(nest_root: &Path, child_id: SessionId) -> Result<PathBuf> {
    let expected = format!("{child_id}.jsonl");
    for entry in fs::read_dir(nest_root).with_context(|| format!("read {}", nest_root.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let candidate = entry.path().join(&expected);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!(
        "child rollout for {child_id} not found under {}",
        nest_root.display()
    )
}
