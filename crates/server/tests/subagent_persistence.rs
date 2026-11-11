//! End-to-end regression coverage for one root JSONL per subagent tree.
//!
//! These tests exercise the public runtime, DB index, filesystem layout, and
//! Native RPC surfaces. Store-level writer/index unit tests stay in
//! `crates/server/src/persistence_tests.rs`; lifecycle consumer tests stay in
//! the lifecycle-owned suites.

use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use devo_core::InternalRecord;
use devo_core::RolloutLine;
use devo_core::SessionPersistenceExtras;
use devo_core::parse_rollout_line;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::SessionId;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_protocol::native::ids::ItemId;
use devo_protocol::native::item::ItemEnvelope;
use devo_protocol::native::page::Page;
use devo_protocol::native::rpc_schedule::SessionExportResult;
use devo_protocol::native::rpc_session::RestorePlan;
use devo_protocol::native::rpc_session::SessionForkResult;
use devo_protocol::native::rpc_session::SessionNewResult;
use devo_protocol::native::rpc_session::SessionResumeResult;
use devo_protocol::native::session::Session;
use devo_protocol::native::session::SessionParent;
use devo_provider::ModelProviderSDK;
use devo_server::ErrorResponse;
use devo_server::ProtocolErrorCode;
use devo_server::ServerRuntime;
use devo_server::db::Database;
use devo_server::db::SessionIndexRow;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Barrier;
use tokio::sync::mpsc;
use tokio::time::timeout;

#[path = "support/subagent_lifecycle.rs"]
mod support;

use support::ScriptedProvider;
use support::build_runtime;
use support::initialize_connection;
use support::request_agent_list;
use support::spawn_child_named;
use support::start_parent_session;
use support::start_turn;
use support::wait_for_child_turn_started;
use support::wait_for_session_idle;
use support::wait_for_session_notification;

const DB_FILE: &str = "subagent_lifecycle.db";

/// A provider that holds task streams until all expected task turns have
/// arrived. This makes the concurrent-append test deterministic without sleeps
/// or polling.
struct BarrierProvider {
    barrier: Arc<Barrier>,
}

impl BarrierProvider {
    fn new(expected_turns: usize) -> Self {
        Self {
            barrier: Arc::new(Barrier::new(expected_turns)),
        }
    }
}

#[async_trait]
impl ModelProviderSDK for BarrierProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(text_response("barrier title"))
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        if !support::is_child_completion_request(&request) {
            self.barrier.wait().await;
        }
        let events = vec![
            StreamEvent::TextDelta {
                index: 0,
                text: "concurrent reply".to_string(),
            },
            StreamEvent::MessageDone {
                response: text_response("concurrent reply"),
            },
        ];
        Ok(Box::pin(futures::stream::iter(
            events.into_iter().map(Ok).collect::<Vec<_>>(),
        )))
    }

    fn name(&self) -> &str {
        "barrier-provider"
    }
}

fn text_response(text: &str) -> ModelResponse {
    ModelResponse {
        id: "test-response".to_string(),
        content: vec![ResponseContent::Text(text.to_string())],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage::default(),
        metadata: ResponseMetadata::default(),
    }
}

async fn rpc(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    runtime
        .handle_incoming(
            connection_id,
            json!({ "id": id, "method": method, "params": params }),
        )
        .await
        .with_context(|| format!("{method} returned no response"))
}

fn ensure_success(response: &Value) -> Result<()> {
    if let Some(error) = response.get("error") {
        anyhow::bail!("unexpected RPC error: {error}");
    }
    Ok(())
}

async fn list_sessions(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    include_children: bool,
) -> Result<Vec<Session>> {
    let response = rpc(
        runtime,
        connection_id,
        100,
        "session/list",
        json!({ "includeChildren": include_children }),
    )
    .await?;
    ensure_success(&response)?;
    let page: Page<Session> = serde_json::from_value(response["result"].clone())?;
    Ok(page.data)
}

async fn list_session_ids(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    include_children: bool,
) -> Result<HashSet<SessionId>> {
    Ok(list_sessions(runtime, connection_id, include_children)
        .await?
        .into_iter()
        .map(|session| session.id)
        .collect())
}

async fn list_items(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
) -> Result<Vec<ItemEnvelope>> {
    let response = rpc(
        runtime,
        connection_id,
        101,
        "session/items/list",
        json!({ "sessionId": session_id }),
    )
    .await?;
    ensure_success(&response)?;
    let page: Page<ItemEnvelope> = serde_json::from_value(response["result"].clone())?;
    Ok(page.data)
}

async fn resume_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
) -> Result<Session> {
    let response = rpc(
        runtime,
        connection_id,
        102,
        "session/resume",
        json!({ "sessionId": session_id }),
    )
    .await?;
    ensure_success(&response)?;
    let result: SessionResumeResult = serde_json::from_value(response["result"].clone())?;
    Ok(result.session)
}

/// Subscribe the connection to a child session so notifications from its
/// direct children (grandchildren of the original root) are delivered. Native
/// session resume installs the legacy session subscription with
/// `include_child_agents` enabled; a root subscription only covers direct
/// children.
async fn subscribe_to_child_notifications(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    child_session_id: SessionId,
) -> Result<()> {
    let _ = resume_session(runtime, connection_id, child_session_id).await?;
    Ok(())
}

async fn create_root_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &Path,
    idempotency_key: &str,
) -> Result<SessionId> {
    let response = rpc(
        runtime,
        connection_id,
        111,
        "session/new",
        json!({ "cwd": cwd, "idempotencyKey": idempotency_key }),
    )
    .await?;
    ensure_success(&response)?;
    let result: SessionNewResult = serde_json::from_value(response["result"].clone())?;
    Ok(result.session.id)
}

async fn fork_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    source_session_id: SessionId,
) -> Result<SessionId> {
    let response = rpc(
        runtime,
        connection_id,
        112,
        "session/fork",
        json!({ "sessionId": source_session_id }),
    )
    .await?;
    ensure_success(&response)?;
    let result: SessionForkResult = serde_json::from_value(response["result"].clone())?;
    Ok(result.session.id)
}

async fn delete_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
) -> Result<()> {
    let response = rpc(
        runtime,
        connection_id,
        103,
        "session/delete",
        json!({ "sessionId": session_id }),
    )
    .await?;
    ensure_success(&response)
}

async fn export_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
    path: &Path,
) -> Result<PathBuf> {
    let response = rpc(
        runtime,
        connection_id,
        104,
        "session/export",
        json!({
            "sessionId": session_id,
            "format": "jsonl",
            "path": path,
        }),
    )
    .await?;
    ensure_success(&response)?;
    let result: SessionExportResult = serde_json::from_value(response["result"].clone())?;
    Ok(result.path)
}

fn parent_session_id(session: &Session) -> Option<SessionId> {
    session.parent.as_ref().map(|parent| {
        let SessionParent::Agent { session_id, .. } = parent;
        *session_id
    })
}

fn root_journal(data_root: &Path, root_session_id: SessionId) -> PathBuf {
    data_root
        .join("sessions")
        .join(format!("{root_session_id}.jsonl"))
}

fn expected_child_dir(
    data_root: &Path,
    root_session_id: SessionId,
    child_id: SessionId,
) -> PathBuf {
    data_root
        .join("session-artifacts")
        .join(root_session_id.to_string())
        .join("agents")
        .join(child_id.to_string())
}

fn journal_files(data_root: &Path) -> Result<Vec<PathBuf>> {
    let sessions_dir = data_root.join("sessions");
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&sessions_dir)? {
        let path = entry?.path();
        if path.extension() == Some(OsStr::new("jsonl")) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn read_journal(path: &Path) -> Result<Vec<RolloutLine>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read journal {}", path.display()))?;
    let mut lines = Vec::new();
    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        lines.push(parse_rollout_line(raw)?);
    }
    Ok(lines)
}

