use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::Context;
use anyhow::Result;
use chrono::Utc;
use tokio::sync::Mutex;

use devo_core::CompactionSnapshotLine;
use devo_core::ItemId;
use devo_core::ItemRecord;
use devo_core::LegacyRolloutLine;
use devo_core::Message;
use devo_core::MessageEditRecordedRecord;
use devo_core::RolloutLineReadError;
use devo_core::SessionContext;
use devo_core::SessionId;
use devo_core::SessionRecord;
use devo_core::SessionRollbackLine;
use devo_core::SessionSettingsField;
use devo_core::SessionTitleFinalSource;
use devo_core::SessionTitleState;
use devo_core::TurnId;
use devo_core::TurnItem;
use devo_core::TurnRecord;
use devo_core::TurnStatus;
use devo_core::TurnSupersededRecord;
use devo_core::TurnWorkspaceChangeRecordedLine;
use devo_core::TurnWorkspaceChangeRecordedRecord;
use devo_core::TurnWorkspaceCheckpointRecordedLine;
use devo_core::TurnWorkspaceCheckpointRecordedRecord;
use devo_core::TurnWorkspaceRestoreCompletedLine;
use devo_core::TurnWorkspaceRestoreCompletedRecord;
use devo_core::TurnWorkspaceRestoreStartedLine;
use devo_core::TurnWorkspaceRestoreStartedRecord;
use devo_core::Worklog;
use devo_core::legacy_compaction_line_from_native;
use devo_core::legacy_lines_from_internal;
use devo_core::legacy_rollback_line_from_native;
use devo_core::legacy_title_line_from_native;
use devo_core::parse_rollout_line;
use devo_core::rollout::InternalRecord;
use devo_core::rollout::SessionPersistenceExtras;
use devo_core::rollout::TurnPersistenceExtras;
use devo_core::rollout::{ROLLOUT_FORMAT_VERSION, RolloutLine};
use devo_core::rollout_write_state::RolloutWriteState;
use devo_core::session_record_from_native;
use devo_core::{EVENT_SCHEMA_VERSION, events_from_rollout_line, source_fact_id};
use devo_protocol::native::event::{EventEnvelope, EventMeta};
use devo_protocol::native::ids::EventId;
use devo_protocol::native::ids::{ItemId as NativeItemId, TurnId as NativeTurnId};
use devo_protocol::native::item::Item as NativeItem;
use devo_protocol::native::turn::TurnKind;

use crate::db::{Database, NewEventLogRow};
use crate::execution::PersistedTurnItem;
use crate::execution::RuntimeSession;
use crate::execution::ServerRuntimeDependencies;
use crate::persisted_native_item::PersistedNativeItem;
use crate::persisted_native_item::prompt_visible_persisted_item;
use crate::prompt_from_native_item::apply_native_item;
use crate::prompt_from_native_item::apply_prompt_native_item;
use crate::replay_hydrate::{ReplayedSession, ReplayedTurn};
use devo_protocol::native::item::ItemEnvelope;

/// Rollout path + persistence extras invented at session create origin.
///
/// Native [`devo_protocol::native::session::Session`] is invented by the
/// caller (create / fork / agent-spawn); this type never builds a
/// [`SessionRecord`].
#[derive(Debug, Clone)]
pub(crate) struct InventedSessionPersistence {
    pub rollout_path: PathBuf,
    pub extras: SessionPersistenceExtras,
}

/// Invents [`SessionPersistenceExtras`] at origin (not from a SessionRecord).
pub(crate) fn invent_session_persistence_extras() -> SessionPersistenceExtras {
    SessionPersistenceExtras {
        session_context: None,
        cli_version: env!("CARGO_PKG_VERSION").into(),
        source: "cli".into(),
        collaboration_mode: None,
        permission_preset: None,
        agent_path: None,
        agent_nickname: None,
        kernel_snapshot_path: None,
    }
}

/// Owns canonical append-only rollout persistence rooted at the server data directory.
pub(crate) struct RolloutStore {
    /// Root data directory that contains `sessions/` and `session-artifacts/`.
    data_root: PathBuf,
    /// Strong per-store cache of the process-global journal engines. Clones
    /// share it; distinct stores converge through the global weak registry.
    journals: Arc<StdMutex<HashMap<PathBuf, Arc<JournalShared>>>>,
    /// Delivery-log sink (08 §5/§7): after each fsynced append, derived
    /// events are projected into the SQLite `event_log` (best effort; the
    /// startup reconciler backfills anything missed). `None` in tests that
    /// do not exercise the event log.
    event_log: Option<Arc<Database>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JournalVisibility {
    /// Only sessions whose latest lifecycle marker is not a tombstone.
    Live,
    /// Include sessions whose latest marker is a tombstone (delete cleanup).
    IncludeDeleted,
}

/// One process-wide engine per canonical physical journal. The state mutex is
/// both the in-process append serialization and the per-session state owner.
struct JournalShared {
    state: StdMutex<JournalState>,
}

#[derive(Default)]
struct JournalState {
    write: Option<WritePathState>,
    /// Set before a journal is unlinked. Any later append/import to this
    /// canonical path fails instead of recreating the deleted root.
    closed: bool,
}

static JOURNAL_REGISTRY: std::sync::OnceLock<
    StdMutex<HashMap<PathBuf, std::sync::Weak<JournalShared>>>,
> = std::sync::OnceLock::new();

fn journal_registry() -> &'static StdMutex<HashMap<PathBuf, std::sync::Weak<JournalShared>>> {
    JOURNAL_REGISTRY.get_or_init(|| StdMutex::new(HashMap::new()))
}

impl RolloutStore {
    /// True when this canonical journal has been durably closed/deleted.
    pub(crate) fn journal_is_closed(&self, rollout_path: &Path) -> bool {
        let Ok(key) = crate::journal_path::canonical_journal_key(rollout_path) else {
            return false;
        };
        let shared = {
            let local = self
                .journals
                .lock()
                .expect("rollout journal cache poisoned");
            local.get(&key).cloned()
        }
        .or_else(|| {
            journal_registry()
                .lock()
                .expect("rollout journal registry poisoned")
                .get(&key)
                .and_then(std::sync::Weak::upgrade)
        });
        shared.is_some_and(|shared| {
            shared
                .state
                .lock()
                .expect("rollout journal state mutex poisoned")
                .closed
        })
    }

    fn shared_journal(&self, rollout_path: &Path) -> Result<Arc<JournalShared>> {
        let key = crate::journal_path::canonical_journal_key(rollout_path)?;
        {
            let local = self
                .journals
                .lock()
                .expect("rollout journal cache poisoned");
            if let Some(shared) = local.get(&key) {
                return Ok(Arc::clone(shared));
            }
        }
        let shared = {
            let mut registry = journal_registry()
                .lock()
                .expect("rollout journal registry poisoned");
            registry.retain(|_, weak| weak.strong_count() > 0);
            match registry.get(&key).and_then(std::sync::Weak::upgrade) {
                Some(shared) => shared,
                None => {
                    let shared = Arc::new(JournalShared {
                        state: StdMutex::new(JournalState::default()),
                    });
                    registry.insert(key.clone(), Arc::downgrade(&shared));
                    shared
                }
            }
        };
        let mut local = self
            .journals
            .lock()
            .expect("rollout journal cache poisoned");
        Ok(Arc::clone(local.entry(key).or_insert(shared)))
    }
}

/// Per-file write-path state: the physical line index plus one
/// [`RolloutWriteState`] per logical session in the journal. Item `seq`,
/// settings epochs, approval folds, and cwd are session-scoped; the line index
/// and the event-log watermark remain file-scoped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JournalWriteMode {
    /// Explicit creation of a new root/fork journal.
    CreateIfMissing,
    /// Ordinary append/settings/tombstone: never recreate a missing journal.
    ExistingOnly,
}

pub(crate) struct WritePathState {
    sessions: HashMap<SessionId, RolloutWriteState>,
    /// Last merged persistence extras per session. A later `SessionMeta` that
    /// passes `None` (cwd/ACP updates) must not erase agent path/nickname/mode.
    session_extras: HashMap<SessionId, SessionPersistenceExtras>,
    next_line_index: u64,
    /// Physical identity/length last observed/committed. A mismatch means an
    /// external append/truncate/recreate happened; the state rehydrates.
    observed: JournalStamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct JournalStamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(windows)]
    creation_time: u64,
    #[cfg(windows)]
    last_write_time: u64,
}

impl JournalStamp {
    fn missing() -> Self {
        Self {
            len: 0,
            modified: None,
            #[cfg(unix)]
            dev: 0,
            #[cfg(unix)]
            ino: 0,
            #[cfg(windows)]
            creation_time: 0,
            #[cfg(windows)]
            last_write_time: 0,
        }
    }
}

