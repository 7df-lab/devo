use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
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
use devo_core::read_canonical_history;
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
        kernel_snapshot_path: None,
    }
}

/// Owns canonical append-only rollout persistence rooted at the server data directory.
pub(crate) struct RolloutStore {
    /// Root data directory that contains `sessions/` and `session-artifacts/`.
    data_root: PathBuf,
    /// Per-file locks that serialise concurrent writes to the same rollout file,
    /// preventing interleaved JSON lines.
    file_locks: Arc<StdMutex<HashMap<PathBuf, Arc<StdMutex<()>>>>>,
    /// Per-file write-path state (v2 single-write, 05 §2.2). One instance per
    /// rollout path, hydrated from the on-disk history on first append so
    /// item seqs and approval folds never collide with it.
    write_states: Arc<StdMutex<HashMap<PathBuf, WritePathState>>>,
    /// Delivery-log sink (08 §5/§7): after each fsynced append, derived
    /// events are projected into the SQLite `event_log` (best effort; the
    /// startup reconciler backfills anything missed). `None` in tests that
    /// do not exercise the event log.
    event_log: Option<Arc<Database>>,
}

/// Per-file write-path state: seq/approval/settings tracking plus the index
/// of the next JSONL row to be written (used as the `source_fact_id` line
/// index).
pub(crate) struct WritePathState {
    write_state: RolloutWriteState,
    next_line_index: u64,
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
            file_locks: Arc::clone(&self.file_locks),
            write_states: Arc::clone(&self.write_states),
            event_log: self.event_log.as_ref().map(Arc::clone),
        }
    }
}