fn line_session_ids(lines: &[RolloutLine]) -> HashSet<SessionId> {
    lines.iter().filter_map(RolloutLine::session_id).collect()
}

fn item_contains_text(item: &ItemEnvelope, needle: &str) -> bool {
    serde_json::to_string(&item.item).is_ok_and(|serialized| serialized.contains(needle))
}

fn session_lines(lines: &[RolloutLine], session_id: SessionId) -> Vec<RolloutLine> {
    lines
        .iter()
        .filter(|line| line.session_id() == Some(session_id))
        .cloned()
        .collect()
}

fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

fn assert_no_child_journal(data_root: &Path, child_ids: &[SessionId]) -> Result<()> {
    let mut files = Vec::new();
    collect_files(&data_root.join("session-artifacts"), &mut files)?;
    let child_journals: HashSet<String> = child_ids
        .iter()
        .map(|child_id| format!("{child_id}.jsonl"))
        .collect();
    assert!(
        !files.iter().any(|path| path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| child_journals.contains(name))),
        "child rollouts must not exist; files: {files:?}"
    );
    Ok(())
}

async fn wait_for_turn_completions(
    notifications: &mut mpsc::Receiver<Value>,
    expected: &[SessionId],
) -> Result<()> {
    let mut remaining: HashMap<SessionId, usize> = HashMap::new();
    for session_id in expected {
        *remaining.entry(*session_id).or_default() += 1;
    }
    timeout(Duration::from_secs(15), async {
        while !remaining.is_empty() {
            let event = notifications
                .recv()
                .await
                .context("notification channel closed before all turns completed")?;
            if event.get("method") != Some(&json!("turn/completed")) {
                continue;
            }
            let session_id = event["params"]["turn"]["sessionId"]
                .as_str()
                .or_else(|| event["params"]["sessionId"].as_str())
                .or_else(|| event["params"]["session_id"].as_str());
            if let Some(session_id) = session_id {
                let session_id = SessionId::from(session_id.to_string());
                if let Some(count) = remaining.get_mut(&session_id) {
                    *count -= 1;
                    if *count == 0 {
                        remaining.remove(&session_id);
                    }
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("timed out waiting for turn completions")??;
    Ok(())
}

fn indexed_rollout_path(db: &Database, session_id: SessionId) -> Result<Option<PathBuf>> {
    Ok(db
        .get_session_index(&session_id)?
        .and_then(|index| index.rollout_path))
}

fn stale_index_row(
    data_root: &Path,
    session_id: SessionId,
    parent_session_id: SessionId,
) -> SessionIndexRow {
    let now = Utc::now();
    SessionIndexRow {
        session_id,
        cwd: data_root.to_path_buf(),
        additional_directories: Vec::new(),
        created_at: now,
        updated_at: now,
        last_activity_at: now,
        title: Some("stale child".to_string()),
        title_state: devo_core::SessionTitleState::Unset,
        parent_session_id: Some(parent_session_id),
        fork_from_id: None,
        fork_at_turn_id: None,
        agent_path: Some("root/stale".to_string()),
        ephemeral: false,
        model: None,
        reasoning_effort_selection: None,
    }
}

fn clear_index_rollout_path(data_root: &Path, session_id: SessionId) -> Result<()> {
    set_index_rollout_path(data_root, session_id, None)
}

fn set_index_rollout_path(
    data_root: &Path,
    session_id: SessionId,
    path: Option<&Path>,
) -> Result<()> {
    let conn = rusqlite::Connection::open(data_root.join(DB_FILE))?;
    let value = path.map(|path| path.to_string_lossy().into_owned());
    let updated = conn.execute(
        "UPDATE sessions SET rollout_path = ?1 WHERE id = ?2",
        rusqlite::params![value, session_id.to_string()],
    )?;
    assert_eq!(updated, 1, "expected one indexed row for {session_id}");
    Ok(())
}

#[tokio::test]
async fn persistent_descendants_share_one_journal_and_keep_separate_artifact_dirs() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child reply"),
        ScriptedProvider::completed("grandchild reply"),
        ScriptedProvider::completed("root reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;

    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;

    subscribe_to_child_notifications(&runtime, connection_id, child.child_session_id).await?;
    let grandchild = spawn_child_named(
        &runtime,
        child.child_session_id,
        "grandchild task",
        Some("none"),
        Some("grandchild"),
    )
    .await?;
    wait_for_session_notification(
        &mut notifications,
        "turn/completed",
        grandchild.child_session_id,
    )
    .await?;

    let journal = root_journal(data_root.path(), root_session_id);
    assert_eq!(journal_files(data_root.path())?, vec![journal.clone()]);
    assert!(journal.is_file(), "root journal must exist");

    let child_artifact_dir =
        expected_child_dir(data_root.path(), root_session_id, child.child_session_id);
    let grandchild_artifact_dir = expected_child_dir(
        data_root.path(),
        root_session_id,
        grandchild.child_session_id,
    );
    assert_eq!(
        child.session_dir.as_deref().map(PathBuf::from),
        Some(child_artifact_dir.clone())
    );
    assert_eq!(
        grandchild.session_dir.as_deref().map(PathBuf::from),
        Some(grandchild_artifact_dir.clone())
    );
    assert!(child_artifact_dir.is_dir());
    assert!(grandchild_artifact_dir.is_dir());
    assert_ne!(child_artifact_dir, grandchild_artifact_dir);
    assert!(!child_artifact_dir.starts_with(data_root.path().join("sessions")));
    assert!(!grandchild_artifact_dir.starts_with(data_root.path().join("sessions")));

    assert_no_child_journal(
        data_root.path(),
        &[child.child_session_id, grandchild.child_session_id],
    )?;

    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(
        indexed_rollout_path(&db, root_session_id)?,
        Some(journal.clone())
    );
    assert_eq!(
        indexed_rollout_path(&db, child.child_session_id)?,
        Some(journal.clone())
    );
    assert_eq!(
        indexed_rollout_path(&db, grandchild.child_session_id)?,
        Some(journal.clone())
    );

    let child_index = db
        .get_session_index(&child.child_session_id)?
        .expect("child index");
    assert_eq!(child_index.session.parent_session_id, Some(root_session_id));
    assert_eq!(
        child_index.session.agent_path.as_deref(),
        Some("root/child")
    );
    let grandchild_index = db
        .get_session_index(&grandchild.child_session_id)?
        .expect("grandchild index");
    assert_eq!(
        grandchild_index.session.parent_session_id,
        Some(child.child_session_id)
    );
    assert_eq!(
        grandchild_index.session.agent_path.as_deref(),
        Some("root/child/grandchild")
    );

    let lines = read_journal(&journal)?;
    let expected_ids: HashSet<SessionId> = [
        root_session_id,
        child.child_session_id,
        grandchild.child_session_id,
    ]
    .into_iter()
    .collect();
    assert_eq!(line_session_ids(&lines), expected_ids);
    assert!(lines.iter().all(|line| line.session_id().is_some()));

    let meta_ids: HashSet<SessionId> = lines
        .iter()
        .filter_map(|line| match line {
            RolloutLine::SessionMeta { session, .. } => Some(session.id),
            _ => None,
        })
        .collect();
    assert_eq!(meta_ids, expected_ids);

    assert_eq!(
        list_session_ids(&runtime, connection_id, /*include_children*/ true).await?,
        expected_ids
    );
    let agents = request_agent_list(&runtime, connection_id, root_session_id).await?;
    let agent_ids: HashSet<SessionId> =
        agents.agents.iter().map(|agent| agent.session_id).collect();
    assert_eq!(
        agent_ids,
        [child.child_session_id].into_iter().collect(),
        "root lists direct children only"
    );
    let grandchild_agents =
        request_agent_list(&runtime, connection_id, child.child_session_id).await?;
    assert_eq!(
        grandchild_agents
            .agents
            .iter()
            .map(|agent| agent.session_id)
            .collect::<HashSet<_>>(),
        [grandchild.child_session_id].into_iter().collect()
    );

    Ok(())
}

#[tokio::test]
async fn legacy_child_journal_remains_readable_and_unmodified() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(
        data_root.path(),
        Arc::new(ScriptedProvider::for_agent_tasks([])) as Arc<dyn ModelProviderSDK>,
    )?;
    let (connection_id, _) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let root_session = resume_session(&runtime, connection_id, root_session_id).await?;

    let child_id = SessionId::new();
    let now = Utc::now();
    let mut child_session = root_session.clone();
    child_session.id = child_id;
    child_session.version = 1;
    child_session.parent = Some(SessionParent::Agent {
        session_id: root_session_id,
        role: Some("default".to_string()),
    });
    child_session.fork_from_id = None;
    child_session.at_turn_id = None;
    child_session.ephemeral = false;
    child_session.created_at = now;
    child_session.status = devo_protocol::native::session::SessionStatus::Idle;
    child_session.flags.clear();
    child_session.archived = false;
    child_session.activity = devo_protocol::native::session::SessionActivity::Idle;
    child_session.active_turn_id = None;
    child_session.queued_count = 0;
    child_session.title = Some("legacy child".to_string());
    child_session.title_state =
        devo_core::SessionTitleState::Final(devo_core::SessionTitleFinalSource::ExplicitCreate);
    child_session.last_activity_at = now;
    child_session.transcript_size_bytes = None;
    child_session.message_count = None;
    child_session.summary = None;
    child_session.task_state = None;

    let extras = SessionPersistenceExtras {
        session_context: None,
        cli_version: "test".to_string(),
        source: "cli".to_string(),
        collaboration_mode: None,
        permission_preset: None,
        agent_path: Some("root/legacy".to_string()),
        agent_nickname: Some("legacy".to_string()),
        kernel_snapshot_path: None,
    };
    let legacy_dir = data_root
        .path()
        .join("session-artifacts")
        .join(root_session_id.to_string())
        .join("sub-legacy");
    std::fs::create_dir_all(&legacy_dir)?;
    let legacy_journal = legacy_dir.join(format!("{child_id}.jsonl"));
    let line = devo_core::session_line(child_session, Some(extras), now);
    std::fs::write(
        &legacy_journal,
        format!(
            "{}
",
            serde_json::to_string(&line)?
        ),
    )?;
    let before = std::fs::read(&legacy_journal)?;

    runtime.refresh_session_index()?;
    let resumed = resume_session(&runtime, connection_id, child_id).await?;
    assert_eq!(resumed.id, child_id);
    assert_eq!(parent_session_id(&resumed), Some(root_session_id));
    assert_eq!(resumed.title.as_deref(), Some("legacy child"));

    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(
        indexed_rollout_path(&db, child_id)?,
        Some(legacy_journal.clone())
    );
    let index = db
        .get_session_index(&child_id)?
        .expect("legacy child index");
    assert_eq!(index.session.agent_path.as_deref(), Some("root/legacy"));

    assert_eq!(
        std::fs::read(&legacy_journal)?,
        before,
        "legacy journal must not be rewritten by indexing or resume"
    );

    let root_journal_path = root_journal(data_root.path(), root_session_id);
    let root_before = std::fs::read(&root_journal_path)?;
    delete_session(&runtime, connection_id, child_id).await?;
    assert!(
        !legacy_journal.exists(),
        "legacy child delete must unlink its own journal"
    );
    assert!(
        !legacy_dir.exists(),
        "legacy child artifact dir must be removed"
    );
    assert!(root_journal_path.is_file(), "root journal must remain");
    assert_eq!(
        std::fs::read(&root_journal_path)?,
        root_before,
        "legacy child delete must not rewrite the root journal"
    );
    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(indexed_rollout_path(&db, child_id)?, None);
    assert_eq!(
        indexed_rollout_path(&db, root_session_id)?,
        Some(root_journal_path.clone())
    );
    runtime.refresh_session_index()?;
    let child_resume = rpc(
        &runtime,
        connection_id,
        110,
        "session/resume",
        json!({ "sessionId": child_id }),
    )
    .await?;
    let error: ErrorResponse = serde_json::from_value(child_resume)?;
    assert_eq!(error.error.code, ProtocolErrorCode::SessionNotFound);
    assert_eq!(
        list_session_ids(&runtime, connection_id, /*include_children*/ true).await?,
        [root_session_id].into_iter().collect()
    );
    Ok(())
}

#[tokio::test]
async fn concurrent_turns_keep_shared_journal_attributed_and_sequenced() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(BarrierProvider::new(3));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;

    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "concurrent child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_child_turn_started(&mut notifications, child.child_session_id).await?;

    subscribe_to_child_notifications(&runtime, connection_id, child.child_session_id).await?;
    let grandchild = spawn_child_named(
        &runtime,
        child.child_session_id,
        "concurrent grandchild task",
        Some("none"),
        Some("grandchild"),
    )
    .await?;
    wait_for_child_turn_started(&mut notifications, grandchild.child_session_id).await?;

    start_turn(
        &runtime,
        connection_id,
        root_session_id,
        "concurrent root task",
    )
    .await?;
    wait_for_turn_completions(
        &mut notifications,
        &[
            root_session_id,
            child.child_session_id,
            grandchild.child_session_id,
            // Grandchild completion wakes the child, and both child turns
            // produce distinct completion turns in the root.
            child.child_session_id,
            root_session_id,
            root_session_id,
        ],
    )
    .await?;

    for session_id in [
        grandchild.child_session_id,
        child.child_session_id,
        root_session_id,
    ] {
        wait_for_session_idle(&runtime, connection_id, session_id).await?;
    }

    let journal = root_journal(data_root.path(), root_session_id);
    let lines = read_journal(&journal)?;
    let expected_ids: HashSet<SessionId> = [
        root_session_id,
        child.child_session_id,
        grandchild.child_session_id,
    ]
    .into_iter()
    .collect();
    assert_eq!(line_session_ids(&lines), expected_ids);
    assert!(lines.iter().all(|line| line.session_id().is_some()));

    let mut item_seqs: HashMap<SessionId, Vec<u64>> = HashMap::new();
    let mut item_ids: HashMap<SessionId, HashSet<ItemId>> = HashMap::new();
    let mut seq_by_item: HashMap<(SessionId, ItemId), u64> = HashMap::new();
    for line in &lines {
        if let RolloutLine::Item { item, .. } = line {
            item_seqs.entry(item.session_id).or_default().push(item.seq);
            item_ids.entry(item.session_id).or_default().insert(item.id);
            if let Some(previous_seq) = seq_by_item.insert((item.session_id, item.id), item.seq) {
                assert_eq!(
                    previous_seq, item.seq,
                    "one item id must keep one sequence in session {}",
                    item.session_id
                );
            }
        }
    }
    assert_eq!(
        item_seqs.keys().copied().collect::<HashSet<_>>(),
        expected_ids
    );
    for session_id in &expected_ids {
        let item_count = item_ids
            .get(session_id)
            .map(HashSet::len)
            .unwrap_or_default();
        // Each turn adds input + assistant output. The first turn also emits
        // the fresh-kernel notice; completion turns reuse that same kernel.
        // Only the running child adds a durable outgoing spawn row.
        let expected_count = if *session_id == root_session_id {
            7
        } else if *session_id == child.child_session_id {
            6
        } else {
            3
        };
        assert_eq!(item_count, expected_count, "item count in {session_id}");
        let mut unique = item_seqs
            .get(session_id)
            .cloned()
            .expect("item seqs for session");
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique,
            (1..=expected_count as u64).collect::<Vec<_>>(),
            "each session must have contiguous unique item seqs"
        );
    }
    assert_eq!(seq_by_item.len(), 16, "full expected item-id/seq count");

    // Pin the identities behind those totals, not just the number of rows.
    // Counts are human inputs, host inputs, assistant outputs, kernel notices,
    // and outgoing child-spawn rows, respectively.
    let mut item_kinds: HashMap<SessionId, [usize; 5]> = HashMap::new();
    let mut counted_items = HashSet::new();
    for line in &lines {
        if let RolloutLine::Item { item, .. } = line
            && counted_items.insert((item.session_id, item.id))
        {
            use devo_protocol::native::item::Item;
            let kind = match &item.item {
                Item::UserMessage { origin: None, .. } => 0,
                Item::UserMessage {
                    origin: Some(_), ..
                } => 1,
                Item::AssistantMessage { .. } => 2,
                Item::Warning { code, .. } if code == "ipythonKernelStateFresh" => 3,
                Item::SubAgent { .. } => 4,
                unexpected => panic!("unexpected persisted item: {unexpected:?}"),
            };
            item_kinds.entry(item.session_id).or_default()[kind] += 1;
        }
    }
    assert_eq!(
        item_kinds,
        HashMap::from([
            (root_session_id, [1, 2, 3, 1, 0]),
            (child.child_session_id, [0, 2, 2, 1, 1]),
            (grandchild.child_session_id, [0, 1, 1, 1, 0]),
        ])
    );

    let mut host_senders: HashMap<SessionId, Vec<String>> = HashMap::new();
    let mut host_input_ids = HashSet::new();
    for line in &lines {
        if let RolloutLine::Item { item, .. } = line
            && let devo_protocol::native::item::Item::UserMessage {
                origin: Some(origin),
                ..
            } = &item.item
            && host_input_ids.insert((item.session_id, item.id))
        {
            assert_eq!(
                origin.source,
                devo_protocol::native::host_message::HostMessageSource::Agent
            );
            host_senders
                .entry(item.session_id)
                .or_default()
                .push(origin.sender.clone());
        }
    }
    assert_eq!(
        host_senders,
        HashMap::from([
            (root_session_id, vec![child.child_session_id.to_string(); 2]),
            (
                child.child_session_id,
                vec![
                    root_session_id.to_string(),
                    grandchild.child_session_id.to_string()
                ]
            ),
            (
                grandchild.child_session_id,
                vec![child.child_session_id.to_string()]
            ),
        ]),
        "spawn tasks and completion items must retain their actual sender; no grandchild completion goes directly to root"
    );

    let spawn_rows: Vec<_> = lines
        .iter()
        .filter_map(|line| match line {
            RolloutLine::Item { item, .. } => match &item.item {
                devo_protocol::native::item::Item::SubAgent {
                    agent_session_id,
                    agent_nickname,
                    parent_session_id,
                    ..
                } => Some((
                    item.session_id,
                    *agent_session_id,
                    agent_nickname.clone(),
                    *parent_session_id,
                )),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        spawn_rows,
        vec![(
            child.child_session_id,
            grandchild.child_session_id,
            Some("grandchild".to_owned()),
            child.child_session_id,
        )]
    );
    Ok(())
}

#[tokio::test]
async fn restart_resumes_each_session_from_shared_journal_with_isolated_history() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("reply-alpha"),
        ScriptedProvider::completed("reply-beta"),
        ScriptedProvider::completed("reply-gamma"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;

    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_turn_completions(
        &mut notifications,
        &[child.child_session_id, root_session_id],
    )
    .await?;
    wait_for_session_idle(&runtime, connection_id, child.child_session_id).await?;
    wait_for_session_idle(&runtime, connection_id, root_session_id).await?;
    subscribe_to_child_notifications(&runtime, connection_id, child.child_session_id).await?;
    let grandchild = spawn_child_named(
        &runtime,
        child.child_session_id,
        "grandchild task",
        Some("none"),
        Some("grandchild"),
    )
    .await?;
    wait_for_turn_completions(
        &mut notifications,
        &[
            grandchild.child_session_id,
            child.child_session_id,
            root_session_id,
        ],
    )
    .await?;
    for session_id in [
        grandchild.child_session_id,
        child.child_session_id,
        root_session_id,
    ] {
        wait_for_session_idle(&runtime, connection_id, session_id).await?;
    }
    start_turn(&runtime, connection_id, root_session_id, "root task").await?;
    wait_for_session_notification(&mut notifications, "turn/completed", root_session_id).await?;
    wait_for_session_idle(&runtime, connection_id, root_session_id).await?;

    runtime.shutdown().await;

    let rebuilt = build_runtime(
        data_root.path(),
        Arc::new(ScriptedProvider::for_agent_tasks([])) as Arc<dyn ModelProviderSDK>,
    )?;
    rebuilt.refresh_session_index()?;
    let (rebuilt_connection, _) = initialize_connection(&rebuilt).await?;

    let markers = ["reply-alpha", "reply-beta", "reply-gamma"];
    for (session_id, expected_parent, marker) in [
        (root_session_id, None, "reply-gamma"),
        (child.child_session_id, Some(root_session_id), "reply-alpha"),
        (
            grandchild.child_session_id,
            Some(child.child_session_id),
            "reply-beta",
        ),
    ] {
        let session = resume_session(&rebuilt, rebuilt_connection, session_id).await?;
        assert_eq!(session.id, session_id);
        assert_eq!(parent_session_id(&session), expected_parent);
        let items = list_items(&rebuilt, rebuilt_connection, session_id).await?;
        assert!(!items.is_empty(), "resumed session must have history");
        assert!(
            items.iter().all(|item| item.session_id == session_id),
            "resumed history for {session_id} must not contain sibling items"
        );
        assert!(
            items.iter().any(|item| item_contains_text(item, marker)),
            "resumed history for {session_id} must contain its distinct marker {marker}"
        );
        // Completion bodies intentionally quote child output. They must be
        // host-authored inputs from the direct child, never replayed child items.
        let expected_child_output = if session_id == root_session_id {
            Some((child.child_session_id, "reply-alpha"))
        } else if session_id == child.child_session_id {
            Some((grandchild.child_session_id, "reply-beta"))
        } else {
            None
        };
        if let Some((sender, output)) = expected_child_output {
            assert!(items.iter().any(|item| {
                matches!(
                    &item.item,
                    devo_protocol::native::item::Item::UserMessage { origin: Some(origin), .. }
                        if origin.sender == sender.to_string()
                            && origin.source == devo_protocol::native::host_message::HostMessageSource::Agent
                ) && item_contains_text(item, output)
            }), "resumed parent must retain the attributed child completion");
        }
        for other in markers.iter().filter(|other| **other != marker) {
            assert!(
                !items.iter().any(|item| {
                    !matches!(
                        &item.item,
                        devo_protocol::native::item::Item::UserMessage {
                            origin: Some(_),
                            ..
                        }
                    ) && item_contains_text(item, other)
                }),
                "resumed task history for {session_id} must not contain {other}"
            );
        }
    }

    Ok(())
}

#[tokio::test]
async fn deleted_child_is_tombstoned_and_not_resurrected_after_reindex() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child reply"),
        ScriptedProvider::completed("sibling reply"),
        ScriptedProvider::completed("grandchild reply"),
        ScriptedProvider::completed("root reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_turn_completions(
        &mut notifications,
        &[child.child_session_id, root_session_id],
    )
    .await?;
    wait_for_session_idle(&runtime, connection_id, child.child_session_id).await?;
    wait_for_session_idle(&runtime, connection_id, root_session_id).await?;
    let sibling = spawn_child_named(
        &runtime,
        root_session_id,
        "sibling task",
        Some("none"),
        Some("sibling"),
    )
    .await?;
    wait_for_turn_completions(
        &mut notifications,
        &[sibling.child_session_id, root_session_id],
    )
    .await?;
    wait_for_session_idle(&runtime, connection_id, sibling.child_session_id).await?;
    wait_for_session_idle(&runtime, connection_id, root_session_id).await?;
    subscribe_to_child_notifications(&runtime, connection_id, child.child_session_id).await?;
    let grandchild = spawn_child_named(
        &runtime,
        child.child_session_id,
        "grandchild task",
        Some("none"),
        Some("grandchild"),
    )
    .await?;
    wait_for_turn_completions(
        &mut notifications,
        &[
            grandchild.child_session_id,
            child.child_session_id,
            root_session_id,
        ],
    )
    .await?;
    for session_id in [
        grandchild.child_session_id,
        child.child_session_id,
        root_session_id,
    ] {
        wait_for_session_idle(&runtime, connection_id, session_id).await?;
    }
    start_turn(&runtime, connection_id, root_session_id, "root task").await?;
    wait_for_session_notification(&mut notifications, "turn/completed", root_session_id).await?;
    wait_for_session_idle(&runtime, connection_id, root_session_id).await?;

    let journal = root_journal(data_root.path(), root_session_id);
    let child_dir = expected_child_dir(data_root.path(), root_session_id, child.child_session_id);
    let sibling_dir =
        expected_child_dir(data_root.path(), root_session_id, sibling.child_session_id);
    let grandchild_dir = expected_child_dir(
        data_root.path(),
        root_session_id,
        grandchild.child_session_id,
    );
    assert!(journal.is_file());
    assert!(child_dir.is_dir());
    assert!(sibling_dir.is_dir());
    assert!(grandchild_dir.is_dir());
    let before = read_journal(&journal)?;
    let root_before = session_lines(&before, root_session_id);
    let sibling_before = session_lines(&before, sibling.child_session_id);
    assert!(!root_before.is_empty());
    assert!(!sibling_before.is_empty());
    delete_session(&runtime, connection_id, child.child_session_id).await?;

    assert!(
        journal.is_file(),
        "child delete must retain the root journal"
    );
    assert!(!child_dir.exists(), "child artifact dir must be removed");
    assert!(
        !grandchild_dir.exists(),
        "descendant artifact dir must be removed"
    );
    assert!(sibling_dir.is_dir(), "sibling artifact dir must remain");
    let after = read_journal(&journal)?;
    assert_eq!(
        session_lines(&after, root_session_id),
        root_before,
        "child delete must not rewrite root session lines"
    );
    assert_eq!(
        session_lines(&after, sibling.child_session_id),
        sibling_before,
        "child delete must not rewrite sibling session lines"
    );
    assert!(
        data_root
            .path()
            .join("session-artifacts")
            .join(root_session_id.to_string())
            .is_dir(),
        "root artifact tree must remain after child delete"
    );
    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(
        indexed_rollout_path(&db, child.child_session_id)?,
        None,
        "deleted child row"
    );
    assert_eq!(
        indexed_rollout_path(&db, grandchild.child_session_id)?,
        None,
        "deleted grandchild row"
    );
    assert_eq!(
        indexed_rollout_path(&db, root_session_id)?,
        Some(journal.clone())
    );
    assert_eq!(
        indexed_rollout_path(&db, sibling.child_session_id)?,
        Some(journal.clone()),
        "sibling row must remain"
    );

    let lines = read_journal(&journal)?;
    let tombstones: HashSet<SessionId> = lines
        .iter()
        .filter_map(|line| match line {
            RolloutLine::Internal {
                session_id,
                entry: InternalRecord::SessionDeleted { .. },
                ..
            } => Some(*session_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        tombstones,
        [child.child_session_id, grandchild.child_session_id]
            .into_iter()
            .collect()
    );

    assert_eq!(
        list_session_ids(&runtime, connection_id, /*include_children*/ true).await?,
        [root_session_id, sibling.child_session_id]
            .into_iter()
            .collect()
    );
    let remaining_agents = request_agent_list(&runtime, connection_id, root_session_id).await?;
    assert_eq!(
        remaining_agents
            .agents
            .iter()
            .map(|agent| agent.session_id)
            .collect::<HashSet<_>>(),
        [sibling.child_session_id].into_iter().collect(),
        "deleting one child must not remove the sibling"
    );

    let resume_response = rpc(
        &runtime,
        connection_id,
        105,
        "session/resume",
        json!({ "sessionId": child.child_session_id }),
    )
    .await?;
    let error: ErrorResponse = serde_json::from_value(resume_response)?;
    assert_eq!(error.error.code, ProtocolErrorCode::SessionNotFound);

    runtime.shutdown().await;
    let rebuilt = build_runtime(
        data_root.path(),
        Arc::new(ScriptedProvider::for_agent_tasks([])) as Arc<dyn ModelProviderSDK>,
    )?;
    rebuilt.refresh_session_index()?;
    let (rebuilt_connection, _) = initialize_connection(&rebuilt).await?;

    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(
        indexed_rollout_path(&db, child.child_session_id)?,
        None,
        "reindex must not resurrect a tombstoned child"
    );
    assert_eq!(
        indexed_rollout_path(&db, grandchild.child_session_id)?,
        None,
        "reindex must not resurrect a tombstoned grandchild"
    );
    assert_eq!(
        list_session_ids(&rebuilt, rebuilt_connection, /*include_children*/ true).await?,
        [root_session_id, sibling.child_session_id]
            .into_iter()
            .collect()
    );
    let session = resume_session(&rebuilt, rebuilt_connection, root_session_id).await?;
    assert_eq!(session.id, root_session_id);
    let sibling_session =
        resume_session(&rebuilt, rebuilt_connection, sibling.child_session_id).await?;
    assert_eq!(sibling_session.id, sibling.child_session_id);
    assert_eq!(parent_session_id(&sibling_session), Some(root_session_id));

    let resume_response = rpc(
        &rebuilt,
        rebuilt_connection,
        106,
        "session/resume",
        json!({ "sessionId": child.child_session_id }),
    )
    .await?;
    let error: ErrorResponse = serde_json::from_value(resume_response)?;
    assert_eq!(error.error.code, ProtocolErrorCode::SessionNotFound);

    Ok(())
}

#[tokio::test]
async fn root_delete_removes_unhydrated_descendants_and_the_journal() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child reply"),
        ScriptedProvider::completed("grandchild reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;
    subscribe_to_child_notifications(&runtime, connection_id, child.child_session_id).await?;
    let grandchild = spawn_child_named(
        &runtime,
        child.child_session_id,
        "grandchild task",
        Some("none"),
        Some("grandchild"),
    )
    .await?;
    wait_for_session_notification(
        &mut notifications,
        "turn/completed",
        grandchild.child_session_id,
    )
    .await?;

    let journal = root_journal(data_root.path(), root_session_id);
    let artifact_dir = data_root
        .path()
        .join("session-artifacts")
        .join(root_session_id.to_string());
    assert!(journal.is_file());
    assert!(artifact_dir.is_dir());

    runtime.shutdown().await;
    let rebuilt = build_runtime(
        data_root.path(),
        Arc::new(ScriptedProvider::for_agent_tasks([])) as Arc<dyn ModelProviderSDK>,
    )?;
    rebuilt.refresh_session_index()?;
    let (rebuilt_connection, _) = initialize_connection(&rebuilt).await?;
    delete_session(&rebuilt, rebuilt_connection, root_session_id).await?;

    assert!(!journal.exists(), "root journal must be removed");
    assert!(!artifact_dir.exists(), "root artifact tree must be removed");
    assert_eq!(
        list_session_ids(&rebuilt, rebuilt_connection, /*include_children*/ true).await?,
        HashSet::new()
    );

    let db = Database::open(data_root.path().join(DB_FILE))?;
    for session_id in [
        root_session_id,
        child.child_session_id,
        grandchild.child_session_id,
    ] {
        assert_eq!(
            indexed_rollout_path(&db, session_id)?,
            None,
            "root delete must remove DB row for {session_id}"
        );
    }

    Ok(())
}

#[tokio::test]
async fn session_export_filters_to_the_target_session() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child reply"),
        ScriptedProvider::completed("grandchild reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;
    subscribe_to_child_notifications(&runtime, connection_id, child.child_session_id).await?;
    let grandchild = spawn_child_named(
        &runtime,
        child.child_session_id,
        "grandchild task",
        Some("none"),
        Some("grandchild"),
    )
    .await?;
    wait_for_session_notification(
        &mut notifications,
        "turn/completed",
        grandchild.child_session_id,
    )
    .await?;

    let export_dir = data_root.path().join("exports");
    std::fs::create_dir_all(&export_dir)?;
    for (session_id, file_name) in [
        (root_session_id, "root.jsonl"),
        (child.child_session_id, "child.jsonl"),
        (grandchild.child_session_id, "grandchild.jsonl"),
    ] {
        let export_path = export_dir.join(file_name);
        let written_path =
            export_session(&runtime, connection_id, session_id, &export_path).await?;
        assert_eq!(written_path, export_path);
        let lines = read_journal(&written_path)?;
        assert_eq!(
            line_session_ids(&lines),
            [session_id].into_iter().collect(),
            "export for {session_id} must contain only that session"
        );
    }

    Ok(())
}

#[tokio::test]
async fn stale_missing_index_child_resolves_shared_journal() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;

    let journal = root_journal(data_root.path(), root_session_id);
    runtime.shutdown().await;
    clear_index_rollout_path(data_root.path(), child.child_session_id)?;

    let rebuilt = build_runtime(
        data_root.path(),
        Arc::new(ScriptedProvider::for_agent_tasks([])) as Arc<dyn ModelProviderSDK>,
    )?;
    // Deliberately do not refresh the index: resume must use the journal-content
    // fallback for a shared child whose indexed rollout_path is missing.
    let (rebuilt_connection, _) = initialize_connection(&rebuilt).await?;
    let resumed = resume_session(&rebuilt, rebuilt_connection, child.child_session_id).await?;
    assert_eq!(resumed.id, child.child_session_id);
    assert_eq!(parent_session_id(&resumed), Some(root_session_id));
    assert!(
        !list_items(&rebuilt, rebuilt_connection, child.child_session_id)
            .await?
            .is_empty()
    );

    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(
        indexed_rollout_path(&db, child.child_session_id)?,
        Some(journal.clone()),
        "resume must backfill the shared journal path"
    );

    // Same fallback must work for a stale path that exists lexically but points
    // at a missing file, not only for a NULL index value.
    rebuilt.shutdown().await;
    set_index_rollout_path(
        data_root.path(),
        child.child_session_id,
        Some(&data_root.path().join("missing-child.jsonl")),
    )?;
    let stale_runtime = build_runtime(
        data_root.path(),
        Arc::new(ScriptedProvider::for_agent_tasks([])) as Arc<dyn ModelProviderSDK>,
    )?;
    let (stale_connection, _) = initialize_connection(&stale_runtime).await?;
    let resumed = resume_session(&stale_runtime, stale_connection, child.child_session_id).await?;
    assert_eq!(resumed.id, child.child_session_id);
    assert_eq!(parent_session_id(&resumed), Some(root_session_id));
    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(
        indexed_rollout_path(&db, child.child_session_id)?,
        Some(journal),
        "stale-path resume must backfill the shared journal path"
    );
    Ok(())
}

#[tokio::test]
async fn reindex_removes_stale_db_row_for_tombstoned_child() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;

    let journal = root_journal(data_root.path(), root_session_id);
    delete_session(&runtime, connection_id, child.child_session_id).await?;
    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(indexed_rollout_path(&db, child.child_session_id)?, None);
    // Simulate the crash window: tombstone is durable, but a stale DB row remains.
    db.upsert_session(
        stale_index_row(data_root.path(), child.child_session_id, root_session_id),
        Some(journal.as_path()),
    )?;
    assert!(db.get_session_index(&child.child_session_id)?.is_some());

    runtime.refresh_session_index()?;
    assert_eq!(
        indexed_rollout_path(&db, child.child_session_id)?,
        None,
        "reindex must delete a tombstoned session's stale row"
    );
    assert_eq!(
        list_session_ids(&runtime, connection_id, /*include_children*/ true).await?,
        [root_session_id].into_iter().collect()
    );
    Ok(())
}

#[tokio::test]
async fn child_delete_with_missing_index_still_persists_tombstone() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;

    let journal = root_journal(data_root.path(), root_session_id);
    runtime.shutdown().await;
    let db = Database::open(data_root.path().join(DB_FILE))?;
    db.delete_session(&child.child_session_id)?;
    assert_eq!(indexed_rollout_path(&db, child.child_session_id)?, None);

    let rebuilt = build_runtime(
        data_root.path(),
        Arc::new(ScriptedProvider::for_agent_tasks([])) as Arc<dyn ModelProviderSDK>,
    )?;
    let (rebuilt_connection, _) = initialize_connection(&rebuilt).await?;
    delete_session(&rebuilt, rebuilt_connection, child.child_session_id).await?;
    assert!(
        journal.is_file(),
        "child delete must retain the root journal"
    );
    let lines = read_journal(&journal)?;
    let tombstones: HashSet<SessionId> = lines
        .iter()
        .filter_map(|line| match line {
            RolloutLine::Internal {
                session_id,
                entry: InternalRecord::SessionDeleted { .. },
                ..
            } => Some(*session_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        tombstones,
        [child.child_session_id].into_iter().collect(),
        "delete must tombstone even when the index row is missing"
    );

    rebuilt.refresh_session_index()?;
    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(
        indexed_rollout_path(&db, child.child_session_id)?,
        None,
        "missing-index delete must not resurrect after reindex"
    );
    assert_eq!(
        list_session_ids(&rebuilt, rebuilt_connection, /*include_children*/ true).await?,
        [root_session_id].into_iter().collect()
    );
    Ok(())
}

#[tokio::test]
async fn child_cwd_update_preserves_attribution_and_shared_size_is_not_child_size() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;

    let listed = list_sessions(&runtime, connection_id, /*include_children*/ true).await?;
    let child_listed = listed
        .iter()
        .find(|session| session.id == child.child_session_id)
        .expect("child listed");
    // G8 contract: the root reports the whole shared journal size; a shared
    // child has no session-owned file, so the optional field is absent. `None`
    // is protocol-legal (`Option<u64>` with skip-none).
    assert_eq!(
        child_listed.transcript_size_bytes, None,
        "a shared child must not report the whole root journal size"
    );
    let root_listed = listed
        .iter()
        .find(|session| session.id == root_session_id)
        .expect("root listed");
    assert!(
        root_listed
            .transcript_size_bytes
            .is_some_and(|size| size > 0),
        "root may report its journal size"
    );

    let changed_cwd = data_root.path().join("changed-cwd");
    std::fs::create_dir_all(&changed_cwd)?;
    let response = rpc(
        &runtime,
        connection_id,
        107,
        "session/cwd/change",
        json!({ "sessionId": child.child_session_id, "cwd": changed_cwd }),
    )
    .await?;
    ensure_success(&response)?;

    runtime.shutdown().await;
    let db = Database::open(data_root.path().join(DB_FILE))?;
    db.delete_session(&child.child_session_id)?;
    let rebuilt = build_runtime(
        data_root.path(),
        Arc::new(ScriptedProvider::for_agent_tasks([])) as Arc<dyn ModelProviderSDK>,
    )?;
    rebuilt.refresh_session_index()?;
    let (rebuilt_connection, _) = initialize_connection(&rebuilt).await?;

    let db = Database::open(data_root.path().join(DB_FILE))?;
    let child_index = db
        .get_session_index(&child.child_session_id)?
        .expect("child index after reindex");
    assert_eq!(child_index.session.parent_session_id, Some(root_session_id));
    assert_eq!(
        child_index.session.agent_path.as_deref(),
        Some("root/child"),
        "cwd SessionMeta must preserve child attribution extras"
    );
    let default_list = list_sessions(
        &rebuilt,
        rebuilt_connection,
        /*include_children*/ false,
    )
    .await?;
    assert_eq!(
        default_list
            .iter()
            .map(|session| session.id)
            .collect::<HashSet<_>>(),
        [root_session_id].into_iter().collect(),
        "a child must not become a root after cwd reindex"
    );
    let resumed = resume_session(&rebuilt, rebuilt_connection, child.child_session_id).await?;
    assert_eq!(resumed.id, child.child_session_id);
    assert_eq!(parent_session_id(&resumed), Some(root_session_id));
    let agents = request_agent_list(&rebuilt, rebuilt_connection, root_session_id).await?;
    let child_agent = agents
        .agents
        .iter()
        .find(|agent| agent.session_id == child.child_session_id)
        .expect("child agent after resume");
    assert_eq!(child_agent.agent_path, "root/child");
    assert_eq!(child_agent.agent_nickname, "child");
    Ok(())
}

#[tokio::test]
async fn child_rollback_preview_counts_only_child_turns() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child first"),
        ScriptedProvider::completed("child second"),
        ScriptedProvider::completed("root reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child first",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_turn_completions(
        &mut notifications,
        &[child.child_session_id, root_session_id],
    )
    .await?;
    wait_for_session_idle(&runtime, connection_id, child.child_session_id).await?;
    wait_for_session_idle(&runtime, connection_id, root_session_id).await?;
    start_turn(
        &runtime,
        connection_id,
        child.child_session_id,
        "child second",
    )
    .await?;
    wait_for_turn_completions(
        &mut notifications,
        &[child.child_session_id, root_session_id],
    )
    .await?;
    wait_for_session_idle(&runtime, connection_id, child.child_session_id).await?;
    wait_for_session_idle(&runtime, connection_id, root_session_id).await?;
    start_turn(&runtime, connection_id, root_session_id, "root task").await?;
    wait_for_session_notification(&mut notifications, "turn/completed", root_session_id).await?;
    wait_for_session_idle(&runtime, connection_id, root_session_id).await?;

    let response = rpc(
        &runtime,
        connection_id,
        108,
        "session/rollback/preview",
        json!({
            "sessionId": child.child_session_id,
            "userTurnIndex": 0,
            "mode": "beforeUserTurn",
        }),
    )
    .await?;
    ensure_success(&response)?;
    let plan: RestorePlan = serde_json::from_value(response["result"].clone())?;
    assert_eq!(
        plan.dropped_turn_count, 2,
        "child rollback must count only the child's two turns, not the root turn"
    );
    Ok(())
}

#[tokio::test]
async fn live_root_delete_removes_shared_journal_and_reindex_does_not_resurrect() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;

    let journal = root_journal(data_root.path(), root_session_id);
    let artifact_dir = data_root
        .path()
        .join("session-artifacts")
        .join(root_session_id.to_string());
    assert!(journal.is_file());
    assert!(artifact_dir.is_dir());

    // Root and child actors are both loaded here. This is the live-root delete
    // path that must physically unlink the shared journal, not only remove DB
    // rows.
    delete_session(&runtime, connection_id, root_session_id).await?;
    assert!(
        !journal.exists(),
        "live root delete must unlink the journal"
    );
    assert!(journal_files(data_root.path())?.is_empty());
    assert!(!artifact_dir.exists());

    runtime.shutdown().await;
    let rebuilt = build_runtime(
        data_root.path(),
        Arc::new(ScriptedProvider::for_agent_tasks([])) as Arc<dyn ModelProviderSDK>,
    )?;
    rebuilt.refresh_session_index()?;
    let (rebuilt_connection, _) = initialize_connection(&rebuilt).await?;

    assert!(
        list_sessions(&rebuilt, rebuilt_connection, /*include_children*/ true)
            .await?
            .is_empty(),
        "deleted root must not resurrect from reindex"
    );
    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(indexed_rollout_path(&db, root_session_id)?, None);
    assert_eq!(indexed_rollout_path(&db, child.child_session_id)?, None);
    let resume_response = rpc(
        &rebuilt,
        rebuilt_connection,
        109,
        "session/resume",
        json!({ "sessionId": root_session_id }),
    )
    .await?;
    let error: ErrorResponse = serde_json::from_value(resume_response)?;
    assert_eq!(error.error.code, ProtocolErrorCode::SessionNotFound);
    assert!(
        journal_files(data_root.path())?.is_empty(),
        "reindex/resume must not recreate the deleted root journal"
    );

    // Repeated delete is safe and must not leave a closed-but-present journal.
    delete_session(&rebuilt, rebuilt_connection, root_session_id).await?;
    assert!(!journal.exists());
    assert!(journal_files(data_root.path())?.is_empty());
    Ok(())
}

#[tokio::test]
async fn wrong_existing_index_path_delete_and_resume_resolve_true_journal() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let wrong_root_id = create_root_session(
        &runtime,
        connection_id,
        data_root.path(),
        "wrong-index-root",
    )
    .await?;
    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;

    let true_journal = root_journal(data_root.path(), root_session_id);
    let wrong_journal = root_journal(data_root.path(), wrong_root_id);
    assert!(true_journal.is_file());
    assert!(wrong_journal.is_file());
    let wrong_before = std::fs::read(&wrong_journal)?;

    // Resume with a wrong path that exists: membership must win and the index
    // must be repaired to the true root journal.
    runtime.shutdown().await;
    set_index_rollout_path(
        data_root.path(),
        child.child_session_id,
        Some(&wrong_journal),
    )?;
    let rebuilt = build_runtime(
        data_root.path(),
        Arc::new(ScriptedProvider::for_agent_tasks([])) as Arc<dyn ModelProviderSDK>,
    )?;
    let (rebuilt_connection, _) = initialize_connection(&rebuilt).await?;
    let resumed = resume_session(&rebuilt, rebuilt_connection, child.child_session_id).await?;
    assert_eq!(resumed.id, child.child_session_id);
    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(
        indexed_rollout_path(&db, child.child_session_id)?,
        Some(true_journal.clone()),
        "resume must backfill the true shared journal, not the wrong existing one"
    );

    // Delete with the wrong existing path again. The tombstone must land in the
    // true root journal; the wrong journal must be byte-identical.
    set_index_rollout_path(
        data_root.path(),
        child.child_session_id,
        Some(&wrong_journal),
    )?;
    delete_session(&rebuilt, rebuilt_connection, child.child_session_id).await?;
    let true_lines = read_journal(&true_journal)?;
    let tombstones: HashSet<SessionId> = true_lines
        .iter()
        .filter_map(|line| match line {
            RolloutLine::Internal {
                session_id,
                entry: InternalRecord::SessionDeleted { .. },
                ..
            } => Some(*session_id),
            _ => None,
        })
        .collect();
    assert_eq!(tombstones, [child.child_session_id].into_iter().collect());
    assert_eq!(
        std::fs::read(&wrong_journal)?,
        wrong_before,
        "wrong existing journal must be untouched"
    );
    assert_eq!(
        indexed_rollout_path(&db, child.child_session_id)?,
        None,
        "deleted child row"
    );
    Ok(())
}

#[tokio::test]
async fn retry_root_delete_cleans_orphan_child_row_and_artifact() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;
    let journal = root_journal(data_root.path(), root_session_id);
    let child_dir = expected_child_dir(data_root.path(), root_session_id, child.child_session_id);
    assert!(journal.is_file());
    assert!(child_dir.is_dir());

    delete_session(&runtime, connection_id, root_session_id).await?;
    assert!(!journal.exists());

    // Simulate a crash after root unlink but before child DB/artifact cleanup.
    let db = Database::open(data_root.path().join(DB_FILE))?;
    db.upsert_session(
        stale_index_row(data_root.path(), child.child_session_id, root_session_id),
        Some(journal.as_path()),
    )?;
    std::fs::create_dir_all(&child_dir)?;
    assert!(db.get_session_index(&child.child_session_id)?.is_some());
    assert!(child_dir.is_dir());

    // Retry root delete: there is no root DB row/journal, but orphan child
    // evidence remains. It must finish cleanup instead of returning Ok(empty).
    delete_session(&runtime, connection_id, root_session_id).await?;
    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(
        indexed_rollout_path(&db, child.child_session_id)?,
        None,
        "retry must delete the orphan child row"
    );
    assert!(
        !child_dir.exists(),
        "retry must delete orphan child artifact dir"
    );
    assert!(
        !journal.exists(),
        "retry must not recreate the root journal"
    );
    assert_eq!(
        list_session_ids(&runtime, connection_id, /*include_children*/ true).await?,
        HashSet::new()
    );
    Ok(())
}