fn journal_stamp(path: &Path) -> JournalStamp {
    let Ok(metadata) = std::fs::metadata(path) else {
        return JournalStamp::missing();
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        JournalStamp {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            dev: metadata.dev(),
            ino: metadata.ino(),
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        JournalStamp {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            creation_time: metadata.creation_time(),
            last_write_time: metadata.last_write_time(),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        JournalStamp {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

impl WritePathState {
    fn session_state_mut(&mut self, session_id: SessionId) -> &mut RolloutWriteState {
        self.sessions.entry(session_id).or_default()
    }
}

impl std::fmt::Debug for RolloutStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RolloutStore")
            .field("data_root", &self.data_root)
            .finish()
    }
}

impl Clone for RolloutStore {
    fn clone(&self) -> Self {
        Self {
            data_root: self.data_root.clone(),
            journals: Arc::clone(&self.journals),
            event_log: self.event_log.as_ref().map(Arc::clone),
        }
    }
}

impl RolloutStore {
    /// Creates a rollout store rooted at the supplied server home directory.
    pub(crate) fn new(data_root: PathBuf, event_log: Option<Arc<Database>>) -> Self {
        Self {
            data_root,
            journals: Arc::new(StdMutex::new(HashMap::new())),
            event_log,
        }
    }

    /// Allocates the on-disk rollout path for a new **root** (or fork) session.
    ///
    /// Layout: `sessions/<session_id>.jsonl`.
    pub(crate) fn allocate_rollout_path(
        &self,
        session_id: &devo_protocol::native::ids::SessionId,
    ) -> PathBuf {
        self.root_rollout_path(session_id)
    }

    /// Invents rollout path + [`SessionPersistenceExtras`] at origin for a root/fork.
    ///
    /// Live create / fork invent Native [`Session`] (and
    /// [`crate::runtime_session_summary::RuntimeSessionSummary`]) separately,
    /// then persist via [`Self::append_session_meta_at`]. Do not build a
    /// [`SessionRecord`] first.
    pub(crate) fn invent_session_persistence(
        &self,
        session_id: &devo_protocol::native::ids::SessionId,
    ) -> InventedSessionPersistence {
        InventedSessionPersistence {
            rollout_path: self.allocate_rollout_path(session_id),
            extras: invent_session_persistence_extras(),
        }
    }

    /// Appends a session header to an already-established journal. Ordinary
    /// metadata/archive updates must never recreate a missing root.
    pub(crate) fn append_session_meta_at(
        &self,
        rollout_path: &Path,
        session: &devo_protocol::native::session::Session,
        extras: Option<devo_core::SessionPersistenceExtras>,
    ) -> Result<()> {
        self.append_session_meta_with_mode(
            rollout_path,
            session,
            extras,
            JournalWriteMode::ExistingOnly,
        )
    }

    /// Explicit root/fork/journal creation: allowed to create the physical file.
    pub(crate) fn append_session_meta_create_at(
        &self,
        rollout_path: &Path,
        session: &devo_protocol::native::session::Session,
        extras: Option<devo_core::SessionPersistenceExtras>,
    ) -> Result<()> {
        self.append_session_meta_with_mode(
            rollout_path,
            session,
            extras,
            JournalWriteMode::CreateIfMissing,
        )
    }

    /// Persist inherited settings before the first metadata header in one locked
    /// create batch. A crash leaving only a prefix cannot expose a root with
    /// relaxed defaults: either its settings precede its header or no header exists.
    pub(crate) fn append_seeded_session_meta_create_at(
        &self,
        rollout_path: &Path,
        session: &devo_protocol::native::session::Session,
        extras: devo_core::SessionPersistenceExtras,
        settings: &[(SessionSettingsField, serde_json::Value)],
    ) -> Result<()> {
        let session_id = session.id;
        self.append_rollout_lines_with(
            rollout_path,
            session_id,
            JournalWriteMode::CreateIfMissing,
            |state| {
                let mut lines = Vec::with_capacity(settings.len() + 1);
                let session_state = state.session_state_mut(session_id);
                for (field, value) in settings {
                    let epoch = session_state.allocate_settings_epoch();
                    lines.push(devo_core::settings_line(
                        Utc::now(),
                        session_id,
                        *field,
                        value.clone(),
                        epoch,
                    ));
                }
                let merged =
                    merge_session_extras(state.session_extras.get(&session_id), Some(&extras));
                state.session_extras.insert(session_id, merged.clone());
                lines.push(devo_core::session_line(
                    session.clone(),
                    Some(merged),
                    Utc::now(),
                ));
                Ok(lines)
            },
        )
    }

    fn append_session_meta_with_mode(
        &self,
        rollout_path: &Path,
        session: &devo_protocol::native::session::Session,
        extras: Option<devo_core::SessionPersistenceExtras>,
        mode: JournalWriteMode,
    ) -> Result<()> {
        let session_id = session.id;
        self.append_rollout_lines_with(rollout_path, session_id, mode, |state| {
            let merged =
                merge_session_extras(state.session_extras.get(&session_id), extras.as_ref());
            state.session_extras.insert(session_id, merged.clone());
            Ok(vec![devo_core::session_line(
                session.clone(),
                Some(merged),
                Utc::now(),
            )])
        })
    }

    /// Test / migrate boundary: SessionRecord → Native SessionMeta append.
    /// Live create paths must use [`Self::append_session_meta_at`].
    #[cfg(test)]
    pub(crate) fn append_session_meta(&self, record: &SessionRecord) -> Result<()> {
        self.append_session_meta_create_at(
            &record.rollout_path,
            &devo_core::native_session_from_record(record)
                .context("build native session from record")?,
            Some(devo_core::session_persistence_extras_from_record(record)),
        )
    }

    /// Test / index / migrate factory only — not for live create/fork/spawn.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_session_record(
        &self,
        id: SessionId,
        created_at: chrono::DateTime<Utc>,
        cwd: PathBuf,
        additional_directories: Vec<PathBuf>,
        title: Option<String>,
        model: Option<String>,
        model_binding_id: Option<String>,
        reasoning_effort_selection: Option<String>,
        model_provider: String,
        parent_session_id: Option<SessionId>,
    ) -> SessionRecord {
        self.create_session_record_with_fork(
            id,
            created_at,
            cwd,
            additional_directories,
            title,
            model,
            model_binding_id,
            reasoning_effort_selection,
            model_provider,
            parent_session_id,
            /*fork_from_id*/ None,
            /*fork_at_turn_id*/ None,
        )
    }

    /// Test / index / migrate factory only — not for live create/fork/spawn.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_session_record_with_fork(
        &self,
        id: SessionId,
        created_at: chrono::DateTime<Utc>,
        cwd: PathBuf,
        additional_directories: Vec<PathBuf>,
        title: Option<String>,
        model: Option<String>,
        model_binding_id: Option<String>,
        reasoning_effort_selection: Option<String>,
        model_provider: String,
        parent_session_id: Option<SessionId>,
        fork_from_id: Option<SessionId>,
        fork_at_turn_id: Option<TurnId>,
    ) -> SessionRecord {
        let native_session_id = id;
        let rollout_path = self.root_rollout_path(&native_session_id);
        let title_state = title
            .as_ref()
            .map(|_| SessionTitleState::Final(SessionTitleFinalSource::ExplicitCreate))
            .unwrap_or(SessionTitleState::Unset);
        SessionRecord {
            id,
            rollout_path,
            created_at,
            updated_at: created_at,
            last_activity_at: Some(created_at),
            source: "cli".into(),
            agent_nickname: None,
            agent_role: None,
            agent_path: None,
            model_provider,
            model,
            model_binding_id,
            reasoning_effort_selection,
            cwd,
            additional_directories,
            cli_version: env!("CARGO_PKG_VERSION").into(),
            title,
            title_state,
            sandbox_policy: "workspace-write".into(),
            approval_mode: "on-request".into(),
            effective_context_window: None,
            tokens_used: 0,
            first_user_message: None,
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
            parent_session_id,
            fork_from_id,
            fork_at_turn_id,
            session_context: None,
            latest_turn_context: None,
            collaboration_mode: None,
            permission_preset: None,
            schema_version: 2,
        }
    }

    /// Appends a settings patch as one locked rollout batch. Keeping all
    /// changed fields under the same file lock prevents concurrent metadata
    /// updates from interleaving their field lines and splitting one logical
    /// patch across settings epochs.
    ///
    /// `settings_epoch` is a caller placeholder; the per-file write state
    /// allocates the authoritative epoch (same as the former projector).
    pub(crate) fn append_session_settings_batch_at(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        _settings_epoch: u64,
        settings: &[(SessionSettingsField, serde_json::Value)],
    ) -> Result<()> {
        let native_session_id =
            devo_core::native_session_id(session_id).context("native session id for settings")?;
        self.append_rollout_lines_with(
            rollout_path,
            native_session_id,
            JournalWriteMode::ExistingOnly,
            |state| {
                let session_state = state.session_state_mut(native_session_id);
                let mut lines = Vec::with_capacity(settings.len());
                for (field, value) in settings {
                    let epoch = session_state.allocate_settings_epoch();
                    lines.push(devo_core::settings_line(
                        Utc::now(),
                        native_session_id,
                        *field,
                        value.clone(),
                        epoch,
                    ));
                }
                Ok(lines)
            },
        )
    }

    /// Appends one turn line (path-first Native).
    pub(crate) fn append_turn_at(
        &self,
        rollout_path: &Path,
        turn: &devo_protocol::native::turn::Turn,
        extras: Option<TurnPersistenceExtras>,
    ) -> Result<()> {
        self.append_rollout_lines(
            rollout_path,
            vec![devo_core::turn_line(turn.clone(), extras, Utc::now())],
        )
    }

    /// Thin SessionRecord + TurnRecord wrapper for migrate / tests that only hold records.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn append_turn(&self, record: &SessionRecord, turn: TurnRecord) -> Result<()> {
        self.append_turn_at(
            &record.rollout_path,
            &devo_core::canonical_turn_from_record(&turn)
                .context("build native turn from record")?,
            Some(devo_core::turn_persistence_extras_from_record(&turn)),
        )
    }

    /// Appends packed legacy [`ItemRecord`] payloads (path-first). Expands under
    /// the write lock via migrate/test-only [`RolloutWriteState::item_lines_from_record`].
    ///
    /// **Not the live or fork write path** — those use
    /// [`Self::append_canonical_item_at`]. Remaining callers: legacy migrate and
    /// server tests/fixtures that still feed packed records.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn append_item_at(&self, rollout_path: &Path, item: ItemRecord) -> Result<()> {
        let session_id = item.session_id;
        self.append_rollout_lines_with(
            rollout_path,
            session_id,
            JournalWriteMode::ExistingOnly,
            |state| {
                state
                    .session_state_mut(session_id)
                    .item_lines_from_record(&item, Utc::now())
                    .context("expand item record to v2 lines")
            },
        )
    }

    /// Narrow packed-ItemRecord helper for migrate / tests that still hold a SessionRecord.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn append_item(&self, record: &SessionRecord, item: ItemRecord) -> Result<()> {
        self.append_item_at(&record.rollout_path, item)
    }

    /// Drops cached append state after a caller rewrites a rollout file
    /// outside [`Self::append_rollout_lines`] (for example session/import).
    /// Runs a raw file mutation under the canonical journal's state mutex.
    /// Used by import and other out-of-band rewrites so no cached append state
    /// can survive the mutation. The caller must hold the SessionFileLock lease.
    pub(crate) fn with_journal_state<T>(
        &self,
        rollout_path: &Path,
        mutate: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let _lease = crate::runtime::session_file_lock::SessionFileLock::acquire(rollout_path)
            .map_err(|error| anyhow::anyhow!("acquire journal lease: {error}"))?;
        let shared = self.shared_journal(rollout_path)?;
        let mut guard = shared
            .state
            .lock()
            .expect("rollout journal state mutex poisoned");
        if guard.closed {
            anyhow::bail!("journal {} was deleted", rollout_path.display());
        }
        guard.write = None;
        let original_len = journal_stamp(rollout_path).len;
        let result = mutate();
        if result.is_err() && journal_stamp(rollout_path).len > original_len {
            let _ = truncate_rollout(rollout_path, original_len);
        }
        guard.write = None;
        result
    }

    /// Appends a durable session-deletion marker. The enclosing Internal line
    /// carries the native session id; scoped readers reject the session and
    /// index rebuilds omit it.
    pub(crate) fn append_session_deleted_at(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        deleted_at: chrono::DateTime<Utc>,
    ) -> Result<()> {
        self.append_rollout_lines(
            rollout_path,
            vec![RolloutLine::Internal {
                v: 2,
                timestamp: deleted_at,
                session_id,
                turn_id: None,
                seq: 0,
                entry: InternalRecord::SessionDeleted {
                    schema_version: 1,
                    deleted_at,
                },
            }],
        )
    }

    /// Appends one session-title update line to the durable rollout journal.
    pub(crate) fn append_title_update_at(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        title: String,
        previous_title: Option<String>,
    ) -> Result<()> {
        let session_id =
            devo_core::native_session_id(session_id).context("native session id for title")?;
        self.append_rollout_lines(
            rollout_path,
            vec![devo_core::title_line(
                Utc::now(),
                session_id,
                title,
                previous_title,
            )],
        )
    }

    /// Thin SessionRecord wrapper for callers that still hold a record.
    #[cfg(test)]
    pub(crate) fn append_title_update(
        &self,
        record: &SessionRecord,
        title: String,
        _title_state: SessionTitleState,
        previous_title: Option<String>,
    ) -> Result<()> {
        self.append_title_update_at(&record.rollout_path, record.id, title, previous_title)
    }

    /// Appends the locked session context once (path-first).
    pub(crate) fn append_session_context_updated_at(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        session_context: SessionContext,
    ) -> Result<()> {
        let native_session_id =
            devo_core::native_session_id(session_id).context("native session id for context")?;
        self.append_rollout_lines(
            rollout_path,
            vec![devo_core::session_context_line(
                Utc::now(),
                native_session_id,
                session_context,
            )],
        )
    }

    /// Appends one turn line, recording session context separately when needed
    /// (path-first Native).
    pub(crate) fn append_turn_deduped_at(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        session_context_recorded: &mut bool,
        turn: &devo_protocol::native::turn::Turn,
        extras: Option<TurnPersistenceExtras>,
        session_context: Option<SessionContext>,
    ) -> Result<()> {
        if let Some(session_context) = session_context
            && !*session_context_recorded
        {
            self.append_session_context_updated_at(rollout_path, session_id, session_context)?;
            *session_context_recorded = true;
        }
        self.append_turn_at(rollout_path, turn, extras)
    }

    /// Thin SessionRecord + TurnRecord wrapper for migrate / test turn dedupe appends.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn append_turn_deduped(
        &self,
        record: &SessionRecord,
        session_context_recorded: &mut bool,
        turn: TurnRecord,
        session_context: Option<SessionContext>,
    ) -> Result<()> {
        self.append_turn_deduped_at(
            &record.rollout_path,
            record.id,
            session_context_recorded,
            &devo_core::canonical_turn_from_record(&turn)
                .context("build native turn from record")?,
            Some(devo_core::turn_persistence_extras_from_record(&turn)),
            session_context,
        )
    }

    /// Appends one compaction snapshot line to the durable rollout journal.
    pub(crate) fn append_compaction_snapshot_at(
        &self,
        rollout_path: &Path,
        snapshot: CompactionSnapshotLine,
    ) -> Result<()> {
        let timestamp = snapshot.timestamp;
        let line = devo_core::compaction_snapshot_line(timestamp, &snapshot)
            .context("build compaction snapshot v2 line")?;
        self.append_rollout_lines(rollout_path, vec![line])
    }

    /// Appends a compaction summary item, its tree edge, its snapshot, and the
    /// advanced session-leaf pointer in one journal write so a mid-persist
    /// failure cannot strand the item without its snapshot or tree linkage.
    pub(crate) fn append_compaction_summary_and_snapshot_lines_at(
        &self,
        rollout_path: &Path,
        item: devo_protocol::native::item::ItemEnvelope,
        snapshot: CompactionSnapshotLine,
        parent_id: Option<devo_protocol::native::ids::ItemId>,
        leaf_epoch: u64,
    ) -> Result<()> {
        let session_id = item.session_id;
        let item_id = item.id;
        let tree_edge_line = devo_core::tree_edge_line(Utc::now(), session_id, item_id, parent_id);
        let item_line = RolloutLine::Item {
            v: ROLLOUT_FORMAT_VERSION,
            timestamp: Utc::now(),
            item,
        };
        let snapshot_line = devo_core::compaction_snapshot_line(snapshot.timestamp, &snapshot)
            .context("build compaction snapshot v2 line")?;
        let session_leaf_line =
            devo_core::session_leaf_line(Utc::now(), session_id, Some(item_id), leaf_epoch);
        self.append_rollout_lines(
            rollout_path,
            vec![tree_edge_line, item_line, snapshot_line, session_leaf_line],
        )
    }

    /// Appends one accepted message-edit record to the durable rollout journal.
    pub(crate) fn append_message_edit_recorded_at(
        &self,
        rollout_path: &Path,
        edit: MessageEditRecordedRecord,
    ) -> Result<()> {
        let line = devo_core::message_edit_line(Utc::now(), &edit)
            .context("build message edit v2 line")?;
        self.append_rollout_lines(rollout_path, vec![line])
    }

    /// Appends one turn-superseded marker to the durable rollout journal.
    pub(crate) fn append_turn_superseded_at(
        &self,
        rollout_path: &Path,
        superseded: TurnSupersededRecord,
    ) -> Result<()> {
        let line = devo_core::turn_superseded_line(Utc::now(), &superseded)
            .context("build turn superseded v2 line")?;
        self.append_rollout_lines(rollout_path, vec![line])
    }

    /// Appends one workspace-restore-start record to the durable rollout journal.
    pub(crate) fn append_workspace_restore_started_at(
        &self,
        rollout_path: &Path,
        restore: TurnWorkspaceRestoreStartedRecord,
    ) -> Result<()> {
        self.append_rollout_lines(
            rollout_path,
            vec![devo_core::workspace_restore_started_line(
                Utc::now(),
                restore,
            )],
        )
    }

    /// Appends one workspace-checkpoint record to the durable rollout journal.
    pub(crate) fn append_workspace_checkpoint_recorded_at(
        &self,
        rollout_path: &Path,
        checkpoint: TurnWorkspaceCheckpointRecordedRecord,
    ) -> Result<()> {
        self.append_rollout_lines(
            rollout_path,
            vec![devo_core::workspace_checkpoint_line(Utc::now(), checkpoint)],
        )
    }

    /// Appends one workspace-change record to the durable rollout journal.
    pub(crate) fn append_workspace_change_recorded_at(
        &self,
        rollout_path: &Path,
        change: TurnWorkspaceChangeRecordedRecord,
    ) -> Result<()> {
        self.append_rollout_lines(
            rollout_path,
            vec![devo_core::workspace_change_line(Utc::now(), change)],
        )
    }

    /// Appends one workspace-restore-completed record to the durable rollout journal.
    pub(crate) fn append_workspace_restore_completed_at(
        &self,
        rollout_path: &Path,
        restore: TurnWorkspaceRestoreCompletedRecord,
    ) -> Result<()> {
        self.append_rollout_lines(
            rollout_path,
            vec![devo_core::workspace_restore_completed_line(
                Utc::now(),
                restore,
            )],
        )
    }

    /// Appends one rollback marker to the durable rollout journal.
    pub(crate) fn append_session_rollback_at(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        retained_turn_ids: Vec<TurnId>,
        retained_item_ids: Vec<ItemId>,
        latest_turn_id: Option<TurnId>,
    ) -> Result<()> {
        let session_id =
            devo_core::native_session_id(session_id).context("native session id for rollback")?;
        let retained_turns = retained_turn_ids
            .into_iter()
            .map(devo_core::native_turn_id)
            .collect::<Result<Vec<_>, _>>()
            .context("native retained turn ids")?;
        let retained_items = retained_item_ids
            .into_iter()
            .map(devo_core::native_item_id)
            .collect::<Result<Vec<_>, _>>()
            .context("native retained item ids")?;
        let latest = latest_turn_id
            .map(devo_core::native_turn_id)
            .transpose()
            .context("native latest turn id")?;
        self.append_rollout_lines(
            rollout_path,
            vec![devo_core::session_rollback_line(
                Utc::now(),
                session_id,
                retained_turns,
                retained_items,
                latest,
            )],
        )
    }

    /// Loads every durable session that can be rebuilt from canonical rollout files.
    pub(crate) async fn load_sessions(
        &self,
        deps: &ServerRuntimeDependencies,
    ) -> Result<HashMap<devo_protocol::native::ids::SessionId, RuntimeSession>> {
        let mut sessions = HashMap::new();
        for rollout_path in self.rollout_paths()? {
            let indexed = match read_rollout_index_sessions(&rollout_path) {
                Ok(indexed) => indexed,
                Err(error) => {
                    tracing::warn!(
                        rollout_path = %rollout_path.display(),
                        error = %error,
                        "failed to index rollout for bulk restore; skipping journal"
                    );
                    continue;
                }
            };
            for entry in indexed {
                let session_id = entry.record.id;
                match self
                    .load_session_from_rollout_for_session(
                        &rollout_path,
                        session_id,
                        deps,
                        /*history*/ None,
                    )
                    .await
                {
                    Ok(recovered) => {
                        sessions.insert(recovered.summary.session_id(), recovered);
                    }
                    Err(error) => {
                        tracing::warn!(
                            rollout_path = %rollout_path.display(),
                            session_id = %session_id,
                            error = %error,
                            "failed to replay session from journal; skipping"
                        );
                    }
                }
            }
        }
        Ok(sessions)
    }

    /// Indexes every logical session in every rollout journal into SQLite
    /// without replaying turns. A shared root journal contributes one index row
    /// per `SessionMeta`; a legacy independent child journal contributes one.
    pub(crate) fn index_rollout_metadata(&self, db: &crate::db::Database) -> Result<()> {
        let mut canonical =
            HashMap::<SessionId, (chrono::DateTime<Utc>, PathBuf, SessionRecord)>::new();
        let mut deleted_session_ids = std::collections::HashSet::new();
        for rollout_path in self.rollout_paths()? {
            match read_rollout_index_scan(&rollout_path) {
                Ok(scan) => {
                    deleted_session_ids.extend(scan.deleted);
                    for indexed in scan.sessions {
                        let record = indexed.record;
                        let last_activity_at = indexed.last_activity_at;
                        match canonical.get(&record.id) {
                            Some((existing_activity, existing_path, _)) => {
                                if last_activity_at > *existing_activity {
                                    tracing::warn!(
                                        session_id = %record.id,
                                        kept_rollout_path = %rollout_path.display(),
                                        replaced_rollout_path = %existing_path.display(),
                                        "duplicate rollout for session id; keeping newest last_activity_at"
                                    );
                                    canonical.insert(
                                        record.id,
                                        (last_activity_at, rollout_path.clone(), record),
                                    );
                                } else {
                                    tracing::warn!(
                                        session_id = %record.id,
                                        kept_rollout_path = %existing_path.display(),
                                        ignored_rollout_path = %rollout_path.display(),
                                        "duplicate rollout for session id; keeping newest last_activity_at"
                                    );
                                }
                            }
                            None => {
                                canonical.insert(
                                    record.id,
                                    (last_activity_at, rollout_path.clone(), record),
                                );
                            }
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        rollout_path = %rollout_path.display(),
                        error = %error,
                        "failed to index rollout metadata; skipping file"
                    );
                }
            }
        }

        for (session_id, (last_activity_at, rollout_path, record)) in canonical {
            if deleted_session_ids.contains(&session_id) {
                continue;
            }
            let index_row = session_index_row_from_record(&record, last_activity_at);
            if let Err(error) =
                db.upsert_rollout_index_session(index_row, Some(rollout_path.as_path()))
            {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "failed to upsert indexed session metadata"
                );
            }
        }
        // A durable tombstone wins over any live row from another journal.
        for session_id in deleted_session_ids {
            if let Err(error) = db.clear_pending(&session_id, crate::db::QueueType::Turn) {
                tracing::warn!(%session_id, %error, "failed to clear tombstoned turn queue");
            }
            if let Err(error) = db.clear_pending(&session_id, crate::db::QueueType::Steer) {
                tracing::warn!(%session_id, %error, "failed to clear tombstoned steer queue");
            }
            if let Err(error) = db.delete_session(&session_id) {
                tracing::warn!(%session_id, %error, "failed to delete tombstoned session row");
            }
        }
        Ok(())
    }

    /// Finds a rollout file by session id when the SQLite index is stale.
    pub(crate) fn find_rollout_by_session_id(
        &self,
        session_id: &devo_protocol::native::ids::SessionId,
    ) -> Result<Option<PathBuf>> {
        let exact = format!("{session_id}.jsonl");
        for rollout_path in self.rollout_paths()? {
            let Some(file_name) = rollout_path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if file_name == exact {
                return Ok(Some(rollout_path));
            }
        }
        Ok(None)
    }

    /// Finds a journal containing the session even when its latest lifecycle
    /// marker is a tombstone. Delete retries use this to remove files left by a
    /// crash between durable marking and unlink; resume/read paths must use the
    /// tombstone-excluding lookup instead.
    pub(crate) fn find_journal_containing_session_including_deleted(
        &self,
        session_id: &devo_protocol::native::ids::SessionId,
    ) -> Result<Option<PathBuf>> {
        self.find_journal_containing_session_strict(session_id, JournalVisibility::IncludeDeleted)
    }

    /// Strict journal lookup. Unlike the lenient index/read fallback, a damaged
    /// or unreadable journal is an error, not a missing session.
    pub(crate) fn find_journal_containing_session_strict(
        &self,
        session_id: &devo_protocol::native::ids::SessionId,
        visibility: JournalVisibility,
    ) -> Result<Option<PathBuf>> {
        for rollout_path in self.rollout_paths()? {
            let (live, tombstoned) = self.journal_session_membership(&rollout_path, session_id)?;
            let deleted = visibility == JournalVisibility::IncludeDeleted && tombstoned;
            if live || deleted {
                return Ok(Some(rollout_path));
            }
        }
        Ok(None)
    }

    /// Strict membership scan for one journal/session using the same fail-closed
    /// rollout visitor as resume. Returns `(live, tombstoned)`.
    pub(crate) fn journal_session_membership(
        &self,
        rollout_path: &Path,
        session_id: &devo_protocol::native::ids::SessionId,
    ) -> Result<(bool, bool)> {
        let mut live = false;
        let mut deleted = false;
        visit_rollout_lines(rollout_path, "journal membership", |line| {
            match line {
                RolloutLine::SessionMeta { session, .. } if session.id == *session_id => {
                    live = true;
                }
                RolloutLine::Internal {
                    session_id: owner,
                    entry: InternalRecord::SessionDeleted { .. },
                    ..
                } if owner == *session_id => {
                    deleted = true;
                }
                _ => {}
            }
            Ok(())
        })?;
        Ok((live, deleted))
    }

    /// Single shared resolver for consumers that hold an indexed path: trust it
    /// only when it exists and proves membership; otherwise scan strictly.
    pub(crate) fn resolve_session_journal(
        &self,
        session_id: &devo_protocol::native::ids::SessionId,
        indexed_path: Option<&Path>,
        visibility: JournalVisibility,
    ) -> Result<Option<PathBuf>> {
        if let Some(path) = indexed_path.filter(|path| path.exists()) {
            let (live, tombstoned) = self.journal_session_membership(path, session_id)?;
            let deleted = visibility == JournalVisibility::IncludeDeleted && tombstoned;
            if live || deleted {
                return Ok(Some(path.to_path_buf()));
            }
        }
        self.find_journal_containing_session_strict(session_id, visibility)
    }

    /// Raw persisted agent lineage, ignoring tombstone state and index rows.
    /// Delete retries need the parent edge even after the live scoped readers
    /// intentionally reject a tombstoned session.
    pub(crate) fn raw_session_parent(
        &self,
        rollout_path: &Path,
        session_id: &SessionId,
    ) -> Result<Option<Option<SessionId>>> {
        let mut found = None;
        visit_rollout_lines(rollout_path, "lineage read", |line| {
            if let RolloutLine::SessionMeta { session, .. } = line
                && session.id == *session_id
            {
                found = Some(session.parent.as_ref().map(|parent| {
                    let devo_protocol::native::session::SessionParent::Agent { session_id, .. } =
                        parent;
                    *session_id
                }));
            }
            Ok(())
        })?;
        Ok(found)
    }

    /// Every persisted agent edge, including sessions whose latest marker is a
    /// tombstone and sessions absent from the rebuildable SQLite index.
    pub(crate) fn persisted_agent_edges(&self) -> Result<Vec<(SessionId, SessionId)>> {
        let mut edges = Vec::new();
        for rollout_path in self.rollout_paths()? {
            visit_rollout_lines(&rollout_path, "lineage read", |line| {
                if let RolloutLine::SessionMeta { session, .. } = line
                    && let Some(devo_protocol::native::session::SessionParent::Agent {
                        session_id: parent,
                        ..
                    }) = session.parent.as_ref()
                {
                    edges.push((session.id, *parent));
                }
                Ok(())
            })?;
        }
        Ok(edges)
    }

    /// Resolves a session cwd from the SQLite index, falling back to rollout SessionMeta.
    pub(crate) fn resolve_indexed_session_cwd(
        &self,
        db: &crate::db::Database,
        session_id: &devo_protocol::native::ids::SessionId,
    ) -> Result<Option<PathBuf>> {
        if let Some(index) = db.get_session_index(session_id)? {
            return Ok(Some(index.session.cwd));
        }
        if let Some(rollout_path) =
            self.resolve_session_journal(session_id, None, JournalVisibility::Live)?
            && let Some(indexed) = read_rollout_index_sessions(&rollout_path)?
                .into_iter()
                .find(|entry| entry.record.id == *session_id)
        {
            return Ok(Some(indexed.record.cwd));
        }
        Ok(None)
    }

    /// Deletes the durable storage owned by one session.
    ///
    /// The journal is unlinked only when it is the target session's own file
    /// (root journal or legacy independent child journal). A child sharing the
    /// root journal never unlinks it; the caller must persist a tombstone first.
    /// `delete_root_tree` removes `session-artifacts/<root_id>/` entirely.
    pub(crate) fn delete_session_journal(
        &self,
        session_id: &SessionId,
        journal_path: Option<&Path>,
        session_dir: Option<&Path>,
        delete_root_tree: bool,
    ) -> Result<bool> {
        let mut deleted = false;
        let mut output_candidates = Vec::new();
        let mut removed_journal = false;

        let own_journal_path = journal_path.filter(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem == session_id.as_str())
        });
        let should_unlink_journal =
            own_journal_path.is_some() || (delete_root_tree && journal_path.is_none());
        if should_unlink_journal {
            if let Some(rollout_path) = journal_path {
                if self.unlink_journal_file(rollout_path, &mut output_candidates)? {
                    deleted = true;
                    removed_journal = true;
                }
            } else {
                // Compatibility fallback for callers without an indexed path.
                let exact = format!("{session_id}.jsonl");
                for rollout_path in self.rollout_paths()? {
                    if rollout_path.file_name().and_then(|name| name.to_str())
                        != Some(exact.as_str())
                    {
                        continue;
                    }
                    if self.unlink_journal_file(&rollout_path, &mut output_candidates)? {
                        deleted = true;
                        removed_journal = true;
                    }
                }
            }
        }

        if delete_root_tree {
            let artifacts = self.root_session_dir(session_id);
            if artifacts.is_dir() {
                match std::fs::remove_dir_all(&artifacts) {
                    Ok(()) => deleted = true,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("delete session artifacts {}", artifacts.display())
                        });
                    }
                }
            }
        } else if let Some(session_dir) = session_dir
            && self.is_child_artifact_dir(session_dir)
            && session_dir.is_dir()
        {
            match std::fs::remove_dir_all(session_dir) {
                Ok(()) => deleted = true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("delete child session dir {}", session_dir.display())
                    });
                }
            }
        }

        if removed_journal && let Err(error) = self.collect_unreferenced_outputs(&output_candidates)
        {
            tracing::warn!(%error, "output cleanup deferred to preserve session references");
        }
        Ok(deleted)
    }

    /// Unlinks one physical journal while its JournalShared state stays locked
    /// against same-process appends. The caller holds / will acquire the
    /// cross-process SessionFileLock lease.
    fn unlink_journal_file(
        &self,
        rollout_path: &Path,
        output_candidates: &mut Vec<devo_core::tools::output_store::OutputArtifact>,
    ) -> Result<bool> {
        let _lease = crate::runtime::session_file_lock::SessionFileLock::acquire(rollout_path)
            .map_err(|error| anyhow::anyhow!("acquire journal lease before unlink: {error}"))?;
        let key = crate::journal_path::canonical_journal_key(rollout_path)?;
        let shared = self.shared_journal(rollout_path)?;
        let mut guard = shared
            .state
            .lock()
            .expect("rollout journal state mutex poisoned");
        if !key.exists() {
            guard.closed = true;
            guard.write = None;
            return Ok(false);
        }
        output_candidates.extend(devo_core::output_replay::read_output_references(&key)?);
        let removed = match std::fs::remove_file(&key) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(error).with_context(|| format!("delete rollout {}", key.display()));
            }
        };
        // Keep the JournalShared (and its local strong Arc) alive with
        // closed=true. Straggler/new appends through this store hit the same
        // mutex and fail instead of recreating the root. No second global set.
        guard.closed = true;
        guard.write = None;
        drop(guard);
        Ok(removed)
    }

    /// True when `dir` is a strict descendant of `<data_root>/session-artifacts`.
    /// A root artifact dir (`session-artifacts/<root>`) is not a child dir and
    /// must only be removed by a root-tree delete.
    pub(crate) fn is_child_artifact_dir(&self, dir: &Path) -> bool {
        let artifacts_root = self.data_root.join("session-artifacts");
        let artifacts_root = std::fs::canonicalize(&artifacts_root).unwrap_or(artifacts_root);
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let Ok(relative) = dir.strip_prefix(&artifacts_root) else {
            return false;
        };
        relative.components().count() >= 2
    }

    #[cfg(test)]
    pub(crate) async fn load_session_from_rollout(
        &self,
        rollout_path: &Path,
        deps: &ServerRuntimeDependencies,
    ) -> Result<RuntimeSession> {
        let mut session_id = None;
        visit_rollout_lines(rollout_path, "resume", |line| {
            if session_id.is_none()
                && let RolloutLine::SessionMeta { session, .. } = line
            {
                session_id = Some(session.id);
            }
            Ok(())
        })?;
        let session_id = session_id.with_context(|| {
            format!(
                "missing SessionMeta line in rollout {}",
                rollout_path.display()
            )
        })?;
        self.load_session_from_rollout_for_session(
            rollout_path,
            session_id,
            deps,
            /*history*/ None,
        )
        .await
    }

    /// Loads one logical session from a journal that may contain many.
    ///
    /// `history` must already be scoped to `session_id`; `None` performs the
    /// scoped read internally.
    pub(crate) async fn load_session_from_rollout_for_session(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        deps: &ServerRuntimeDependencies,
        history: Option<&devo_core::CanonicalHistory>,
    ) -> Result<RuntimeSession> {
        let mut replay = ReplayState::default();
        let mut deleted_at = None;
        visit_rollout_lines(rollout_path, "resume", |v2| {
            if v2.session_id() != Some(session_id) {
                return Ok(());
            }
            if let RolloutLine::Internal {
                entry:
                    InternalRecord::SessionDeleted {
                        deleted_at: deleted,
                        ..
                    },
                ..
            } = &v2
            {
                deleted_at = Some(*deleted);
                return Ok(());
            }
            replay
                .apply_rollout_line(v2)
                .with_context(|| format!("apply v2 line from {}", rollout_path.display()))
        })?;
        if let Some(deleted_at) = deleted_at {
            anyhow::bail!("session {session_id} was deleted at {deleted_at} and cannot be resumed");
        }
        // Captured before `into_runtime_session` consumes the replay state:
        // the path-scoping block below needs it to keep the compacted prompt.
        let compaction_snapshot = replay.latest_compaction_snapshot.clone();

        let mut recovered = replay
            .into_runtime_session(deps)
            .await
            .with_context(|| format!("replay rollout {}", rollout_path.display()))?;
        if recovered.summary.session_id() != session_id {
            anyhow::bail!(
                "rollout session id mismatch: expected {session_id}, got {}",
                recovered.summary.session_id()
            );
        }
        if let Some(turn) = &recovered.latest_turn {
            let execution = devo_core::durable_execution::read_execution_replay_for_session(
                rollout_path,
                &turn.turn_id(),
                &session_id,
            )?;
            if execution.has_checkpoint {
                recovered.core_session.lock().await.set_prompt_messages(
                    devo_core::history::response_items_to_messages(&execution.items),
                );
            }
        }
        // Absolute path is actor-owned; Native Session does not expose it.
        recovered.rollout_path = Some(rollout_path.to_path_buf());
        recovered.summary.session_dir = self.session_dir_for(rollout_path, &session_id);
        // Restrict model context to the active transcript-tree path (root→leaf).
        let owned_history;
        let scoped_history = match history {
            Some(history) => Some(history),
            None => {
                owned_history =
                    devo_core::read_canonical_history_for_session(rollout_path, &session_id).ok();
                owned_history.as_ref()
            }
        };
        if let Some(history) = scoped_history {
            let path_ids = devo_core::active_path_item_ids(history);
            if !path_ids.is_empty() {
                recovered.persisted_turn_items.retain(|item| {
                    path_ids.contains(&item.item_id) || !devo_core::is_tree_visible_item(&item.item)
                });
                let mut rebuilt_messages = Vec::new();
                let mut rebuilt_history = Vec::new();
                let mut tool_names = std::collections::HashMap::new();
                for item in &recovered.persisted_turn_items {
                    crate::prompt_from_native_item::apply_native_item(
                        &mut rebuilt_messages,
                        &mut rebuilt_history,
                        &mut tool_names,
                        item.item.clone(),
                    );
                }
                recovered.history_items = rebuilt_history;
                // A live compaction snapshot must survive path scoping: the
                // snapshot rebuild positions its summary anchor inside the
                // scoped path, so it both compacts and scopes. The linear
                // rebuild is only the fallback for a snapshot that no longer
                // resolves (e.g. a rollback dropped the summary item) — the
                // same degradation `into_runtime_session` applies unscoped.
                let prompt_messages = compaction_snapshot
                    .as_ref()
                    .and_then(|snapshot| {
                        build_prompt_messages_from_snapshot(
                            &recovered.persisted_turn_items,
                            snapshot,
                        )
                    })
                    .unwrap_or(rebuilt_messages);
                recovered
                    .core_session
                    .lock()
                    .await
                    .set_prompt_messages(prompt_messages);
            }
        }
        Ok(recovered)
    }

    /// Reads durable workspace checkpoints without reconstructing runtime state.
    ///
    /// P4d rollback plans need the pre-turn ghost commit plus its untracked
    /// manifest. This follows the same versioned parse and fail-closed rules as
    /// `load_session_from_rollout`.
    pub(crate) fn workspace_checkpoints_at(
        &self,
        rollout_path: &Path,
        session_id: &SessionId,
    ) -> Result<Vec<TurnWorkspaceCheckpointRecordedRecord>> {
        let mut checkpoints = Vec::new();
        visit_rollout_lines(rollout_path, "checkpoint read", |v2| {
            if v2.session_id() != Some(*session_id) {
                return Ok(());
            }
            if let RolloutLine::WorkspaceCheckpoint { record, .. } = v2 {
                checkpoints.push(record);
            }
            Ok(())
        })?;
        Ok(checkpoints)
    }

    pub(crate) fn rollout_paths(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let sessions = self.data_root.join("sessions");
        if sessions
            .try_exists()
            .context("inspect sessions directory")?
        {
            collect_rollout_files(&sessions, &mut files)?;
        }
        let artifacts = self.data_root.join("session-artifacts");
        if artifacts
            .try_exists()
            .context("inspect session artifacts directory")?
        {
            collect_rollout_files(&artifacts, &mut files)?;
        }
        files.sort();
        Ok(files)
    }

    fn root_rollout_path(&self, session_id: &devo_protocol::native::ids::SessionId) -> PathBuf {
        self.data_root
            .join("sessions")
            .join(format!("{session_id}.jsonl"))
    }

    /// Root session artifact directory for a durable root session.
    pub(crate) fn root_session_dir(&self, root_id: &SessionId) -> PathBuf {
        self.data_root
            .join("session-artifacts")
            .join(root_id.to_string())
    }

    /// Deterministic child/grandchild artifact directory under the true root.
    ///
    /// New descendants share the root journal, so they must not derive their
    /// artifact directory from the journal's parent (`sessions/`). The id is
    /// stable and independent for every child at any depth.
    pub(crate) fn child_session_dir(&self, root_id: &SessionId, child_id: &SessionId) -> PathBuf {
        self.root_session_dir(root_id)
            .join("agents")
            .join(child_id.to_string())
    }

    /// Artifact directory for a loaded session, derived from its journal path
    /// and session id. Used at hydration time when no live spawn computed it.
    pub(crate) fn session_dir_for(
        &self,
        rollout_path: &Path,
        session_id: &SessionId,
    ) -> Option<PathBuf> {
        let canonical = crate::journal_path::canonical_journal_key(rollout_path).ok()?;
        let parent = canonical.parent()?;
        let stem = canonical.file_stem()?.to_str()?;
        let sessions_dir = self.data_root.join("sessions");
        let sessions_dir = std::fs::canonicalize(&sessions_dir).unwrap_or(sessions_dir);
        if parent == sessions_dir {
            if stem == session_id.as_str() {
                return Some(self.root_session_dir(session_id));
            }
            let root_id = SessionId::from_string(stem.to_owned());
            return Some(self.child_session_dir(&root_id, session_id));
        }
        if stem == session_id.as_str() {
            // Legacy independent child journal: its parent is the child dir.
            return Some(parent.to_path_buf());
        }
        // A child sharing a non-root journal (legacy branch). Keep the
        // artifact dir independent of the shared file.
        Some(parent.join("agents").join(session_id.to_string()))
    }

    /// Appends one canonical Native item envelope as a v2 rollout line
    /// (path-first).
    pub(crate) fn append_canonical_item_at(
        &self,
        rollout_path: &Path,
        item: devo_protocol::native::item::ItemEnvelope,
    ) -> Result<()> {
        let line = RolloutLine::Item {
            v: 2,
            timestamp: Utc::now(),
            item,
        };
        self.append_rollout_lines(rollout_path, vec![line])
    }

    /// Appends a transcript-tree leaf pointer (last-wins on read).
    pub(crate) fn append_session_leaf_at(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        leaf_id: Option<devo_protocol::native::ids::ItemId>,
        epoch: u64,
    ) -> Result<()> {
        let line = devo_core::session_leaf_line(Utc::now(), session_id, leaf_id, epoch);
        self.append_rollout_lines(rollout_path, vec![line])
    }

    /// Appends a transcript-tree parent edge for one item.
    pub(crate) fn append_tree_edge_at(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        child_id: devo_protocol::native::ids::ItemId,
        parent_id: Option<devo_protocol::native::ids::ItemId>,
    ) -> Result<()> {
        let line = devo_core::tree_edge_line(Utc::now(), session_id, child_id, parent_id);
        self.append_rollout_lines(rollout_path, vec![line])
    }

    /// Thin SessionRecord wrapper for canonical item appends.
    #[cfg(test)]
    pub(crate) fn append_canonical_item(
        &self,
        record: &SessionRecord,
        item: devo_protocol::native::item::ItemEnvelope,
    ) -> Result<()> {
        self.append_canonical_item_at(&record.rollout_path, item)
    }

    pub(crate) fn append_goal_state(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        goal: Option<serde_json::Value>,
    ) -> Result<()> {
        self.append_rollout_lines(
            rollout_path,
            vec![RolloutLine::Internal {
                v: 2,
                timestamp: Utc::now(),
                session_id,
                turn_id: None,
                seq: 0,
                entry: devo_core::InternalRecord::GoalState {
                    schema_version: 1,
                    goal,
                },
            }],
        )
    }

    pub(crate) fn append_usage_record(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        record: devo_protocol::native::usage::UsageRecord,
    ) -> Result<()> {
        self.append_rollout_lines(
            rollout_path,
            vec![RolloutLine::Internal {
                v: 2,
                timestamp: record.recorded_at,
                session_id,
                turn_id: record.turn_id,
                seq: 0,
                entry: devo_core::InternalRecord::UsageRecord { record },
            }],
        )
    }

    pub(crate) fn append_approval_checkpoint(
        &self,
        rollout_path: &Path,
        checkpoint: &devo_core::TurnApprovalCheckpointRecordedRecord,
    ) -> Result<()> {
        self.append_rollout_lines(
            rollout_path,
            vec![RolloutLine::Internal {
                v: 2,
                timestamp: checkpoint.created_at,
                session_id: checkpoint.session_id,
                turn_id: Some(checkpoint.turn_id),
                seq: 0,
                entry: devo_core::InternalRecord::TurnApprovalCheckpoint(Box::new(
                    checkpoint.clone(),
                )),
            }],
        )
    }

    fn append_rollout_lines_with<F>(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        mode: JournalWriteMode,
        build: F,
    ) -> Result<()>
    where
        F: FnOnce(&mut WritePathState) -> Result<Vec<RolloutLine>>,
    {
        if let Some(parent) = rollout_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create rollout directory {}", parent.display()))?;
        }
        // Central lease enforcement: every write path takes the same-process
        // reusable / cross-process exclusive SessionFileLock before the journal
        // state mutex. Actor callers already hold it; this is a cheap same-
        // process reacquire, not a second lock scheme.
        let _lease = crate::runtime::session_file_lock::SessionFileLock::acquire(rollout_path)
            .map_err(|error| anyhow::anyhow!("acquire journal lease: {error}"))?;
        let shared = self.shared_journal(rollout_path)?;
        let mut guard = shared
            .state
            .lock()
            .expect("rollout journal state mutex poisoned");
        if guard.closed {
            anyhow::bail!("journal {} was deleted", rollout_path.display());
        }
        // Existence is checked under the same state mutex that unlink holds,
        // so an ordinary append cannot recreate a journal being deleted.
        if mode == JournalWriteMode::ExistingOnly && !rollout_path.exists() {
            anyhow::bail!(
                "journal {} is missing; refusing to recreate an established journal",
                rollout_path.display()
            );
        }
        let observed = journal_stamp(rollout_path);
        let mut state = match guard.write.take() {
            Some(state) if state.observed == observed => state,
            _ => hydrate_write_state(rollout_path)?,
        };
        // Hydration may repair a crash tail; the rollback length must be the
        // repaired length, never the pre-repair observed length.
        let original_len = state.observed.len;
        let result = (|| -> Result<()> {
            let rollout_lines = build(&mut state)?;
            for line in &rollout_lines {
                if rollout_line_session_id(line) != Some(session_id) {
                    anyhow::bail!("rollout append batch mixes sessions: expected {session_id}");
                }
                if let RolloutLine::SessionMeta { extras, .. } = line {
                    let merged = merge_session_extras(
                        state.session_extras.get(&session_id),
                        extras.as_deref(),
                    );
                    state.session_extras.insert(session_id, merged);
                }
            }
            {
                let session_state = state.session_state_mut(session_id);
                for line in &rollout_lines {
                    session_state.observe_rollout_line(line);
                }
            }
            self.write_rollout_lines(rollout_path, &mut state, &rollout_lines)?;
            state.observed = journal_stamp(rollout_path);
            Ok(())
        })();
        if let Err(error) = result {
            if journal_stamp(rollout_path).len > original_len {
                let _ = truncate_rollout(rollout_path, original_len);
            }
            guard.write = None;
            return Err(error);
        }
        guard.write = Some(state);
        Ok(())
    }

    pub(crate) fn append_rollout_lines(
        &self,
        rollout_path: &Path,
        rollout_lines: Vec<RolloutLine>,
    ) -> Result<()> {
        let Some(session_id) = rollout_lines.iter().find_map(rollout_line_session_id) else {
            return Ok(());
        };
        if rollout_lines
            .iter()
            .any(|line| rollout_line_session_id(line) != Some(session_id))
        {
            anyhow::bail!("rollout append batch mixes sessions: expected {session_id}");
        }
        self.append_rollout_lines_with(
            rollout_path,
            session_id,
            JournalWriteMode::ExistingOnly,
            |_| Ok(rollout_lines),
        )
    }

    fn write_rollout_lines(
        &self,
        rollout_path: &Path,
        state: &mut WritePathState,
        rollout_lines: &[RolloutLine],
    ) -> Result<()> {
        let canonical_journal = crate::journal_path::canonical_journal_key(rollout_path)?;
        let rollout_path = canonical_journal.as_path();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(rollout_path)
            .with_context(|| format!("open rollout file {}", rollout_path.display()))?;
        let first_line_index = state.next_line_index;
        for rollout_line in rollout_lines {
            serde_json::to_writer(&mut file, rollout_line)
                .with_context(|| format!("serialize rollout line {}", rollout_path.display()))?;
            file.write_all(b"\n")
                .with_context(|| format!("write rollout newline {}", rollout_path.display()))?;
        }
        file.flush()
            .with_context(|| format!("flush rollout file {}", rollout_path.display()))?;
        // The rollout is the event log: an acknowledged write must survive a
        // crash, so every append ends in fsync (file data only, not the
        // directory entry — matches the pre-v2 durability floor plus the
        // event-log requirement).
        file.sync_data()
            .with_context(|| format!("fsync rollout file {}", rollout_path.display()))?;
        state.next_line_index += rollout_lines.len() as u64;

        // Outbox projection (08 §5/§7): derive delivery-log events from the
        // fsynced facts. Best effort — a failure here is backfilled by the
        // startup reconciler, so a crash may delay an event but never lose
        // or duplicate it.
        if let Some(db) = &self.event_log
            && let Err(error) =
                project_events_into_log(db, rollout_path, first_line_index, rollout_lines)
        {
            tracing::warn!(
                rollout = %rollout_path.display(),
                %error,
                "failed to project events into event_log; reconciliation will backfill"
            );
        }
        Ok(())
    }
}