impl RolloutStore {
    /// Creates a rollout store rooted at the supplied server home directory.
    pub(crate) fn new(data_root: PathBuf, event_log: Option<Arc<Database>>) -> Self {
        Self {
            data_root,
            file_locks: Arc::new(StdMutex::new(HashMap::new())),
            write_states: Arc::new(StdMutex::new(HashMap::new())),
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

    /// Allocates a nested subagent rollout under the parent's artifact tree.
    ///
    /// Roots nest under `session-artifacts/<root_id>/sub-xxxxxxxx/<child_id>.jsonl`.
    /// Children nest further: `…/sub-xxxxxxxx/sub-yyyyyyyy/<grandchild_id>.jsonl`.
    pub(crate) fn allocate_child_rollout_path(
        &self,
        parent_rollout_path: Option<&Path>,
        parent_session_id: &devo_protocol::native::ids::SessionId,
        child_session_id: &devo_protocol::native::ids::SessionId,
    ) -> Result<PathBuf> {
        let nest_dir = self.nesting_dir_for_parent(parent_rollout_path, parent_session_id)?;
        std::fs::create_dir_all(&nest_dir)
            .with_context(|| format!("create session nest dir {}", nest_dir.display()))?;
        let child_dir = create_unique_sub_dir(&nest_dir)?;
        Ok(child_dir.join(format!("{child_session_id}.jsonl")))
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

    /// Invents a nested subagent rollout path + extras (session-artifacts tree).
    pub(crate) fn invent_child_session_persistence(
        &self,
        parent_rollout_path: Option<&Path>,
        parent_session_id: &devo_protocol::native::ids::SessionId,
        child_session_id: &devo_protocol::native::ids::SessionId,
    ) -> Result<InventedSessionPersistence> {
        Ok(InventedSessionPersistence {
            rollout_path: self.allocate_child_rollout_path(
                parent_rollout_path,
                parent_session_id,
                child_session_id,
            )?,
            extras: invent_session_persistence_extras(),
        })
    }

    /// Appends the mandatory session header line (path-first Native).
    pub(crate) fn append_session_meta_at(
        &self,
        rollout_path: &Path,
        session: &devo_protocol::native::session::Session,
        extras: Option<devo_core::SessionPersistenceExtras>,
    ) -> Result<()> {
        self.append_rollout_lines(
            rollout_path,
            vec![devo_core::session_line(session.clone(), extras, Utc::now())],
        )
    }

    /// Test / migrate boundary: SessionRecord → Native SessionMeta append.
    /// Live create paths must use [`Self::append_session_meta_at`].
    #[cfg(test)]
    pub(crate) fn append_session_meta(&self, record: &SessionRecord) -> Result<()> {
        self.append_session_meta_at(
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
        self.append_rollout_lines_with(rollout_path, |state| {
            let native_session_id = devo_core::native_session_id(session_id)
                .context("native session id for settings")?;
            let mut lines = Vec::with_capacity(settings.len());
            for (field, value) in settings {
                let epoch = state.write_state.allocate_settings_epoch();
                lines.push(devo_core::settings_line(
                    Utc::now(),
                    native_session_id,
                    *field,
                    value.clone(),
                    epoch,
                ));
            }
            Ok(lines)
        })
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
        self.append_rollout_lines_with(rollout_path, |state| {
            state
                .write_state
                .item_lines_from_record(&item, Utc::now())
                .context("expand item record to v2 lines")
        })
    }

    /// Narrow packed-ItemRecord helper for migrate / tests that still hold a SessionRecord.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn append_item(&self, record: &SessionRecord, item: ItemRecord) -> Result<()> {
        self.append_item_at(&record.rollout_path, item)
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
            match self.load_session_from_rollout(&rollout_path, deps).await {
                Ok(recovered) => {
                    sessions.insert(recovered.summary.session_id(), recovered);
                }
                Err(error) => {
                    tracing::warn!(
                        rollout_path = %rollout_path.display(),
                        error = %error,
                        "failed to replay rollout; skipping persisted session"
                    );
                }
            }
        }
        Ok(sessions)
    }

    /// Indexes rollout SessionMeta headers into SQLite without replaying turns.
    pub(crate) fn index_rollout_metadata(&self, db: &crate::db::Database) -> Result<()> {
        let mut canonical =
            HashMap::<SessionId, (chrono::DateTime<Utc>, PathBuf, SessionRecord)>::new();
        for rollout_path in self.rollout_paths()? {
            match read_rollout_index_fields(&rollout_path) {
                Ok((record, last_activity_at)) => match canonical.get(&record.id) {
                    Some((existing_activity, existing_path, _)) => {
                        if last_activity_at > *existing_activity {
                            tracing::warn!(
                                session_id = %record.id,
                                kept_rollout_path = %rollout_path.display(),
                                replaced_rollout_path = %existing_path.display(),
                                "duplicate rollout for session id; keeping newest last_activity_at"
                            );
                            canonical.insert(record.id, (last_activity_at, rollout_path, record));
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
                        canonical.insert(record.id, (last_activity_at, rollout_path, record));
                    }
                },
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

    /// Resolves a session cwd from the SQLite index, falling back to rollout SessionMeta.
    pub(crate) fn resolve_indexed_session_cwd(
        &self,
        db: &crate::db::Database,
        session_id: &devo_protocol::native::ids::SessionId,
    ) -> Result<Option<PathBuf>> {
        if let Some(index) = db.get_session_index(session_id)? {
            return Ok(Some(index.session.cwd));
        }
        if let Some(rollout_path) = self.find_rollout_by_session_id(session_id)? {
            let (record, _) = read_rollout_index_fields(&rollout_path)?;
            return Ok(Some(record.cwd));
        }
        Ok(None)
    }

    /// Deletes canonical rollout files for a session.
    ///
    /// Root sessions also remove `session-artifacts/<session_id>/` (nested children).
    pub(crate) fn delete_session_rollouts(&self, session_id: &SessionId) -> Result<bool> {
        let exact = format!("{session_id}.jsonl");
        let mut deleted = false;
        let mut output_candidates = Vec::new();
        for rollout_path in self.rollout_paths()? {
            let Some(file_name) = rollout_path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if file_name != exact {
                continue;
            }
            output_candidates.extend(devo_core::output_replay::read_output_references(
                &rollout_path,
            )?);
            let file_lock = {
                let mut locks = self
                    .file_locks
                    .lock()
                    .expect("rollout file-locks table poisoned");
                locks
                    .entry(rollout_path.clone())
                    .or_insert_with(|| Arc::new(StdMutex::new(())))
                    .clone()
            };
            let _guard = file_lock.lock().expect("rollout per-file lock poisoned");
            match std::fs::remove_file(&rollout_path) {
                Ok(()) => deleted = true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("delete rollout {}", rollout_path.display()));
                }
            }
        }
        let artifacts = self
            .data_root
            .join("session-artifacts")
            .join(session_id.to_string());
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
        if let Err(error) = self.collect_unreferenced_outputs(&output_candidates) {
            tracing::warn!(%error, "output cleanup deferred to preserve session references");
        }
        Ok(deleted)
    }

    pub(crate) async fn load_session_from_rollout(
        &self,
        rollout_path: &Path,
        deps: &ServerRuntimeDependencies,
    ) -> Result<RuntimeSession> {
        self.load_session_from_rollout_with_history(rollout_path, deps, /*history*/ None)
            .await
    }

    /// [`Self::load_session_from_rollout`] with a caller-provided canonical
    /// history: hydrate reads the journal once and feeds both the
    /// active-path scoping here and the downstream restores. `None` performs
    /// the history read internally (callers that hold no history yet).
    pub(crate) async fn load_session_from_rollout_with_history(
        &self,
        rollout_path: &Path,
        deps: &ServerRuntimeDependencies,
        history: Option<&devo_core::CanonicalHistory>,
    ) -> Result<RuntimeSession> {
        let mut replay = ReplayState::default();
        visit_rollout_lines(rollout_path, "resume", |v2| {
            replay
                .apply_rollout_line(v2)
                .with_context(|| format!("apply v2 line from {}", rollout_path.display()))
        })?;
        // Captured before `into_runtime_session` consumes the replay state:
        // the path-scoping block below needs it to keep the compacted prompt.
        let compaction_snapshot = replay.latest_compaction_snapshot.clone();

        let mut recovered = replay
            .into_runtime_session(deps)
            .await
            .with_context(|| format!("replay rollout {}", rollout_path.display()))?;
        if let Some(turn) = &recovered.latest_turn {
            let execution =
                devo_core::durable_execution::read_execution_replay(rollout_path, &turn.turn_id())?;
            if execution.has_checkpoint {
                recovered.core_session.lock().await.set_prompt_messages(
                    devo_core::history::response_items_to_messages(&execution.items),
                );
            }
        }
        // Absolute path is actor-owned; Native Session does not expose it.
        recovered.rollout_path = Some(rollout_path.to_path_buf());
        // Restrict model context to the active transcript-tree path (root→leaf).
        let owned_history;
        let scoped_history = match history {
            Some(history) => Some(history),
            None => {
                owned_history = read_canonical_history(rollout_path).ok();
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
    ) -> Result<Vec<TurnWorkspaceCheckpointRecordedRecord>> {
        let mut checkpoints = Vec::new();
        visit_rollout_lines(rollout_path, "checkpoint read", |v2| {
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
        if sessions.exists() {
            collect_rollout_files(&sessions, &mut files)?;
        }
        let artifacts = self.data_root.join("session-artifacts");
        if artifacts.exists() {
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

    /// RLM session artifact directory for harness / refine.
    ///
    /// Root rollouts live at `sessions/<id>.jsonl`; harness state must not sit
    /// beside them (that would share one store across all roots). Instead use
    /// `session-artifacts/<id>/` — the same tree child sessions nest under.
    /// Nested child rollouts already live under `session-artifacts/…/sub-…/`,
    /// so their parent directory is the artifact dir.
    pub(crate) fn rlm_session_dir_for_rollout(rollout_path: &Path) -> Option<PathBuf> {
        let parent = rollout_path.parent()?;
        let stem = rollout_path.file_stem()?.to_str()?;
        if parent.file_name().and_then(|s| s.to_str()) == Some("sessions") {
            let data_root = parent.parent()?;
            return Some(data_root.join("session-artifacts").join(stem));
        }
        Some(parent.to_path_buf())
    }

    /// Directory under which the next `sub-xxxxxxxx/` child folder is created.
    fn nesting_dir_for_parent(
        &self,
        parent_rollout_path: Option<&Path>,
        parent_session_id: &devo_protocol::native::ids::SessionId,
    ) -> Result<PathBuf> {
        let Some(parent_rollout_path) = parent_rollout_path else {
            // Ephemeral parent (no file): still nest under artifacts by parent id.
            return Ok(self
                .data_root
                .join("session-artifacts")
                .join(parent_session_id.to_string()));
        };
        let sessions_dir = self.data_root.join("sessions");
        let parent_dir = parent_rollout_path.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "parent rollout path has no directory: {}",
                parent_rollout_path.display()
            )
        })?;
        if parent_dir == sessions_dir {
            // Root session file → session-artifacts/<root_id>
            let stem = parent_rollout_path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "invalid root rollout file name: {}",
                        parent_rollout_path.display()
                    )
                })?;
            return Ok(self.data_root.join("session-artifacts").join(stem));
        }
        // Already under session-artifacts/…/sub-xxx/ → nest further in that dir.
        Ok(parent_dir.to_path_buf())
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

    /// RLM kernel fence outcome at spawn (design doc §5.3): audit trail for
    /// fenced/downgraded-unfenced kernels. Only downgrades need to be audited,
    /// but recording fenced spawns keeps the trail complete.
    pub(crate) fn append_kernel_fence_state(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        state: &str,
    ) -> Result<()> {
        self.append_rollout_lines(
            rollout_path,
            vec![RolloutLine::Internal {
                v: 2,
                timestamp: Utc::now(),
                session_id,
                turn_id: None,
                seq: 0,
                entry: devo_core::InternalRecord::KernelFence {
                    schema_version: 1,
                    state: state.to_string(),
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

    fn append_rollout_lines_with<F>(&self, rollout_path: &Path, build: F) -> Result<()>
    where
        F: FnOnce(&mut WritePathState) -> Result<Vec<RolloutLine>>,
    {
        if let Some(parent) = rollout_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create rollout directory {}", parent.display()))?;
        }
        let file_lock = {
            let mut locks = self
                .file_locks
                .lock()
                .expect("rollout file-locks table poisoned");
            locks
                .entry(rollout_path.to_path_buf())
                .or_insert_with(|| Arc::new(StdMutex::new(())))
                .clone()
        };
        let _guard = file_lock.lock().expect("rollout per-file lock poisoned");
        let mut write_states = self
            .write_states
            .lock()
            .expect("rollout write-state table poisoned");
        let state = match write_states.get_mut(rollout_path) {
            Some(state) => state,
            None => {
                let state = hydrate_write_state(rollout_path)?;
                write_states
                    .entry(rollout_path.to_path_buf())
                    .or_insert(state)
            }
        };
        let rollout_lines = build(state)?;
        for line in &rollout_lines {
            state.write_state.observe_rollout_line(line);
        }
        self.write_rollout_lines(rollout_path, state, &rollout_lines)
    }

    pub(crate) fn append_rollout_lines(
        &self,
        rollout_path: &Path,
        rollout_lines: Vec<RolloutLine>,
    ) -> Result<()> {
        self.append_rollout_lines_with(rollout_path, |_| Ok(rollout_lines))
    }

    fn write_rollout_lines(
        &self,
        rollout_path: &Path,
        state: &mut WritePathState,
        rollout_lines: &[RolloutLine],
    ) -> Result<()> {
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
            source_fact_id: source_fact_id(rollout_path, line_index, sub_index),
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

/// Walks a rollout JSONL file, skipping blanks and stopping at a crash tail.
/// Returns whether a truncated final line was seen.
fn visit_rollout_lines(
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
    let reader = BufReader::new(file);
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
    let mut write_state = RolloutWriteState::new();
    let mut next_line_index = 0u64;
    let file = match File::open(rollout_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WritePathState {
                write_state,
                next_line_index,
            });
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("open rollout file {}", rollout_path.display()));
        }
    };
    let saw_crash_tail = visit_open_rollout_lines(rollout_path, file, "append", &mut |v2| {
        write_state.observe_rollout_line(&v2);
        next_line_index += 1;
        Ok(())
    })?;
    if saw_crash_tail {
        // Drop the unacked crash-tail bytes so a later append cannot turn this
        // into mid-file truncation for loaders. Must run after the read handle
        // is closed (Windows cannot truncate an open file).
        discard_rollout_crash_tail(rollout_path)?;
    }
    Ok(WritePathState {
        write_state,
        next_line_index,
    })
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
        native.model.provider = turn_config
            .model_binding_id
            .unwrap_or_else(|| "unknown".into());
        native.model.model = turn_config.model.slug.clone();
        native.model.reasoning_effort = summary_reasoning_effort;
        native.settings.reasoning_effort = summary_reasoning_effort_selection.clone();
        native.settings.mode = Some(match core_session.collaboration_mode {
            devo_protocol::CollaborationMode::Build => "build".to_string(),
            devo_protocol::CollaborationMode::Plan => "plan".to_string(),
        });
        native.settings.effective_context_window = Some(applied_compaction_limit);
        native.usage = SessionUsage {
            total: UsageTotals {
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

fn read_rollout_index_fields(path: &Path) -> Result<(SessionRecord, chrono::DateTime<Utc>)> {
    let file = File::open(path).with_context(|| format!("open rollout file {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut session: Option<SessionRecord> = None;
    let mut last_activity_at: Option<chrono::DateTime<Utc>> = None;
    // The index is a rebuildable cache, so unreadable or unmappable lines are skipped.
    for line in reader.lines() {
        let line = line.with_context(|| format!("read line from {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(line) = parse_rollout_line(&line) else {
            continue;
        };
        let timestamp = rollout_line_timestamp(&line);
        last_activity_at =
            Some(last_activity_at.map_or(timestamp, |current| current.max(timestamp)));
        match line {
            RolloutLine::SessionMeta {
                session: native,
                extras,
                ..
            } => {
                let Ok(mut record) = session_record_from_native(&native, extras.as_deref()) else {
                    continue;
                };
                if record.last_activity_at.is_none() {
                    record.last_activity_at = Some(record.created_at);
                }
                session = Some(record);
            }
            RolloutLine::SessionTitleUpdated {
                timestamp, title, ..
            } => {
                if let Some(record) = session.as_mut() {
                    record.title = Some(title);
                    record.title_state =
                        SessionTitleState::Final(SessionTitleFinalSource::ExplicitCreate);
                    record.updated_at = timestamp;
                }
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

    let session = session
        .with_context(|| format!("missing SessionMeta line in rollout {}", path.display()))?;
    let last_activity_at = last_activity_at
        .or(session.last_activity_at)
        .unwrap_or(session.created_at);
    Ok((session, last_activity_at))
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

/// Creates `sub-<8 hex chars>/` under `parent_dir` (RLM child dirs).
fn create_unique_sub_dir(parent_dir: &Path) -> Result<PathBuf> {
    for _ in 0..100 {
        let name = format!("sub-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let child_dir = parent_dir.join(name);
        match std::fs::create_dir(&child_dir) {
            Ok(()) => return Ok(child_dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("create child session dir {}", child_dir.display()));
            }
        }
    }
    anyhow::bail!(
        "unable to create unique subagent session directory under {}",
        parent_dir.display()
    )
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