#[tokio::test]
async fn retry_delete_with_missing_target_journal_cleans_db_and_artifact() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;

    let journal = root_journal(data_root.path(), root_session_id);
    let child_dir = expected_child_dir(data_root.path(), root_session_id, child.child_session_id);
    let root_artifact_dir = data_root
        .path()
        .join("session-artifacts")
        .join(root_session_id.to_string());
    runtime.shutdown().await;
    std::fs::remove_file(&journal)?;

    // Child DB row and artifact remain, but the target journal is gone. A retry
    // must finish cleanup instead of failing with "journal path is unknown".
    let rebuilt = build_runtime(
        data_root.path(),
        Arc::new(ScriptedProvider::for_agent_tasks([])) as Arc<dyn ModelProviderSDK>,
    )?;
    let (rebuilt_connection, _) = initialize_connection(&rebuilt).await?;
    delete_session(&rebuilt, rebuilt_connection, child.child_session_id).await?;

    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(
        indexed_rollout_path(&db, child.child_session_id)?,
        None,
        "retry must delete the child DB row"
    );
    assert!(
        !child_dir.exists(),
        "retry must delete the child artifact dir"
    );
    assert!(
        root_artifact_dir.is_dir(),
        "child retry must not remove the root artifact tree"
    );
    assert!(
        !journal.exists(),
        "retry must not recreate the root journal"
    );
    Ok(())
}