/// Derives delivery-log rows from freshly written v2 lines and inserts them
/// idempotently, then advances the projection watermark.
fn project_events_into_log(
    db: &Database,
    rollout_path: &Path,
    first_line_index: u64,
    rollout_lines: &[RolloutLine],
) -> Result<()> {
    let mut rows = Vec::new();
    let mut last_line_index = first_line_index;
    for (offset, rollout_line) in rollout_lines.iter().enumerate() {
        let line_index = first_line_index + offset as u64;
        last_line_index = line_index;
        rows.extend(event_log_rows_for_rollout_line(
            rollout_path,
            line_index,
            0,
            rollout_line,
        )?);
    }
    db.insert_event_log_rows(&rows)?;
    if !rollout_lines.is_empty() {
        db.set_projection_watermark(rollout_path, last_line_index)?;
    }
    Ok(())
}

/// Builds the delivery-log rows derived from one v2 rollout fact (also used
/// by the startup reconciler, which passes a nonzero `sub_index` for v2
/// lines expanded from a packed legacy row).
pub(crate) fn event_log_rows_for_rollout_line(
    rollout_path: &Path,
    line_index: u64,
    sub_index: u64,
    rollout_line: &RolloutLine,
) -> Result<Vec<NewEventLogRow>> {
    let timestamp = rollout_line_timestamp(rollout_line);
    let canonical_journal = crate::journal_path::canonical_journal_key(rollout_path)
        .with_context(|| format!("resolve canonical journal {}", rollout_path.display()))?;
    let mut rows = Vec::new();
    for derived in events_from_rollout_line(rollout_line) {
        let envelope = EventEnvelope {
            meta: EventMeta {
                event_id: EventId::new(),
                stream_id: derived.stream_id.clone(),
                // Allocated by the event_log insert (per-stream monotonic);
                // replay hydrates meta.seq from the stored row.
                seq: None,
                emitted_at: timestamp,
                persisted: true,
                schema_version: EVENT_SCHEMA_VERSION,
                actor_client_id: None,
            },
            notification: derived.notification,
        };
        rows.push(NewEventLogRow {
            source_fact_id: source_fact_id(&canonical_journal, line_index, sub_index),
            event_kind: derived.event_kind.to_owned(),
            stream_id: derived.stream_id,
            event_id: envelope.meta.event_id.to_string(),
            payload: serde_json::to_string(&envelope).context("serialize event envelope")?,
            created_at: timestamp.to_rfc3339(),
        });
    }
    Ok(rows)
}

/// The wall-clock timestamp carried by any v2 line variant.
fn rollout_line_timestamp(line: &RolloutLine) -> chrono::DateTime<Utc> {
    match line {
        RolloutLine::SessionMeta { timestamp, .. }
        | RolloutLine::Turn { timestamp, .. }
        | RolloutLine::Item { timestamp, .. }
        | RolloutLine::Internal { timestamp, .. }
        | RolloutLine::SessionTitleUpdated { timestamp, .. }
        | RolloutLine::CompactionSnapshot { timestamp, .. }
        | RolloutLine::SessionRollback { timestamp, .. }
        | RolloutLine::WorkspaceCheckpoint { timestamp, .. }
        | RolloutLine::WorkspaceChange { timestamp, .. }
        | RolloutLine::WorkspaceRestoreStarted { timestamp, .. }
        | RolloutLine::WorkspaceRestoreCompleted { timestamp, .. } => *timestamp,
    }
}

/// Session attribution for one v2 line. This mirrors the core
/// `RolloutLine::session_id` contract while the core helper is being landed by
/// the storage worker; server write-state routing and index grouping use it.
fn rollout_line_session_id(line: &RolloutLine) -> Option<SessionId> {
    line.session_id()
}

fn merge_session_extras(
    existing: Option<&SessionPersistenceExtras>,
    incoming: Option<&SessionPersistenceExtras>,
) -> SessionPersistenceExtras {
    let mut merged = existing
        .cloned()
        .unwrap_or_else(invent_session_persistence_extras);
    let Some(incoming) = incoming else {
        return merged;
    };
    if incoming.session_context.is_some() {
        merged.session_context = incoming.session_context.clone();
    }
    if !incoming.cli_version.is_empty() {
        merged.cli_version = incoming.cli_version.clone();
    }
    if !incoming.source.is_empty() {
        merged.source = incoming.source.clone();
    }
    if incoming.collaboration_mode.is_some() {
        merged.collaboration_mode = incoming.collaboration_mode;
    }
    if incoming.permission_preset.is_some() {
        merged.permission_preset = incoming.permission_preset;
    }
    if incoming.agent_path.is_some() {
        merged.agent_path = incoming.agent_path.clone();
    }
    if incoming.agent_nickname.is_some() {
        merged.agent_nickname = incoming.agent_nickname.clone();
    }
    if incoming.kernel_snapshot_path.is_some() {
        merged.kernel_snapshot_path = incoming.kernel_snapshot_path.clone();
    }
    merged
}