#[tokio::test]
async fn delete_with_damaged_journal_fails_closed() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;
    let journal = root_journal(data_root.path(), root_session_id);
    let root_artifact_dir = data_root
        .path()
        .join("session-artifacts")
        .join(root_session_id.to_string());
    runtime.shutdown().await;
    let damaged = b"not-json-damaged-journal\n";
    std::fs::write(&journal, damaged)?;

    let rebuilt = build_runtime(
        data_root.path(),
        Arc::new(ScriptedProvider::for_agent_tasks([])) as Arc<dyn ModelProviderSDK>,
    )?;
    let (rebuilt_connection, _) = initialize_connection(&rebuilt).await?;
    let response = rpc(
        &rebuilt,
        rebuilt_connection,
        113,
        "session/delete",
        json!({ "sessionId": root_session_id }),
    )
    .await?;
    assert!(
        response.get("error").is_some(),
        "deleting an unreadable journal must fail closed, got: {response}"
    );

    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert!(
        db.get_session_index(&root_session_id)?.is_some(),
        "failed delete must leave the root DB row"
    );
    assert!(
        db.get_session_index(&child.child_session_id)?.is_some(),
        "failed delete must leave the child DB row"
    );
    assert!(
        root_artifact_dir.is_dir(),
        "failed delete must keep artifacts"
    );
    assert_eq!(
        std::fs::read(&journal)?,
        damaged,
        "failed delete must not append a tombstone to a damaged journal"
    );
    Ok(())
}