/// Walks a rollout JSONL file, skipping blanks and stopping at a crash tail.
/// Returns whether a truncated final line was seen.
pub(crate) fn visit_rollout_lines(
    rollout_path: &Path,
    refuse_verb: &str,
    mut visit: impl FnMut(RolloutLine) -> Result<()>,
) -> Result<bool> {
    let file = File::open(rollout_path)
        .with_context(|| format!("open rollout file {}", rollout_path.display()))?;
    visit_open_rollout_lines(rollout_path, file, refuse_verb, &mut visit)
}

fn visit_open_rollout_lines(
    rollout_path: &Path,
    file: File,
    refuse_verb: &str,
    visit: &mut impl FnMut(RolloutLine) -> Result<()>,
) -> Result<bool> {
    // Snapshot the opened file length so a concurrent append cannot turn an
    // in-flight truncated tail into a seemingly damaged middle line. Only the
    // final row of this snapshot may be tolerated as a crash tail.
    let snapshot_len = file
        .metadata()
        .with_context(|| format!("stat rollout file {}", rollout_path.display()))?
        .len();
    visit_open_rollout_lines_snapshot(rollout_path, file, snapshot_len, refuse_verb, visit)
}

fn visit_open_rollout_lines_snapshot(
    rollout_path: &Path,
    file: File,
    snapshot_len: u64,
    refuse_verb: &str,
    visit: &mut impl FnMut(RolloutLine) -> Result<()>,
) -> Result<bool> {
    let reader = BufReader::new(file.take(snapshot_len));
    let mut lines = reader.lines().enumerate().peekable();
    while let Some((line_index, line)) = lines.next() {
        let line = line.with_context(|| format!("read line from {}", rollout_path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        match parse_rollout_line(&line) {
            Ok(line) => visit(line)?,
            Err(RolloutLineReadError::TruncatedTail)
                if rollout_remainder_is_crash_tail(&mut lines) =>
            {
                return Ok(true);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "rollout {} is damaged at line {}; refusing to {refuse_verb}",
                        rollout_path.display(),
                        line_index + 1
                    )
                });
            }
        }
    }
    Ok(false)
}

/// True when every remaining JSONL row is blank (or there are none). Used so a
/// truncated crash tail followed only by blank lines is still treated as final.
fn rollout_remainder_is_crash_tail<I>(lines: &mut std::iter::Peekable<I>) -> bool
where
    I: Iterator<Item = (usize, std::io::Result<String>)>,
{
    while let Some((_, next)) = lines.peek() {
        match next {
            Ok(line) if line.trim().is_empty() => {
                let _ = lines.next();
            }
            Ok(_) => return false,
            Err(_) => return false,
        }
    }
    true
}

/// Removes a trailing truncated JSONL crash tail so later appends cannot leave
/// mid-file truncation for loaders.
fn discard_rollout_crash_tail(rollout_path: &Path) -> Result<()> {
    let contents = match std::fs::read(rollout_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read rollout file {}", rollout_path.display()));
        }
    };
    if contents.is_empty() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&contents);
    let mut keep_len = 0usize;
    let mut line_start = 0usize;
    for (idx, _) in text.match_indices('\n') {
        let raw = &text[line_start..idx];
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            keep_len = idx + 1;
        } else {
            match parse_rollout_line(line) {
                Ok(_) => keep_len = idx + 1,
                Err(RolloutLineReadError::TruncatedTail) => {
                    truncate_rollout(rollout_path, keep_len as u64)?;
                    return Ok(());
                }
                Err(_) => return Ok(()),
            }
        }
        line_start = idx + 1;
    }
    let trailing = text[line_start..].trim_end_matches('\r');
    if !trailing.trim().is_empty()
        && matches!(
            parse_rollout_line(trailing),
            Err(RolloutLineReadError::TruncatedTail)
        )
    {
        truncate_rollout(rollout_path, keep_len as u64)?;
    }
    Ok(())
}

fn truncate_rollout(rollout_path: &Path, keep_len: u64) -> Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .open(rollout_path)
        .with_context(|| format!("open rollout for truncate {}", rollout_path.display()))?
        .set_len(keep_len)
        .with_context(|| format!("truncate rollout crash tail {}", rollout_path.display()))
}

/// Builds the write-path state for an existing rollout file by replaying its
/// current v2 contents through [`RolloutWriteState::observe_rollout_line`] (seq
/// counter, approval folds, settings epochs, cwd). Bounded per path: runs
/// once, on the first append, and the result is cached in the store. Also
/// returns the next JSONL row index, which becomes the `source_fact_id` line
/// index of every subsequent append.
///
/// Fails closed on any damaged or unsupported line: appending onto history
/// the write state could not fully read would fork the session's history.
fn hydrate_write_state(rollout_path: &Path) -> Result<WritePathState> {
    let mut state = WritePathState {
        sessions: HashMap::new(),
        session_extras: HashMap::new(),
        next_line_index: 0,
        observed: JournalStamp::missing(),
    };
    let file = match File::open(rollout_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(state);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("open rollout file {}", rollout_path.display()));
        }
    };
    let saw_crash_tail = visit_open_rollout_lines(rollout_path, file, "append", &mut |v2| {
        if let Some(session_id) = rollout_line_session_id(&v2) {
            if let RolloutLine::SessionMeta { extras, .. } = &v2 {
                let merged =
                    merge_session_extras(state.session_extras.get(&session_id), extras.as_deref());
                state.session_extras.insert(session_id, merged);
            }
            state
                .sessions
                .entry(session_id)
                .or_default()
                .observe_rollout_line(&v2);
        }
        state.next_line_index += 1;
        Ok(())
    })?;
    if saw_crash_tail {
        // Drop the unacked crash-tail bytes so a later append cannot turn this
        // into mid-file truncation for loaders. Must run after the read handle
        // is closed (Windows cannot truncate an open file).
        discard_rollout_crash_tail(rollout_path)?;
    }
    // Refresh after the optional repair: the pre-repair length must never
    // become the rollback length for a later failed append.
    state.observed = journal_stamp(rollout_path);
    Ok(state)
}

/// Resume hydrate state. Session/Turn lines are stored as Native (+ extras).
/// `into_runtime_session` installs `rollout_path` + Native summary and keeps
/// [`ReplayedTurn`] maps on `RuntimeSession::turns_by_id` for fork/cuts.
#[derive(Default)]
struct ReplayState {
    session: Option<ReplayedSession>,
    latest_turn: Option<ReplayedTurn>,
    latest_query_usage: Option<devo_protocol::native::usage::TurnUsage>,
    latest_context_occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
    turns_by_id: HashMap<NativeTurnId, ReplayedTurn>,
    loaded_item_count: u64,
    next_item_seq: u64,
    turns_seen: u32,
    total_input_tokens: usize,
    total_output_tokens: usize,
    total_tokens: usize,
    total_cache_creation_tokens: usize,
    total_cache_read_tokens: usize,
    last_input_tokens: usize,
    last_turn_tokens: usize,
    session_context: Option<devo_core::SessionContext>,
    /// Session context loaded from a dedicated `SessionContextUpdated` rollout line.
    /// Preserved across rollback because that line is session-scoped, not turn-scoped.
    recorded_session_context: Option<devo_core::SessionContext>,
    latest_turn_context: Option<devo_core::TurnContext>,
    session_context_recorded: bool,
    turn_kinds_by_id: HashMap<TurnId, TurnKind>,
    messages: Vec<Message>,
    history_items: Vec<crate::SessionHistoryEntry>,
    pending_items: Vec<ReplayHistoryItem>,
    latest_compaction_snapshot: Option<CompactionSnapshotLine>,
    turn_order: Vec<TurnId>,
    superseded_turn_ids: HashSet<TurnId>,
    summarized_turn_ids: HashSet<TurnId>,
    last_activity_at: Option<chrono::DateTime<Utc>>,
    /// Field-level session settings accumulated during replay (L2-DES-CONV-002
    /// Phase 1). The last line per field wins; a `PermissionPreset` line
    /// clears the explicit `SandboxProfile` override (the preset re-implies
    /// the sandbox), matching the approved patch-interaction rule.
    session_settings: HashMap<SessionSettingsField, serde_json::Value>,
    /// Transcript-tree tip folded from `SessionLeaf` lines.
    transcript_leaf_id: Option<devo_protocol::native::ids::ItemId>,
    leaf_epoch: u64,
    tree_edges:
        HashMap<devo_protocol::native::ids::ItemId, Option<devo_protocol::native::ids::ItemId>>,
}

/// Auto-compact / status pressure restored on resume.
///
/// Prefers post-compaction (or tip) occupancy so a prior large query total cannot
/// re-trigger compaction after the context was already reduced. Falls back to
/// latest-query display total, then the reconstituted prompt estimate.
fn resume_context_pressure_tokens(
    occupancy: Option<&devo_protocol::native::item::ContextOccupancy>,
    latest_query_usage: Option<&devo_protocol::native::usage::TurnUsage>,
    prompt_token_estimate: usize,
) -> (usize, usize) {
    let last_turn_tokens = occupancy
        .map(|occupancy| occupancy.total_tokens as usize)
        .or_else(|| latest_query_usage.map(|usage| usage.display_total_tokens() as usize))
        .unwrap_or(prompt_token_estimate);
    let last_input_tokens = latest_query_usage
        .map(|usage| usage.query.input_tokens as usize)
        .or_else(|| occupancy.map(|occupancy| occupancy.total_tokens as usize))
        .unwrap_or(prompt_token_estimate);
    (last_turn_tokens, last_input_tokens)
}

fn parse_optional_string_setting(
    session_id: SessionId,
    field: &str,
    value: serde_json::Value,
) -> Option<Option<String>> {
    match serde_json::from_value(value) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            tracing::warn!(session_id = %session_id, field, %error, "ignoring damaged settings line");
            None
        }
    }
}

fn warn_settings_disagree(session_id: SessionId, field: &str, disagreed: bool) {
    if disagreed {
        tracing::warn!(
            session_id = %session_id,
            field,
            "settings field line disagrees with SessionMeta value; field line wins"
        );
    }
}

impl ReplayState {
    fn apply_line(&mut self, line: LegacyRolloutLine) -> Result<()> {
        match line {
            // Legacy-shaped Session/Turn lines (unit tests) convert once via
            // persistence-boundary helpers, then share the Native apply path.
            LegacyRolloutLine::SessionMeta(line) => {
                let agent_path = line.session.agent_path.clone();
                let agent_nickname = line.session.agent_nickname.clone();
                let v2 = devo_core::session_line_from_record(&line.session, line.timestamp)?;
                self.apply_rollout_line(v2)?;
                if let Some(session) = self.session.as_mut() {
                    session.agent_path = agent_path;
                    session.agent_nickname = agent_nickname;
                }
                Ok(())
            }
            LegacyRolloutLine::Turn(line) => {
                let v2 = devo_core::turn_line_from_record(&line.turn, line.timestamp)?;
                self.apply_rollout_line(v2)
            }
            LegacyRolloutLine::Item(line) => self.apply_item_record(line.item),
            LegacyRolloutLine::SessionTitleUpdated(line) => {
                let session = self
                    .session
                    .as_mut()
                    .context("title update without session header")?;
                session.native.title = Some(line.title);
                session.native.title_state = line.title_state;
                session.updated_at = line.timestamp;
                Ok(())
            }
            LegacyRolloutLine::SessionContextUpdated(line) => {
                let line = *line;
                if let Some(session) =
                    self.matching_session_mut(line.session_id, "session context update")?
                {
                    session.updated_at = line.timestamp;
                }
                self.recorded_session_context = Some(line.session_context.clone());
                self.session_context = Some(line.session_context);
                self.session_context_recorded = true;
                Ok(())
            }
            LegacyRolloutLine::CompactionSnapshot(line) => {
                if let Some(occupancy) = line.context_occupancy.clone() {
                    self.latest_context_occupancy = Some(occupancy);
                }
                self.latest_compaction_snapshot = Some(*line);
                Ok(())
            }
            LegacyRolloutLine::MessageEditRecorded(line) => self.apply_timestamped_session_record(
                line.record.session_id,
                line.timestamp,
                "message edit line",
            ),
            LegacyRolloutLine::TurnSuperseded(line) => {
                self.apply_timestamped_session_record(
                    line.record.session_id,
                    line.timestamp,
                    "turn superseded line",
                )?;
                self.apply_turn_superseded(line.record);
                Ok(())
            }
            LegacyRolloutLine::TurnWorkspaceCheckpointRecorded(line) => self
                .apply_timestamped_session_record(
                    line.record.session_id,
                    line.timestamp,
                    "workspace checkpoint line",
                ),
            LegacyRolloutLine::TurnWorkspaceChangeRecorded(line) => self
                .apply_timestamped_session_record(
                    line.record.session_id,
                    line.timestamp,
                    "workspace change line",
                ),
            LegacyRolloutLine::TurnWorkspaceRestoreStarted(line) => self
                .apply_timestamped_session_record(
                    line.record.session_id,
                    line.timestamp,
                    "workspace restore started line",
                ),
            LegacyRolloutLine::TurnWorkspaceRestoreCompleted(line) => self
                .apply_timestamped_session_record(
                    line.record.session_id,
                    line.timestamp,
                    "workspace restore completed line",
                ),
            LegacyRolloutLine::SessionRollback(line) => self.apply_session_rollback(*line),
            LegacyRolloutLine::SessionSettings(line) => {
                // Approved patch-interaction rule: a preset change re-implies
                // the sandbox, so it clears any explicit override seen so far.
                if line.field == SessionSettingsField::PermissionPreset {
                    self.session_settings
                        .remove(&SessionSettingsField::SandboxProfile);
                }
                self.session_settings.insert(line.field, line.value);
                Ok(())
            }
        }
    }

    fn apply_native_session(
        &mut self,
        session: devo_protocol::native::session::Session,
        extras: Option<SessionPersistenceExtras>,
        updated_at: chrono::DateTime<Utc>,
    ) {
        let mut replayed = ReplayedSession::from_native(session, extras, updated_at);
        if replayed.native.last_activity_at == chrono::DateTime::<Utc>::UNIX_EPOCH {
            replayed.native.last_activity_at = replayed.native.created_at;
        }
        self.last_activity_at = Some(replayed.native.last_activity_at);
        if let Some(context) = replayed.extras.session_context.clone() {
            self.session_context = Some(context);
            self.session_context_recorded = true;
        }
        self.session = Some(replayed);
    }

    fn apply_native_turn(
        &mut self,
        turn: devo_protocol::native::turn::Turn,
        extras: Option<TurnPersistenceExtras>,
    ) -> Result<()> {
        let replayed = ReplayedTurn::from_native(turn, extras);
        let turn_id = replayed.legacy_turn_id();

        // Insert turn summary for the previous turn before processing the new turn.
        if let Some(prev_turn) = self.latest_turn.clone() {
            self.enqueue_terminal_history_items(&prev_turn);
        }

        if self.superseded_turn_ids.contains(&turn_id) {
            return Ok(());
        }
        let session_id: SessionId = replayed
            .native
            .session_id
            .as_str()
            .parse()
            .map_err(|error| anyhow::anyhow!("invalid turn session id: {error}"))?;
        self.apply_activity_timestamp(
            session_id,
            replayed
                .native
                .completed_at
                .unwrap_or(replayed.native.started_at),
            "turn line",
        )?;
        if !self.turns_by_id.contains_key(&turn_id) {
            self.turn_order.push(turn_id);
        }
        self.turns_seen = self.turns_seen.max(replayed.native.sequence);
        self.apply_turn_usage(&replayed);
        self.turn_kinds_by_id.insert(turn_id, replayed.native.kind);
        if let Some(session_context) = replayed.extras.session_context.clone() {
            self.session_context = Some(session_context);
            self.session_context_recorded = true;
        }
        if let Some(turn_context) = replayed.extras.turn_context.clone() {
            self.latest_turn_context = Some(turn_context);
        }
        self.turns_by_id.insert(turn_id, replayed.clone());
        self.latest_turn = Some(replayed);
        Ok(())
    }

    fn apply_item_record(&mut self, item: ItemRecord) -> Result<()> {
        if !self.superseded_turn_ids.contains(&item.turn_id) {
            self.apply_activity_timestamp(item.session_id, item.timestamp, "item line")?;
            self.loaded_item_count += 1;
            self.next_item_seq = self.next_item_seq.max(item.seq + 1);
            self.collect_item_line(item);
        }
        Ok(())
    }

    fn apply_native_item_envelope(&mut self, envelope: ItemEnvelope) -> Result<()> {
        let turn_id = envelope.turn_id;
        let legacy_turn_id = turn_id;
        if self.superseded_turn_ids.contains(&legacy_turn_id) {
            return Ok(());
        }
        let session_id: SessionId = envelope
            .session_id
            .as_str()
            .parse()
            .map_err(|error| anyhow::anyhow!("invalid session id on item envelope: {error}"))?;
        let item_id = envelope.id;
        self.apply_activity_timestamp(session_id, envelope.updated_at, "item envelope")?;
        self.loaded_item_count += 1;
        self.next_item_seq = self.next_item_seq.max(envelope.seq + 1);
        let turn_kind = self
            .turn_kinds_by_id
            .get(&legacy_turn_id)
            .cloned()
            .unwrap_or_default();
        self.pending_items.push(ReplayHistoryItem {
            turn_id,
            turn_kind,
            item_id,
            seq: envelope.seq,
            timestamp: envelope.updated_at,
            record_timestamp: envelope.created_at,
            line_timestamp: envelope.updated_at,
            bucket_priority: 0,
            intra_record_order: 0,
            payload: ReplayHistoryItemPayload::NativeItem(envelope.item),
        });
        Ok(())
    }

    /// Applies one v2 rollout line. Native Session/Turn/Item update replay state
    /// directly; compatibility-only kinds still adapt to frozen legacy records.
    fn apply_rollout_line(&mut self, line: RolloutLine) -> Result<()> {
        match line {
            RolloutLine::SessionMeta {
                timestamp,
                session,
                extras,
                ..
            } => {
                self.apply_native_session(*session, extras.map(|extras| *extras), timestamp);
                Ok(())
            }
            RolloutLine::Turn { turn, extras, .. } => {
                self.apply_native_turn(turn, extras.map(|extras| *extras))
            }
            RolloutLine::Item { item, .. } => {
                self.apply_native_item_envelope(item)?;
                Ok(())
            }
            RolloutLine::WorkspaceCheckpoint {
                timestamp, record, ..
            } => self.apply_line(LegacyRolloutLine::TurnWorkspaceCheckpointRecorded(
                Box::new(TurnWorkspaceCheckpointRecordedLine { timestamp, record }),
            )),
            RolloutLine::WorkspaceChange {
                timestamp, record, ..
            } => self.apply_line(LegacyRolloutLine::TurnWorkspaceChangeRecorded(Box::new(
                TurnWorkspaceChangeRecordedLine { timestamp, record },
            ))),
            RolloutLine::WorkspaceRestoreStarted {
                timestamp, record, ..
            } => self.apply_line(LegacyRolloutLine::TurnWorkspaceRestoreStarted(Box::new(
                TurnWorkspaceRestoreStartedLine { timestamp, record },
            ))),
            RolloutLine::WorkspaceRestoreCompleted {
                timestamp, record, ..
            } => self.apply_line(LegacyRolloutLine::TurnWorkspaceRestoreCompleted(Box::new(
                TurnWorkspaceRestoreCompletedLine { timestamp, record },
            ))),
            RolloutLine::Internal {
                timestamp,
                session_id,
                turn_id,
                seq,
                entry,
                ..
            } => {
                match &entry {
                    InternalRecord::SessionLeaf { epoch, leaf_id } => {
                        if *epoch >= self.leaf_epoch {
                            self.leaf_epoch = *epoch;
                            self.transcript_leaf_id = *leaf_id;
                        }
                    }
                    InternalRecord::TreeEdge {
                        child_id,
                        parent_id,
                    } => {
                        self.tree_edges.insert(*child_id, *parent_id);
                    }
                    _ => {}
                }
                for legacy in legacy_lines_from_internal(
                    timestamp,
                    &session_id,
                    turn_id.as_ref(),
                    seq,
                    &entry,
                )? {
                    self.apply_line(legacy)?;
                }
                Ok(())
            }
            RolloutLine::SessionTitleUpdated {
                timestamp,
                session_id,
                title,
                previous_title,
                ..
            } => self.apply_line(legacy_title_line_from_native(
                timestamp,
                &session_id,
                title,
                previous_title,
            )?),
            RolloutLine::CompactionSnapshot {
                timestamp,
                session_id,
                turn_id,
                summary_item_id,
                preserved_item_ids,
                context_occupancy,
                ..
            } => self.apply_line(legacy_compaction_line_from_native(
                timestamp,
                &session_id,
                &turn_id,
                &summary_item_id,
                &preserved_item_ids,
                context_occupancy,
            )?),
            RolloutLine::SessionRollback {
                timestamp,
                session_id,
                retained_turn_ids,
                retained_item_ids,
                latest_turn_id,
                ..
            } => self.apply_line(legacy_rollback_line_from_native(
                timestamp,
                &session_id,
                &retained_turn_ids,
                &retained_item_ids,
                latest_turn_id.as_ref(),
            )?),
        }
    }

    /// Applies accumulated field-level settings onto the replayed Native
    /// session, ahead of the derivations in `into_runtime_session`. Field lines
    /// win over the whole-record `SessionMeta` values; a disagreement between
    /// the two is logged because it indicates a missed dual-write.
    fn apply_session_settings(&mut self, session: &mut ReplayedSession) {
        let fields = std::mem::take(&mut self.session_settings);
        let session_id = session.legacy_session_id();
        for (field, value) in fields {
            match field {
                SessionSettingsField::PermissionPreset => {
                    match serde_json::from_value::<devo_protocol::PermissionPreset>(value) {
                        Ok(preset) => {
                            let profile = match preset {
                                devo_protocol::PermissionPreset::Default => {
                                    devo_protocol::native::model::PermissionProfile::Default
                                }
                                devo_protocol::PermissionPreset::AutoReview => {
                                    devo_protocol::native::model::PermissionProfile::AutoReview
                                }
                                devo_protocol::PermissionPreset::FullAccess => {
                                    devo_protocol::native::model::PermissionProfile::FullAccess
                                }
                            };
                            if session
                                .extras
                                .permission_preset
                                .is_some_and(|p| p != preset)
                                || session.native.settings.permission_profile != profile
                            {
                                tracing::warn!(
                                    session_id = %session_id,
                                    "settings field line disagrees with SessionMeta permission_preset; field line wins"
                                );
                            }
                            session.extras.permission_preset = Some(preset);
                            session.native.settings.permission_profile = profile;
                        }
                        Err(error) => {
                            tracing::warn!(session_id = %session_id, %error, "ignoring damaged permissionPreset settings line");
                        }
                    }
                }
                SessionSettingsField::Model => {
                    let Some(parsed) = parse_optional_string_setting(session_id, "model", value)
                    else {
                        continue;
                    };
                    warn_settings_disagree(
                        session_id,
                        "model",
                        (!session.native.model.model.is_empty())
                            .then(|| session.native.model.model.clone())
                            != parsed,
                    );
                    session.native.model.model = parsed.unwrap_or_default();
                }
                SessionSettingsField::ModelBindingId => {
                    let Some(parsed) =
                        parse_optional_string_setting(session_id, "model_binding_id", value)
                    else {
                        continue;
                    };
                    warn_settings_disagree(
                        session_id,
                        "model_binding_id",
                        (session.native.model.provider != "unknown"
                            && !session.native.model.provider.is_empty())
                        .then(|| session.native.model.provider.clone())
                            != parsed,
                    );
                    session.native.model.provider = parsed.unwrap_or_else(|| "unknown".to_string());
                }
                SessionSettingsField::ReasoningEffortSelection => {
                    let Some(parsed) = parse_optional_string_setting(
                        session_id,
                        "reasoning_effort_selection",
                        value,
                    ) else {
                        continue;
                    };
                    warn_settings_disagree(
                        session_id,
                        "reasoning_effort_selection",
                        session.native.settings.reasoning_effort != parsed,
                    );
                    session.native.settings.reasoning_effort = parsed.clone();
                    session.native.model.reasoning_effort = parsed
                        .as_deref()
                        .and_then(|selection| selection.parse().ok());
                }
                SessionSettingsField::CollaborationMode => {
                    match serde_json::from_value::<devo_protocol::CollaborationMode>(value) {
                        Ok(mode) => {
                            if session.extras.collaboration_mode.is_some_and(|m| m != mode) {
                                tracing::warn!(
                                    session_id = %session_id,
                                    "settings field line disagrees with SessionMeta collaboration_mode; field line wins"
                                );
                            }
                            session.extras.collaboration_mode = Some(mode);
                            session.native.settings.mode = Some(match mode {
                                devo_protocol::CollaborationMode::Build => "build".to_string(),
                                devo_protocol::CollaborationMode::Plan => "plan".to_string(),
                            });
                        }
                        Err(error) => {
                            tracing::warn!(session_id = %session_id, %error, "ignoring damaged collaborationMode settings line");
                        }
                    }
                }
                // Applied to `core_session.config` after the preset
                // derivation, not to the record (the record has no sandbox
                // profile name field).
                SessionSettingsField::SandboxProfile => {
                    self.session_settings.insert(field, value);
                }
                // Auto-refine and Python wait budget live on the Native session snapshot.
                SessionSettingsField::AutoRefineEnabled => {
                    if let Ok(enabled) = serde_json::from_value::<bool>(value) {
                        session.native.settings.auto_refine_enabled = Some(enabled);
                    }
                }
                SessionSettingsField::AutoRefineTurnInterval => {
                    if let Ok(interval) = serde_json::from_value::<u32>(value) {
                        session.native.settings.auto_refine_turn_interval = Some(interval.max(1));
                    }
                }
                SessionSettingsField::PythonCellFirstWaitMs => {
                    if let Ok(ms) = serde_json::from_value::<u64>(value) {
                        session.native.settings.python_cell_first_wait_ms = Some(ms);
                    }
                }
            }
        }
    }

    /// Returns the explicit sandbox profile override accumulated from settings
    /// field lines, if any survived preset re-derivation.
    fn sandbox_profile_override(&self) -> Option<String> {
        self.session_settings
            .get(&SessionSettingsField::SandboxProfile)
            .and_then(
                |value| match serde_json::from_value::<String>(value.clone()) {
                    Ok(name) => Some(name),
                    Err(error) => {
                        tracing::warn!(%error, "ignoring damaged sandboxProfile settings line");
                        None
                    }
                },
            )
    }