#[tokio::test]
async fn user_fork_is_not_an_agent_child_and_owns_its_child_tree() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("fork child reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let source_root_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let fork_id = fork_session(&runtime, connection_id, source_root_id).await?;
    let fork_journal = root_journal(data_root.path(), fork_id);
    let source_journal = root_journal(data_root.path(), source_root_id);
    assert!(fork_journal.is_file());
    assert!(source_journal.is_file());

    let child = spawn_child_named(
        &runtime,
        fork_id,
        "fork child task",
        Some("none"),
        Some("fork-child"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;
    let child_dir = expected_child_dir(data_root.path(), fork_id, child.child_session_id);
    assert!(child_dir.is_dir());
    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(
        indexed_rollout_path(&db, child.child_session_id)?,
        Some(fork_journal.clone()),
        "agent child under a user fork must use the fork as its lifecycle root"
    );
    let fork_index = db.get_session_index(&fork_id)?.expect("fork index");
    assert_eq!(fork_index.session.fork_from_id, Some(source_root_id));

    // Deleting the source root must not traverse fork_from_id as an agent edge.
    delete_session(&runtime, connection_id, source_root_id).await?;
    assert!(
        !source_journal.exists(),
        "source root journal must be deleted"
    );
    assert!(fork_journal.is_file(), "user fork journal must remain");
    assert!(child_dir.is_dir(), "fork child artifact must remain");
    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(
        indexed_rollout_path(&db, fork_id)?,
        Some(fork_journal.clone())
    );
    assert_eq!(
        indexed_rollout_path(&db, child.child_session_id)?,
        Some(fork_journal.clone())
    );

    // Deleting the fork root cleans its own agent child and journal only.
    delete_session(&runtime, connection_id, fork_id).await?;
    assert!(!fork_journal.exists(), "fork journal must be deleted");
    assert!(!child_dir.exists(), "fork child artifact must be deleted");
    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(indexed_rollout_path(&db, fork_id)?, None);
    assert_eq!(indexed_rollout_path(&db, child.child_session_id)?, None);
    Ok(())
}

#[tokio::test]
async fn wrong_index_missing_journal_preserves_unrelated_artifact_sentinel() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::for_agent_tasks([
        ScriptedProvider::completed("child reply"),
    ]));
    let runtime = build_runtime(data_root.path(), provider)?;
    let (connection_id, mut notifications) = initialize_connection(&runtime).await?;
    let root_session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let child = spawn_child_named(
        &runtime,
        root_session_id,
        "child task",
        Some("none"),
        Some("child"),
    )
    .await?;
    wait_for_session_notification(&mut notifications, "turn/completed", child.child_session_id)
        .await?;

    let journal = root_journal(data_root.path(), root_session_id);
    let child_dir = expected_child_dir(data_root.path(), root_session_id, child.child_session_id);
    let unrelated_dir = data_root
        .path()
        .join("session-artifacts")
        .join(root_session_id.to_string())
        .join("agents")
        .join("unrelated-sentinel");
    std::fs::create_dir_all(&unrelated_dir)?;
    let sentinel = unrelated_dir.join("keep.txt");
    std::fs::write(&sentinel, b"keep")?;
    let wrong_missing_path = unrelated_dir.join(format!("{}.jsonl", child.child_session_id));
    assert!(!wrong_missing_path.exists());

    runtime.shutdown().await;
    std::fs::remove_file(&journal)?;
    set_index_rollout_path(
        data_root.path(),
        child.child_session_id,
        Some(&wrong_missing_path),
    )?;
    let rebuilt = build_runtime(
        data_root.path(),
        Arc::new(ScriptedProvider::for_agent_tasks([])) as Arc<dyn ModelProviderSDK>,
    )?;
    let (rebuilt_connection, _) = initialize_connection(&rebuilt).await?;
    delete_session(&rebuilt, rebuilt_connection, child.child_session_id).await?;

    assert!(
        sentinel.is_file(),
        "unrelated artifact sentinel must remain after child delete"
    );
    assert!(
        !child_dir.exists(),
        "proven child artifact dir should be removed when lifecycle root is proven"
    );
    let db = Database::open(data_root.path().join(DB_FILE))?;
    assert_eq!(
        indexed_rollout_path(&db, child.child_session_id)?,
        None,
        "child DB row must be removed"
    );
    Ok(())
}