    async fn into_runtime_session(
        mut self,
        deps: &ServerRuntimeDependencies,
    ) -> Result<RuntimeSession> {
        // Insert turn summary for the last turn before converting
        if let Some(last_turn) = self.latest_turn.clone() {
            self.enqueue_terminal_history_items(&last_turn);
        }

        let mut replayed = self
            .session
            .take()
            .context("missing SessionMetaLine in rollout")?;
        let last_activity_at = self
            .last_activity_at
            .unwrap_or(replayed.native.last_activity_at);
        replayed.native.last_activity_at = last_activity_at;
        // Field-level settings lines win over the whole-record SessionMeta
        // values (L2-DES-CONV-002 Phase 1); apply before the derivations below.
        let has_model_setting = self
            .session_settings
            .contains_key(&SessionSettingsField::Model);
        let has_model_binding_setting = self
            .session_settings
            .contains_key(&SessionSettingsField::ModelBindingId);
        self.apply_session_settings(&mut replayed);
        let sandbox_profile_override = self.sandbox_profile_override();
        let session_id = replayed.legacy_session_id();
        let runtime_context = deps.context_for_workspace(&replayed.native.cwd).await?;
        let mut core_session = runtime_context.new_session_state(
            session_id,
            replayed.native.cwd.clone(),
            replayed.native.additional_directories.clone(),
        );
        let mut ordered_items = self.pending_items;
        ordered_items.sort_by(|left, right| {
            left.seq
                .cmp(&right.seq)
                .then_with(|| left.timestamp.cmp(&right.timestamp))
                .then_with(|| left.record_timestamp.cmp(&right.record_timestamp))
                .then_with(|| left.line_timestamp.cmp(&right.line_timestamp))
                .then_with(|| left.bucket_priority.cmp(&right.bucket_priority))
                .then_with(|| left.intra_record_order.cmp(&right.intra_record_order))
        });

        let mut replayed_messages = self.messages;
        let mut replayed_history_items = self.history_items;
        let mut replayed_persisted_turn_items = Vec::with_capacity(ordered_items.len());
        let mut tool_names_by_id = HashMap::new();
        for pending_item in ordered_items {
            match pending_item.payload {
                ReplayHistoryItemPayload::NativeItem(native_item) => {
                    apply_native_item(
                        &mut replayed_messages,
                        &mut replayed_history_items,
                        &mut tool_names_by_id,
                        native_item.clone(),
                    );
                    replayed_persisted_turn_items.push(PersistedNativeItem::new(
                        pending_item.turn_id,
                        pending_item.turn_kind,
                        pending_item.item_id,
                        native_item,
                    ));
                }
                ReplayHistoryItemPayload::HistoryOnly(history_item) => {
                    replayed_history_items.push(history_item);
                }
            }
        }

        core_session.messages = replayed_messages;
        core_session.prompt_messages =
            self.latest_compaction_snapshot
                .as_ref()
                .and_then(|snapshot| {
                    build_prompt_messages_from_snapshot(&replayed_persisted_turn_items, snapshot)
                });
        core_session.session_context = self
            .session_context
            .or_else(|| replayed.extras.session_context.clone());
        // Skills are environment state, not conversation content: a restart
        // must re-advertise what is actually on disk (bundle updates, user
        // edits). Without this, a session restored from its rollout keeps
        // advertising the skills snapshot captured when it was created —
        // forever, across every restart. Refresh only the skills block from
        // the freshly built session config; the rest of the snapshot stays
        // locked per session (prompt-cache stability).
        if let Some(context) = core_session.session_context.as_mut() {
            context.available_skills = core_session.config.available_skills_instructions.clone();
        }
        core_session.latest_turn_context = self.latest_turn_context.clone();
        if let Some(latest_turn_context) = core_session.latest_turn_context.as_ref() {
            core_session.collaboration_mode = latest_turn_context.collaboration_mode;
        }
        if let Some(mode) = replayed.extras.collaboration_mode {
            core_session.collaboration_mode = mode;
        }
        if let Some(preset) = replayed.extras.permission_preset {
            let safety_preset = match preset {
                devo_protocol::PermissionPreset::Default => devo_safety::PermissionPreset::Default,
                devo_protocol::PermissionPreset::AutoReview => {
                    devo_safety::PermissionPreset::AutoReview
                }
                devo_protocol::PermissionPreset::FullAccess => {
                    devo_safety::PermissionPreset::FullAccess
                }
            };
            let profile = devo_safety::RuntimePermissionProfile::from_preset(
                safety_preset,
                replayed.native.cwd.clone(),
            )
            .with_additional_roots(replayed.native.additional_directories.clone());
            let sandbox = Some(profile.implied_sandbox_profile().to_string());
            core_session.config.permission_mode = profile.permission_mode();
            core_session.config.permission_profile = profile;
            core_session.config.sandbox_profile = sandbox;
        }
        // An explicit sandbox override from settings field lines wins over the
        // preset-implied sandbox (approved patch-interaction rule).
        if let Some(sandbox) = sandbox_profile_override {
            core_session.config.sandbox_profile = Some(sandbox);
        }
        core_session.turn_count = self.turns_seen as usize;
        core_session.total_input_tokens = self.total_input_tokens;
        core_session.total_output_tokens = self.total_output_tokens;
        core_session.total_tokens = self.total_tokens;
        core_session.total_cache_creation_tokens = self.total_cache_creation_tokens;
        core_session.total_cache_read_tokens = self.total_cache_read_tokens;
        let prompt_bytes = core_session
            .prompt_source_messages()
            .iter()
            .map(|message| serde_json::to_string(message).map_or(0, |json| json.len()))
            .sum::<usize>();
        core_session.prompt_token_estimate =
            devo_protocol::approx_tokens_from_byte_count(prompt_bytes)
                .try_into()
                .unwrap_or(usize::MAX);
        let (last_turn_tokens, last_input_tokens) = resume_context_pressure_tokens(
            self.latest_context_occupancy.as_ref(),
            self.latest_query_usage.as_ref(),
            core_session.prompt_token_estimate,
        );
        core_session.last_input_tokens = last_input_tokens;
        core_session.last_turn_tokens = last_turn_tokens;
        let pending_turn_queue = std::sync::Arc::clone(&core_session.pending_turn_queue);
        let steer_input_queue = std::sync::Arc::clone(&core_session.steer_input_queue);

        let session_model =
            (!replayed.native.model.model.is_empty()).then(|| replayed.native.model.model.clone());
        let session_binding = (replayed.native.model.provider != "unknown"
            && !replayed.native.model.provider.is_empty())
        .then(|| replayed.native.model.provider.clone());
        // A session-level model update supersedes the binding captured by an
        // older turn. With no explicit binding line, use the slug directly;
        // this also repairs rollouts written before slug updates cleared the
        // stale binding. When a binding line is present, its value remains
        // authoritative (including an explicit null, which falls back to the
        // paired model slug).
        let summary_model_selection = if has_model_setting || has_model_binding_setting {
            if has_model_binding_setting {
                session_binding.clone().or_else(|| session_model.clone())
            } else {
                session_model.clone()
            }
        } else {
            self.latest_turn
                .as_ref()
                .and_then(|turn| {
                    (turn.native.model.provider != "unknown")
                        .then(|| turn.native.model.provider.clone())
                })
                .or_else(|| {
                    self.latest_turn
                        .as_ref()
                        .map(|turn| turn.native.model.model.clone())
                })
                .or_else(|| session_binding.clone())
                .or_else(|| session_model.clone())
        }
        .unwrap_or_else(|| runtime_context.default_model.clone());
        let turn_config = runtime_context.resolve_turn_config(Some(&summary_model_selection), None);
        let concrete_selection = |selection: Option<&str>| {
            selection
                .map(str::trim)
                .filter(|selection| !selection.is_empty())
                .filter(|selection| !selection.eq_ignore_ascii_case("default"))
                .map(str::to_ascii_lowercase)
        };
        let latest_turn_effort = self.latest_turn.as_ref().and_then(|turn| {
            turn.native
                .model
                .reasoning_effort
                .map(|effort| effort.to_string())
                .or_else(|| {
                    turn.extras.turn_context.as_ref().and_then(|ctx| {
                        ctx.reasoning_effort
                            .map(|effort| effort.label().to_lowercase())
                    })
                })
        });
        let explicit_reasoning_effort_selection = concrete_selection(latest_turn_effort.as_deref())
            .or_else(|| concrete_selection(replayed.native.settings.reasoning_effort.as_deref()));
        let context_reasoning_effort_selection = core_session
            .latest_turn_context
            .as_ref()
            .and_then(|context| context.reasoning_effort)
            .or_else(|| {
                core_session
                    .session_context
                    .as_ref()
                    .and_then(|context| context.reasoning_effort)
            })
            .map(|effort| effort.label().to_lowercase());
        let summary_reasoning_effort_selection =
            turn_config.model.normalize_reasoning_effort_selection(
                explicit_reasoning_effort_selection
                    .as_deref()
                    .or(context_reasoning_effort_selection.as_deref()),
            );
        let summary_reasoning_effort = turn_config
            .model
            .resolve_reasoning_effort_selection(summary_reasoning_effort_selection.as_deref())
            .effective_reasoning_effort;

        let applied_compaction_limit =
            crate::runtime::context_occupancy::resolved_compaction_limit(&turn_config.model);
        // Apply before wrapping in Mutex so resume never needs to lock a
        // single-owner Arc that `from_runtime_session` later unwraps.
        crate::runtime::context_occupancy::apply_resolved_compaction_limit(
            &mut core_session.config,
            applied_compaction_limit as usize,
        );

        use devo_protocol::native::session::SessionStatus;
        use devo_protocol::native::usage::{SessionUsage, UsageTotals};

        let last_query_total_tokens = self
            .latest_context_occupancy
            .as_ref()
            .map(|occupancy| occupancy.total_tokens as usize)
            .or_else(|| {
                self.latest_query_usage
                    .as_ref()
                    .map(|usage| usage.display_total_tokens() as usize)
            })
            .unwrap_or(0);

        // Native session is the hydrate source of truth; overlay resolved model
        // / idle status / usage totals for the actor summary.
        let mut native = replayed.native.clone();
        native.status = SessionStatus::Idle;
        native.active_turn_id = None;
        native.queued_count = 0;
        native.last_activity_at = last_activity_at;
        // The actor's next turn resolves its selection from these strings.
        // Keeping only the base binding silently drops a named variant after
        // restart even when the display-only `variant` field still says fast.
        let persisted_selection = turn_config.variant.as_ref().map(|variant| {
            let base = turn_config
                .model_binding_id
                .as_ref()
                .unwrap_or(&turn_config.model.slug);
            format!("{base}/{variant}")
        });
        native.model.provider = turn_config
            .model_binding_id
            .map(|binding| persisted_selection.clone().unwrap_or(binding))
            .unwrap_or_else(|| "unknown".into());
        native.model.model = persisted_selection.unwrap_or_else(|| turn_config.model.slug.clone());
        native.model.variant = turn_config.variant.clone();
        native.model.reasoning_effort = summary_reasoning_effort;
        native.settings.reasoning_effort = summary_reasoning_effort_selection.clone();
        native.settings.mode = Some(match core_session.collaboration_mode {
            devo_protocol::CollaborationMode::Build => "build".to_string(),
            devo_protocol::CollaborationMode::Plan => "plan".to_string(),
        });
        native.settings.effective_context_window = Some(applied_compaction_limit);
        native.usage = SessionUsage {
            total: UsageTotals {
                unreported_fields: None,
                total_tokens: self.total_tokens as u64,
                input_tokens: self.total_input_tokens as u64,
                output_tokens: self.total_output_tokens as u64,
                reasoning_tokens: 0,
                cache_read_input_tokens: self.total_cache_read_tokens as u64,
                cache_creation_input_tokens: self.total_cache_creation_tokens as u64,
                call_count: 0,
                metered_call_count: 0,
                failed_call_count: 0,
                cancelled_call_count: 0,
                estimated_cost: None,
            },
            by_purpose: Vec::new(),
            legacy: None,
            updated_at: replayed.updated_at,
        };
        native.sync_activity();

        let summary = crate::runtime_session_summary::RuntimeSessionSummary {
            native: native.clone(),
            updated_at: replayed.updated_at,
            agent_path: replayed.agent_path.clone(),
            agent_nickname: replayed.agent_nickname.clone(),
            agent_role: replayed.agent_role(),
            session_dir: None,
            prompt_token_estimate: core_session.prompt_token_estimate,
            last_query_usage: self.latest_query_usage.clone(),
            last_query_total_tokens,
            last_context_occupancy: self.latest_context_occupancy.clone(),
            collaboration_mode: core_session.collaboration_mode,
        };

        // Native turn map is the resume/fork source of truth — no TurnRecord bridge.
        let turns_by_id = self.turns_by_id;

        let config = core_session.config.clone();
        Ok(RuntimeSession {
            runtime_context,
            // Caller (`load_session_from_rollout`) fills the absolute path.
            rollout_path: None,
            summary,
            config,
            core_session: std::sync::Arc::new(Mutex::new(core_session)),
            active_turn: None,
            latest_turn: self.latest_turn.map(ReplayedTurn::into_runtime_turn),
            loaded_item_count: self.loaded_item_count,
            history_items: replayed_history_items,
            persisted_turn_items: replayed_persisted_turn_items,
            latest_compaction_snapshot: self.latest_compaction_snapshot,
            turns_by_id,
            pending_turn_queue,
            steer_input_queue,
            agent_tool_policy: Default::default(),
            max_turns: None,
            deferred_assistant: None,
            deferred_reasoning: None,
            next_item_seq: self.next_item_seq.max(1),
            first_user_input: None,
            tool_registry: None,
            file_read_ledger: std::sync::Arc::new(devo_core::tools::FileReadLedger::new()),
            session_approval_cache: crate::execution::ApprovalGrantCache::default(),
            turn_approval_cache: crate::execution::ApprovalGrantCache::default(),
            session_context_recorded: self.session_context_recorded,
        })
    }

    fn apply_timestamped_session_record(
        &mut self,
        session_id: SessionId,
        timestamp: chrono::DateTime<Utc>,
        line_kind: &str,
    ) -> Result<()> {
        self.apply_record_timestamp(session_id, timestamp, line_kind)?;
        self.apply_activity_timestamp(session_id, timestamp, line_kind)
    }

    fn matching_session_mut(
        &mut self,
        session_id: SessionId,
        line_kind: &str,
    ) -> Result<Option<&mut ReplayedSession>> {
        match self.session.as_mut() {
            Some(session) if session.legacy_session_id() != session_id => {
                anyhow::bail!("{line_kind} session id does not match session header")
            }
            Some(session) => Ok(Some(session)),
            None => Ok(None),
        }
    }

    fn apply_record_timestamp(
        &mut self,
        session_id: SessionId,
        timestamp: chrono::DateTime<Utc>,
        line_kind: &str,
    ) -> Result<()> {
        if let Some(session) = self.matching_session_mut(session_id, line_kind)? {
            session.updated_at = timestamp;
        }
        Ok(())
    }

    fn apply_activity_timestamp(
        &mut self,
        session_id: SessionId,
        timestamp: chrono::DateTime<Utc>,
        line_kind: &str,
    ) -> Result<()> {
        if self.matching_session_mut(session_id, line_kind)?.is_none() {
            return Ok(());
        }
        let last_activity_at = self
            .last_activity_at
            .map(|current| current.max(timestamp))
            .unwrap_or(timestamp);
        self.last_activity_at = Some(last_activity_at);
        if let Some(session) = self.session.as_mut() {
            session.native.last_activity_at = last_activity_at;
        }
        Ok(())
    }

    fn apply_turn_usage(&mut self, turn: &ReplayedTurn) {
        if let Some(usage) = turn.native.usage.as_ref() {
            self.total_input_tokens += usage.query.input_tokens as usize;
            self.total_output_tokens += usage.query.output_tokens as usize;
            self.total_tokens += usage.query.total_tokens as usize;
            self.total_cache_creation_tokens += usage.query.cache_creation_input_tokens as usize;
            self.total_cache_read_tokens += usage.query.cache_read_input_tokens as usize;
        }
        match &turn.extras.latest_query_usage {
            Some(usage) => {
                self.last_input_tokens = usage.query.input_tokens as usize;
                self.last_turn_tokens = usage.display_total_tokens() as usize;
                self.latest_query_usage = Some(usage.clone());
            }
            None if turn.native.usage.is_some() => {
                // Older rollout records only contain aggregate turn usage.
                // Do not mistake it for the latest model query.
                self.last_input_tokens = 0;
                self.last_turn_tokens = 0;
                self.latest_query_usage = None;
            }
            None => {}
        }
        if let Some(occupancy) = turn.extras.context_occupancy.clone() {
            self.latest_context_occupancy = Some(occupancy);
        }
    }

    fn apply_turn_superseded(&mut self, record: TurnSupersededRecord) {
        self.superseded_turn_ids.insert(record.superseded_turn_id);
        let superseded_native_turn = record.superseded_turn_id;
        let removed_item_ids = self
            .pending_items
            .iter()
            .filter(|item| item.turn_id == superseded_native_turn)
            .map(|item| item.item_id)
            .collect::<HashSet<_>>();

        self.pending_items
            .retain(|item| item.turn_id != superseded_native_turn);
        self.turn_order
            .retain(|turn_id| *turn_id != record.superseded_turn_id);
        self.turns_by_id.remove(&record.superseded_turn_id);
        self.turn_kinds_by_id.remove(&record.superseded_turn_id);

        if self
            .latest_turn
            .as_ref()
            .is_some_and(|turn| turn.legacy_turn_id() == record.superseded_turn_id)
        {
            self.latest_turn = self
                .turn_order
                .iter()
                .rev()
                .find_map(|turn_id| self.turns_by_id.get(turn_id).cloned());
        }

        if self
            .latest_compaction_snapshot
            .as_ref()
            .is_some_and(|snapshot| {
                removed_item_ids.contains(&snapshot.summary_item_id)
                    || snapshot
                        .preserved_item_ids
                        .iter()
                        .any(|item_id| removed_item_ids.contains(item_id))
            })
        {
            self.latest_compaction_snapshot = None;
        }

        self.recompute_turn_aggregates();
    }

    fn apply_session_rollback(&mut self, line: SessionRollbackLine) -> Result<()> {
        if let Some(session) = self.session.as_mut() {
            if session.legacy_session_id() != line.session_id {
                anyhow::bail!("rollback line session id does not match session header");
            }
            session.updated_at = line.timestamp;
        }
        self.apply_activity_timestamp(line.session_id, line.timestamp, "rollback line")?;

        let retained_turn_ids = line
            .retained_turn_ids
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        let retained_item_ids = line
            .retained_item_ids
            .iter()
            .cloned()
            .collect::<HashSet<_>>();

        self.pending_items.retain(|item| {
            retained_turn_ids.contains(&item.turn_id)
                && (matches!(&item.payload, ReplayHistoryItemPayload::HistoryOnly(_))
                    || retained_item_ids.contains(&item.item_id))
        });
        self.turn_order
            .retain(|turn_id| retained_turn_ids.contains(turn_id));
        self.turns_by_id
            .retain(|turn_id, _| retained_turn_ids.contains(turn_id));
        self.turn_kinds_by_id
            .retain(|turn_id, _| retained_turn_ids.contains(turn_id));
        self.superseded_turn_ids
            .retain(|turn_id| retained_turn_ids.contains(turn_id));
        self.summarized_turn_ids
            .retain(|turn_id| retained_turn_ids.contains(turn_id));

        self.latest_turn = line
            .latest_turn_id
            .and_then(|turn_id| self.turns_by_id.get(&turn_id).cloned());
        // Prefer the dedicated SessionContextUpdated side-channel. Rollback prunes
        // turn records but must not drop locked session context that was recorded
        // once for the rollout file.
        self.session_context = self
            .recorded_session_context
            .clone()
            .or_else(|| {
                self.latest_turn
                    .as_ref()
                    .and_then(|turn| turn.extras.session_context.clone())
            })
            .or_else(|| {
                self.session
                    .as_ref()
                    .and_then(|session| session.extras.session_context.clone())
            });
        self.session_context_recorded = self.recorded_session_context.is_some()
            || self
                .latest_turn
                .as_ref()
                .is_some_and(|turn| turn.extras.session_context.is_some())
            || self
                .session
                .as_ref()
                .is_some_and(|session| session.extras.session_context.is_some());
        self.latest_turn_context = None;
        self.loaded_item_count = u64::try_from(retained_item_ids.len()).unwrap_or(u64::MAX);
        self.next_item_seq = self
            .pending_items
            .iter()
            .map(|item| item.seq.saturating_add(1))
            .max()
            .unwrap_or(1);
        self.recompute_turn_aggregates();

        if self
            .latest_compaction_snapshot
            .as_ref()
            .is_some_and(|snapshot| {
                !retained_item_ids.contains(&snapshot.summary_item_id)
                    || snapshot
                        .preserved_item_ids
                        .iter()
                        .any(|item_id| !retained_item_ids.contains(item_id))
            })
        {
            self.latest_compaction_snapshot = None;
        }
        Ok(())
    }

    fn recompute_turn_aggregates(&mut self) {
        self.turns_seen = 0;
        self.total_input_tokens = 0;
        self.total_output_tokens = 0;
        self.total_tokens = 0;
        self.total_cache_creation_tokens = 0;
        self.total_cache_read_tokens = 0;
        self.last_input_tokens = 0;
        self.last_turn_tokens = 0;
        self.latest_query_usage = None;
        self.latest_context_occupancy = None;

        let turns: Vec<_> = self
            .turn_order
            .iter()
            .filter_map(|turn_id| self.turns_by_id.get(turn_id).cloned())
            .collect();
        for turn in &turns {
            self.turns_seen = self.turns_seen.max(turn.native.sequence);
            self.apply_turn_usage(turn);
        }
        if let Some(occupancy) = self
            .latest_compaction_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.context_occupancy.clone())
        {
            self.latest_context_occupancy = Some(occupancy);
        }
    }

    fn collect_item_line(&mut self, item: ItemRecord) {
        let item_id = item.id;
        let record_timestamp = item.timestamp;
        let line_timestamp = record_timestamp;
        let seq = item.seq;
        let turn_kind = self
            .turn_kinds_by_id
            .get(&item.turn_id)
            .cloned()
            .unwrap_or_default();
        let mut intra_record_order = 0usize;

        for (bucket_priority, items) in [(0u8, item.output_items), (1, item.input_items)] {
            for turn_item in items {
                self.push_legacy_turn_item_payload(
                    item.turn_id,
                    turn_kind,
                    item_id,
                    seq,
                    record_timestamp,
                    line_timestamp,
                    bucket_priority,
                    &mut intra_record_order,
                    turn_item,
                );
            }
        }
    }

    /// Migrate/resume boundary: packed legacy ItemRecord payloads → Native journal.
    #[allow(clippy::too_many_arguments)]
    fn push_legacy_turn_item_payload(
        &mut self,
        turn_id: NativeTurnId,
        turn_kind: TurnKind,
        item_id: NativeItemId,
        seq: u64,
        record_timestamp: chrono::DateTime<Utc>,
        line_timestamp: chrono::DateTime<Utc>,
        bucket_priority: u8,
        intra_record_order: &mut usize,
        turn_item: TurnItem,
    ) {
        use crate::projection::history_entry_from_turn_item;

        let payload = match &turn_item {
            TurnItem::TurnSummary(_) => {
                history_entry_from_turn_item(&turn_item).map(ReplayHistoryItemPayload::HistoryOnly)
            }
            other => devo_core::native_item_from_turn_item(other)
                .map(ReplayHistoryItemPayload::NativeItem),
        };
        let Some(payload) = payload else {
            return;
        };
        self.pending_items.push(ReplayHistoryItem {
            turn_id,
            turn_kind,
            item_id,
            seq,
            timestamp: record_timestamp,
            record_timestamp,
            line_timestamp,
            bucket_priority,
            intra_record_order: *intra_record_order,
            payload,
        });
        *intra_record_order += 1;
    }

    fn enqueue_terminal_history_items(&mut self, turn: &ReplayedTurn) {
        let legacy_turn_id = turn.legacy_turn_id();
        let native_turn_id = turn.native.id;
        let status = turn.legacy_status();
        // Native Turn status/timing is the turn-summary source. Only terminal
        // failures leave a history row (as Item::Warning). Do not synthesize
        // SessionHistoryEntry::TurnSummary / ::Error on the resume path.
        if !matches!(status, TurnStatus::Failed) {
            return;
        }
        if self.superseded_turn_ids.contains(&legacy_turn_id)
            || !self.summarized_turn_ids.insert(legacy_turn_id)
        {
            return;
        }
        let Some(error) = &turn.native.error else {
            return;
        };

        let seq = self
            .pending_items
            .iter()
            .filter(|item| item.turn_id == native_turn_id)
            .map(|item| item.seq)
            .max()
            .unwrap_or(0);
        let timestamp = turn.native.completed_at.unwrap_or(turn.native.started_at);
        self.pending_items.push(ReplayHistoryItem {
            turn_id: native_turn_id,
            turn_kind: turn.native.kind,
            item_id: devo_protocol::native::ids::ItemId::new(),
            seq,
            timestamp,
            record_timestamp: timestamp,
            line_timestamp: timestamp,
            bucket_priority: 2,
            intra_record_order: 0,
            payload: ReplayHistoryItemPayload::HistoryOnly(
                crate::persisted_native_item::turn_failure_history_entry(
                    error.error_code.clone(),
                    error.message.clone(),
                ),
            ),
        });
    }
}

#[derive(Debug, Clone)]
struct ReplayHistoryItem {
    turn_id: NativeTurnId,
    turn_kind: TurnKind,
    item_id: NativeItemId,
    seq: u64,
    timestamp: chrono::DateTime<Utc>,
    record_timestamp: chrono::DateTime<Utc>,
    line_timestamp: chrono::DateTime<Utc>,
    bucket_priority: u8,
    intra_record_order: usize,
    payload: ReplayHistoryItemPayload,
}

#[derive(Debug, Clone)]
enum ReplayHistoryItemPayload {
    NativeItem(NativeItem),
    HistoryOnly(crate::SessionHistoryEntry),
}

pub(crate) fn build_prompt_messages_from_snapshot(
    persisted_turn_items: &[PersistedTurnItem],
    snapshot: &CompactionSnapshotLine,
) -> Option<Vec<Message>> {
    let ordered_items = persisted_turn_items
        .iter()
        .filter(|item| prompt_visible_persisted_item(item))
        .collect::<Vec<_>>();
    let summary_index = ordered_items.iter().position(|item| {
        item.legacy_item_id()
            .is_some_and(|id| id == snapshot.summary_item_id)
    })?;

    let mut by_item_id: HashMap<ItemId, PersistedTurnItem> = ordered_items
        .iter()
        .filter_map(|item| {
            item.legacy_item_id()
                .map(|legacy_id| (legacy_id, (*item).clone()))
        })
        .collect();

    let mut rebuilt = Vec::new();
    if let Some(summary_item) = by_item_id.remove(&snapshot.summary_item_id) {
        rebuilt.push(summary_item);
    }

    for preserved_id in &snapshot.preserved_item_ids {
        if let Some(item) = by_item_id.remove(preserved_id) {
            rebuilt.push(item);
        }
    }

    rebuilt.extend(
        ordered_items
            .iter()
            .skip(summary_index + 1)
            .filter(|item| {
                item.legacy_item_id()
                    .is_none_or(|id| id != snapshot.summary_item_id)
            })
            .filter(|item| {
                item.legacy_item_id()
                    .is_none_or(|id| !snapshot.preserved_item_ids.contains(&id))
            })
            .map(|item| (*item).clone()),
    );

    let mut messages = Vec::new();
    let mut tool_names_by_id = HashMap::new();
    for item in rebuilt {
        apply_prompt_native_item(&mut messages, &mut tool_names_by_id, item.item);
    }
    Some(messages)
}

/// Latest compaction snapshot recorded in a rollout. Live rebuilds that
/// re-scope the transcript (tree navigate) read this so the compacted prompt
/// survives the scope; a damaged or unreadable rollout yields `None` and the
/// caller falls back to the linear rebuild.
pub(crate) fn latest_compaction_snapshot_from_rollout(
    rollout_path: &Path,
) -> Option<CompactionSnapshotLine> {
    let mut latest: Option<CompactionSnapshotLine> = None;
    match visit_rollout_lines(rollout_path, "compaction snapshot scan", |line| {
        if let RolloutLine::CompactionSnapshot {
            timestamp,
            session_id,
            turn_id,
            summary_item_id,
            preserved_item_ids,
            context_occupancy,
            ..
        } = line
        {
            latest = legacy_compaction_line_from_native(
                timestamp,
                &session_id,
                &turn_id,
                &summary_item_id,
                &preserved_item_ids,
                context_occupancy,
            )
            .ok()
            .and_then(|line| match line {
                LegacyRolloutLine::CompactionSnapshot(boxed) => Some(*boxed),
                _ => None,
            });
        }
        Ok(())
    }) {
        Ok(_) => latest,
        Err(error) => {
            tracing::warn!(
                %error,
                rollout = %rollout_path.display(),
                "compaction snapshot scan failed; continuing without a snapshot"
            );
            None
        }
    }
}

pub(crate) fn prompt_visible_persisted_turn_item(item: &PersistedTurnItem) -> bool {
    prompt_visible_persisted_item(item)
}

#[cfg(test)]
use crate::projection::history_entry_from_turn_item;

/// Migrate/tests: packed [`TurnItem`] → Native prompt/history apply.
#[cfg(test)]
pub(crate) fn apply_turn_item(
    messages: &mut Vec<Message>,
    history_items: &mut Vec<crate::SessionHistoryEntry>,
    tool_names_by_id: &mut HashMap<String, String>,
    item: TurnItem,
) {
    match &item {
        TurnItem::TurnSummary(_) => {
            if let Some(history_item) = history_entry_from_turn_item(&item) {
                history_items.push(history_item);
            }
        }
        other => {
            let Some(native_item) = devo_core::native_item_from_turn_item(other) else {
                return;
            };
            apply_native_item(messages, history_items, tool_names_by_id, native_item);
        }
    }
}

/// One session recovered from a journal's `SessionMeta` and per-session
/// activity lines. A shared journal returns one entry per logical session.
pub(crate) struct IndexedJournalSession {
    pub(crate) record: SessionRecord,
    pub(crate) last_activity_at: chrono::DateTime<Utc>,
}

/// One journal scan: live session entries plus ids whose latest lifecycle
/// marker is a tombstone. Tombstoned ids stay visible to the index rebuild so
/// stale SQLite rows can be deleted.
pub(crate) struct RolloutIndexScan {
    pub(crate) sessions: Vec<IndexedJournalSession>,
    pub(crate) deleted: std::collections::HashSet<SessionId>,
}

/// Reads every logical session header in one rollout journal.
///
/// The index is a rebuildable cache, so unreadable or unmappable lines are
/// skipped. Per-session activity is computed from that session's own lines;
/// title updates fold into the matching session only.
pub(crate) fn read_rollout_index_scan(path: &Path) -> Result<RolloutIndexScan> {
    let file = File::open(path).with_context(|| format!("open rollout file {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut records: HashMap<SessionId, SessionRecord> = HashMap::new();
    let mut session_extras: HashMap<SessionId, SessionPersistenceExtras> = HashMap::new();
    let mut activity: HashMap<SessionId, chrono::DateTime<Utc>> = HashMap::new();
    let mut deleted: std::collections::HashSet<SessionId> = std::collections::HashSet::new();
    for line in reader.lines() {
        let line = line.with_context(|| format!("read line from {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(line) = parse_rollout_line(&line) else {
            continue;
        };
        let Some(session_id) = rollout_line_session_id(&line) else {
            continue;
        };
        let timestamp = rollout_line_timestamp(&line);
        activity
            .entry(session_id)
            .and_modify(|current| *current = (*current).max(timestamp))
            .or_insert(timestamp);
        match line {
            RolloutLine::SessionMeta {
                session: native,
                extras,
                ..
            } => {
                let merged =
                    merge_session_extras(session_extras.get(&session_id), extras.as_deref());
                session_extras.insert(session_id, merged.clone());
                let Ok(mut record) = session_record_from_native(&native, Some(&merged)) else {
                    continue;
                };
                if record.last_activity_at.is_none() {
                    record.last_activity_at = Some(record.created_at);
                }
                records.insert(record.id, record);
            }
            RolloutLine::SessionTitleUpdated {
                timestamp, title, ..
            } => {
                if let Some(record) = records.get_mut(&session_id) {
                    record.title = Some(title);
                    record.title_state =
                        SessionTitleState::Final(SessionTitleFinalSource::ExplicitCreate);
                    record.updated_at = timestamp;
                }
            }
            RolloutLine::Internal {
                entry: InternalRecord::SessionDeleted { .. },
                ..
            } => {
                deleted.insert(session_id);
            }
            RolloutLine::Turn { .. }
            | RolloutLine::Item { .. }
            | RolloutLine::Internal { .. }
            | RolloutLine::CompactionSnapshot { .. }
            | RolloutLine::SessionRollback { .. }
            | RolloutLine::WorkspaceCheckpoint { .. }
            | RolloutLine::WorkspaceChange { .. }
            | RolloutLine::WorkspaceRestoreStarted { .. }
            | RolloutLine::WorkspaceRestoreCompleted { .. } => {}
        }
    }

    for session_id in &deleted {
        records.remove(session_id);
    }
    let mut indexed = Vec::with_capacity(records.len());
    for (session_id, record) in records {
        let last_activity_at = activity
            .get(&session_id)
            .copied()
            .or(record.last_activity_at)
            .unwrap_or(record.created_at);
        indexed.push(IndexedJournalSession {
            record,
            last_activity_at,
        });
    }
    indexed.sort_by_key(|entry| entry.record.id.to_string());
    Ok(RolloutIndexScan {
        sessions: indexed,
        deleted,
    })
}

/// Live sessions only. Use [`read_rollout_index_scan`] when stale DB rows must
/// also be removed for tombstones.
pub(crate) fn read_rollout_index_sessions(path: &Path) -> Result<Vec<IndexedJournalSession>> {
    Ok(read_rollout_index_scan(path)?.sessions)
}

pub(crate) fn session_index_row_from_record(
    record: &SessionRecord,
    last_activity_at: chrono::DateTime<Utc>,
) -> crate::db::SessionIndexRow {
    crate::db::SessionIndexRow {
        session_id: record.id,
        cwd: record.cwd.clone(),
        additional_directories: record.additional_directories.clone(),
        created_at: record.created_at,
        updated_at: record.updated_at,
        last_activity_at,
        title: record.title.clone(),
        title_state: record.title_state.clone(),
        parent_session_id: record.parent_session_id,
        fork_from_id: record.fork_from_id,
        fork_at_turn_id: record.fork_at_turn_id,
        agent_path: record.agent_path.clone(),
        ephemeral: false,
        model: record.model.clone(),
        reasoning_effort_selection: record.reasoning_effort_selection.clone(),
    }
}

fn collect_rollout_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(root).with_context(|| format!("read dir {}", root.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", root.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type for {}", path.display()))?;
        if file_type.is_dir() {
            // `session-artifacts/<id>/harness/` holds refine sidecar state
            // (harness_state.json, refinements.jsonl), not rollouts. Skip the
            // whole directory: feeding those files to the boot-time
            // index/reconcile scans logs "missing SessionMeta" warnings on
            // every server start.
            if path.file_name().and_then(|s| s.to_str()) == Some("harness") {
                continue;
            }
            collect_rollout_files(&path, files)?;
        } else if file_type.is_file()
            && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        {
            files.push(path);
        }
    }
    Ok(())
}

/// Builds path-first turn persistence extras from Native-first runtime state.
pub(crate) fn turn_persistence_extras_from_runtime(
    turn: &crate::turn::RuntimeTurn,
    session_context: Option<devo_core::SessionContext>,
    turn_context: Option<devo_core::TurnContext>,
    latest_query_usage: Option<devo_protocol::native::usage::TurnUsage>,
    context_occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
) -> TurnPersistenceExtras {
    TurnPersistenceExtras {
        session_context,
        turn_context,
        request_thinking: turn.extras.request_thinking.clone(),
        input_token_estimate: None,
        latest_query_usage,
        context_occupancy,
        stop_reason: turn.extras.stop_reason.clone(),
        failure_reason: turn.extras.failure_reason,
    }
}

/// Creates one canonical persisted item record from a normalized turn item payload.
///
/// Migrate / fixture boundary only — live and fork writes Native ItemEnvelope.
#[cfg_attr(not(test), allow(dead_code))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_item_record(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: devo_core::ItemId,
    seq: u64,
    item: TurnItem,
    turn_status: Option<TurnStatus>,
    worklog: Option<Worklog>,
    started_at: Option<chrono::DateTime<Utc>>,
) -> ItemRecord {
    ItemRecord {
        id: item_id,
        session_id,
        turn_id,
        seq,
        timestamp: Utc::now(),
        started_at,
        attempt_placement: None,
        turn_status,
        sibling_turn_ids: Vec::new(),
        input_items: Vec::new(),
        output_items: vec![item],
        worklog,
        error: None,
        schema_version: 1,
    }
}

mod output_gc;

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "persistence/index_probe.rs"]
mod index_probe;
