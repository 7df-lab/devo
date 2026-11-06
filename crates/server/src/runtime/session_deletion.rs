use std::path::PathBuf;
use std::sync::Arc;

use devo_protocol::native::ids::SessionId;

use super::{ServerRuntime, TerminalTurnSnapshot};

#[derive(Debug, thiserror::Error)]
pub(crate) enum LifecycleRootError {
    #[error("missing lifecycle evidence for {session_id} at {current}")]
    MissingEvidence {
        session_id: SessionId,
        current: SessionId,
    },
    #[error("cycle while resolving lifecycle root for {session_id}")]
    Cycle { session_id: SessionId },
    #[error("storage error while resolving lifecycle root for {session_id}: {detail}")]
    Storage {
        session_id: SessionId,
        detail: String,
    },
    #[error("lifecycle admission error: {detail}")]
    Admission { detail: String },
}

struct DeletePlan {
    session_id: SessionId,
    journal_path: Option<PathBuf>,
    session_dir: Option<PathBuf>,
    delete_root_tree: bool,
}

#[derive(Default)]
pub(crate) struct LifecycleAdmission {
    deleting: std::collections::HashSet<SessionId>,
    active: std::collections::HashMap<SessionId, usize>,
}

pub(crate) struct LifecycleOperationGuard {
    admission: Arc<std::sync::Mutex<LifecycleAdmission>>,
    root_session_id: SessionId,
}

impl Drop for LifecycleOperationGuard {
    fn drop(&mut self) {
        if let Ok(mut admission) = self.admission.lock()
            && let Some(count) = admission.active.get_mut(&self.root_session_id)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                admission.active.remove(&self.root_session_id);
            }
        }
    }
}

pub(crate) struct DeleteAdmissionGuard {
    admission: Arc<std::sync::Mutex<LifecycleAdmission>>,
    root_session_id: SessionId,
}

impl Drop for DeleteAdmissionGuard {
    fn drop(&mut self) {
        if let Ok(mut admission) = self.admission.lock() {
            admission.deleting.remove(&self.root_session_id);
        }
    }
}

impl ServerRuntime {
    async fn interrupt_session_for_delete(self: &Arc<Self>, session_id: SessionId) {
        self.command_exec_manager
            .terminate_session(
                session_id,
                super::command_exec::SessionCommandOwner::AllConnections,
            )
            .await;
        // Discard queued work before cooperative cancellation can finalize and
        // spawn a follow-up. These are the actor's shared control-plane queues.
        if let Some(handle) = self.session(session_id).await
            && let Some(snapshot) = handle.turn_reservation_snapshot().await
        {
            snapshot
                .pending_turn_queue
                .lock()
                .expect("pending queue poisoned")
                .clear();
            snapshot
                .steer_input_queue
                .lock()
                .expect("steer queue poisoned")
                .clear();
        }
        let Some(mut turn) = self.active_turns.active_turn(session_id).await else {
            return;
        };
        let turn_id = turn.id;
        let terminal = self.subscribe_terminal_turn_status(turn_id).await;
        self.signal_active_turn_interrupt(session_id).await;
        // Give normal finalization/MergeTurn a short grace period. Deletion
        // does not retain history and must not wait on unresponsive tools.
        if self.recent_terminal_turn_status(turn_id).await.is_none()
            && !matches!(
                tokio::time::timeout(std::time::Duration::from_millis(500), terminal).await,
                Ok(Ok(_))
            )
        {
            self.active_turns.abort_task(session_id).await;
            tokio::task::yield_now().await;
        }
        // A forcibly aborted task cannot publish its own terminal status.
        // Resolve all turn waiters, including a subscriber that raced normal
        // finalization, rather than keeping deleted turns in the waiter map.
        let snapshot = match self.recent_terminal_turn_status(turn_id).await {
            Some(snapshot) => snapshot,
            None => {
                // Hard abort bypasses finalize_executed_turn. Publish the same
                // Native terminal event while the session/subscriptions still
                // exist, otherwise clients retain an in-progress turn forever.
                turn.status = devo_protocol::native::turn::TurnStatus::Interrupted;
                turn.completed_at = Some(chrono::Utc::now());
                self.broadcast_notification(
                    devo_protocol::native::event::ServerNotification::TurnCompleted {
                        turn: Box::new(turn),
                    },
                )
                .await;
                TerminalTurnSnapshot {
                    status: devo_protocol::TurnStatus::Interrupted,
                    stop_reason: None,
                    failure_reason: None,
                }
            }
        };
        self.record_terminal_turn_status(turn_id, snapshot).await;
    }

    /// Single authoritative lifecycle-root resolver. Uses the DB index first,
    /// then raw persisted SessionMeta lineage (including tombstones/missing
    /// index rows), and returns an error for unprovable chains or cycles. Spawn,
    /// resume, admission, and delete all use this function.
    pub(crate) async fn resolve_lifecycle_root(
        &self,
        session_id: SessionId,
        fallback: Option<&crate::runtime_session_summary::RuntimeSessionSummary>,
    ) -> Result<SessionId, LifecycleRootError> {
        let mut current = session_id;
        let mut seen = std::collections::HashSet::new();
        loop {
            if !seen.insert(current) {
                return Err(LifecycleRootError::Cycle { session_id });
            }
            match self.deps.db.get_session_index(&current) {
                Ok(Some(index)) => {
                    if index.session.agent_path.is_none() {
                        return Ok(current);
                    }
                    let Some(parent_session_id) = index.session.parent_session_id else {
                        return Err(LifecycleRootError::Storage {
                            session_id,
                            detail: format!("agent session {current} has no parent in the index"),
                        });
                    };
                    current = parent_session_id;
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    return Err(LifecycleRootError::Storage {
                        session_id,
                        detail: format!("read session index for {current}: {error}"),
                    });
                }
            }
            if current == session_id
                && let Some(summary) = fallback
            {
                if summary.agent_path.is_none() {
                    return Ok(current);
                }
                if let Some(parent_session_id) = summary.parent_session_id() {
                    current = parent_session_id;
                    continue;
                }
            }
            // Live runtime metadata covers ephemeral sessions that have no DB
            // row and no journal, while keeping the same root key as durable
            // ancestors.
            if let Some(handle) = self.session(current).await
                && let Some(summary) = handle.summary().await
            {
                if summary.agent_path.is_none() {
                    return Ok(current);
                }
                if let Some(parent_session_id) = summary.parent_session_id() {
                    current = parent_session_id;
                    continue;
                }
            }
            match self
                .rollout_store
                .find_journal_containing_session_including_deleted(&current)
            {
                Ok(Some(journal_path)) => match self
                    .rollout_store
                    .raw_session_parent(&journal_path, &current)
                    .map_err(|error| LifecycleRootError::Storage {
                        session_id,
                        detail: format!("read persisted lineage for {current}: {error}"),
                    })? {
                    Some(Some(parent_session_id)) => {
                        current = parent_session_id;
                        continue;
                    }
                    Some(None) => return Ok(current),
                    None => {}
                },
                Ok(None) => {}
                Err(error) => {
                    return Err(LifecycleRootError::Storage {
                        session_id,
                        detail: format!("scan journals for {current}: {error}"),
                    });
                }
            }
            return Err(LifecycleRootError::MissingEvidence {
                session_id,
                current,
            });
        }
    }

    async fn session_has_any_evidence(&self, session_id: SessionId) -> Result<bool, String> {
        if self.session(session_id).await.is_some() {
            return Ok(true);
        }
        if self.active_turns.active_turn_id(session_id).await.is_some() {
            return Ok(true);
        }
        if self
            .deps
            .db
            .get_session_index(&session_id)
            .map_err(|error| format!("failed to read session index: {error}"))?
            .is_some()
        {
            return Ok(true);
        }
        Ok(self
            .rollout_store
            .find_journal_containing_session_strict(
                &session_id,
                crate::persistence::JournalVisibility::IncludeDeleted,
            )
            .map_err(|error| format!("failed to scan session journal: {error}"))?
            .is_some())
    }

    /// Reserve the lifecycle root for a spawn/resume. The guard is synchronous
    /// and drop-safe, so an aborted future cannot leave a stale marker.
    pub(crate) fn begin_lifecycle_operation_for_root(
        &self,
        root_session_id: SessionId,
    ) -> Result<LifecycleOperationGuard, String> {
        let mut admission = self
            .lifecycle_admission
            .lock()
            .expect("lifecycle admission mutex poisoned");
        if admission.deleting.contains(&root_session_id) {
            return Err(format!(
                "session deletion is in progress for {root_session_id}"
            ));
        }
        *admission.active.entry(root_session_id).or_default() += 1;
        Ok(LifecycleOperationGuard {
            admission: Arc::clone(&self.lifecycle_admission),
            root_session_id,
        })
    }

    pub(crate) async fn begin_lifecycle_operation(
        &self,
        session_id: SessionId,
    ) -> Result<LifecycleOperationGuard, LifecycleRootError> {
        let root_session_id = self.resolve_lifecycle_root(session_id, None).await?;
        self.begin_lifecycle_operation_for_root(root_session_id)
            .map_err(|detail| LifecycleRootError::Admission { detail })
    }

    fn begin_delete_admission_for_root(
        &self,
        root_session_id: SessionId,
    ) -> Result<DeleteAdmissionGuard, String> {
        let mut admission = self
            .lifecycle_admission
            .lock()
            .expect("lifecycle admission mutex poisoned");
        if admission.deleting.contains(&root_session_id) {
            return Err(format!(
                "session deletion already in progress for {root_session_id}"
            ));
        }
        if admission.active.get(&root_session_id).copied().unwrap_or(0) > 0 {
            return Err(format!(
                "spawn/resume is in progress for {root_session_id}; retry deletion"
            ));
        }
        admission.deleting.insert(root_session_id);
        Ok(DeleteAdmissionGuard {
            admission: Arc::clone(&self.lifecycle_admission),
            root_session_id,
        })
    }

    pub(crate) async fn delete_session_tree(
        self: &Arc<Self>,
        root_session_id: SessionId,
    ) -> Result<Vec<SessionId>, String> {
        let session_ids = self.collect_session_delete_tree(root_session_id).await?;
        let target_has_evidence = self.session_has_any_evidence(root_session_id).await?;
        if !target_has_evidence && session_ids.len() <= 1 {
            return Ok(Vec::new());
        }
        let lifecycle_root = match self.resolve_lifecycle_root(root_session_id, None).await {
            Ok(root) => root,
            Err(LifecycleRootError::MissingEvidence { .. })
                if target_has_evidence || session_ids.len() > 1 =>
            {
                // Delete-only fallback for an orphan target with proven DB/
                // runtime/journal or descendant evidence. Cycles/storage
                // failures are never swallowed.
                root_session_id
            }
            Err(error) => return Err(error.to_string()),
        };
        let _delete_guard = self.begin_delete_admission_for_root(lifecycle_root)?;
        // Recollect under the admission guard: a spawn/resume could have
        // completed between the pre-check collection and this point.
        let session_ids = self.collect_session_delete_tree(root_session_id).await?;
        let deleting_root = lifecycle_root == root_session_id;
        self.delete_session_tree_inner(root_session_id, deleting_root, session_ids)
            .await
    }

    async fn delete_session_tree_inner(
        self: &Arc<Self>,
        root_session_id: SessionId,
        deleting_root: bool,
        session_ids: Vec<SessionId>,
    ) -> Result<Vec<SessionId>, String> {
        // Lease every physical journal in the tree before any mutation. Sort by
        // the canonical key so concurrent deletes acquire in one order. Missing
        // journals are skipped; a real lock error aborts before interruption.
        // Lease the initial target journals, then recollect under those leases.
        // Cross-process spawn/resume must take the same journal lease; a daemon
        // completing before our lease shows up in the recollected DB tree.
        let initial_paths = self.session_journal_paths(&session_ids)?;
        let mut _journal_leases = Vec::new();
        let mut leased_keys = std::collections::HashSet::new();
        for path in &initial_paths {
            let lease = crate::runtime::session_file_lock::SessionFileLock::acquire(path)
                .map_err(|error| format!("failed to lock journal before delete: {error}"))?;
            _journal_leases.push(lease);
            leased_keys.insert(
                crate::journal_path::canonical_journal_key(path)
                    .map_err(|error| format!("canonical journal key: {error}"))?,
            );
        }
        let session_ids = self.collect_session_delete_tree(root_session_id).await?;
        let added_paths = self.session_journal_paths(&session_ids)?;
        for path in added_paths {
            let key = crate::journal_path::canonical_journal_key(&path)
                .map_err(|error| format!("canonical journal key: {error}"))?;
            if leased_keys.insert(key) {
                let lease = crate::runtime::session_file_lock::SessionFileLock::acquire(&path)
                    .map_err(|error| {
                        format!("failed to lock added journal before delete: {error}")
                    })?;
                _journal_leases.push(lease);
            }
        }
        // One final collection under all leases. No unbounded retry: if a new
        // descendant journal appeared despite the leases, fail closed.
        let session_ids = self.collect_session_delete_tree(root_session_id).await?;
        let final_paths = self.session_journal_paths(&session_ids)?;
        for path in &final_paths {
            let key = crate::journal_path::canonical_journal_key(path)
                .map_err(|error| format!("canonical journal key: {error}"))?;
            if !leased_keys.contains(&key) {
                return Err(format!(
                    "descendant journal set changed during lease acquisition: {}",
                    path.display()
                ));
            }
        }

        let mut plans: Vec<DeletePlan> = Vec::new();
        for session_id in &session_ids {
            let index = self
                .deps
                .db
                .get_session_index(session_id)
                .map_err(|error| format!("failed to read session index: {error}"))?;
            let indexed_path = index.as_ref().and_then(|index| index.rollout_path.clone());
            let path = self
                .rollout_store
                .resolve_session_journal(
                    session_id,
                    indexed_path.as_deref(),
                    crate::persistence::JournalVisibility::IncludeDeleted,
                )
                .map_err(|error| {
                    format!("failed to resolve session journal for {session_id}: {error}")
                })?;
            let resolved_root = self.resolve_lifecycle_root(*session_id, None).await;
            let target_is_agent = match &resolved_root {
                Ok(root) => *root != *session_id,
                Err(LifecycleRootError::MissingEvidence { .. }) => {
                    index.as_ref().is_some_and(|index| {
                        index.session.agent_path.is_some()
                            && index.session.parent_session_id.is_some_and(|parent| {
                                self.deps
                                    .db
                                    .get_session_index(&parent)
                                    .ok()
                                    .flatten()
                                    .is_none()
                            })
                    })
                }
                Err(error) => return Err(error.to_string()),
            };
            let fallback_child_dir = match &resolved_root {
                Ok(root) if *root != *session_id => {
                    Some(self.rollout_store.child_session_dir(root, session_id))
                }
                Ok(_) | Err(_) => None,
            };
            // A recorded index path is trusted only when it is this session's
            // own expected filename and its legacy dir sits under the proven
            // lifecycle root. Unknown ownership skips cleanup; never delete an
            // unrelated descendant dir.
            let recorded_session_dir = match (indexed_path.as_ref(), &resolved_root) {
                (Some(path), Ok(root))
                    if path.file_name().and_then(|name| name.to_str())
                        == Some(format!("{session_id}.jsonl").as_str())
                        && *root != *session_id =>
                {
                    let root_dir = self.rollout_store.root_session_dir(root);
                    let root_dir = std::fs::canonicalize(&root_dir).unwrap_or(root_dir);
                    let expected_child_dir = self.rollout_store.child_session_dir(root, session_id);
                    let expected_child_dir =
                        std::fs::canonicalize(&expected_child_dir).unwrap_or(expected_child_dir);
                    self.rollout_store
                        .session_dir_for(path, session_id)
                        .filter(|dir| {
                            if !self.rollout_store.is_child_artifact_dir(dir) {
                                return false;
                            }
                            if *dir == expected_child_dir {
                                return true;
                            }
                            let Ok(relative) = dir.strip_prefix(&root_dir) else {
                                return false;
                            };
                            // Legacy ancestry: every segment below the root is
                            // a sub-xxxxxxxx directory.
                            let segments: Vec<_> = relative
                                .components()
                                .filter_map(|component| component.as_os_str().to_str())
                                .collect();
                            !segments.is_empty()
                                && segments.iter().all(|segment| segment.starts_with("sub-"))
                        })
                }
                (Some(path), _) => {
                    tracing::warn!(
                        session_id = %session_id,
                        path = %path.display(),
                        "skipping unproven recorded artifact dir during delete"
                    );
                    None
                }
                (None, _) => None,
            };
            let session_dir = path
                .as_ref()
                .and_then(|journal_path| {
                    self.rollout_store.session_dir_for(journal_path, session_id)
                })
                .or(recorded_session_dir)
                .or(fallback_child_dir);
            let delete_root_tree =
                deleting_root && *session_id == root_session_id && !target_is_agent;
            plans.push(DeletePlan {
                session_id: *session_id,
                journal_path: path,
                session_dir,
                delete_root_tree,
            });
        }
        // Persist deletion markers before any fallible teardown (interrupt,
        // actor removal, close, unlink). A crash after this point cannot
        // resurrect any target in the tree; retries can finish the delete.
        for plan in &plans {
            // No journal file means a prior delete already unlinked it; still
            // continue DB/artifact cleanup idempotently. Lookup errors were
            // already propagated by strict resolution above.
            if let Some(path) = plan.journal_path.as_ref().filter(|path| path.exists()) {
                self.rollout_store
                    .append_session_deleted_at(path, plan.session_id, chrono::Utc::now())
                    .map_err(|error| {
                        format!(
                            "failed to persist session deletion marker for {}: {error}",
                            plan.session_id
                        )
                    })?;
            }
        }
        // Interrupt the whole tree together; a blocked child must not delay
        // cancellation of its siblings or multiply the grace period.
        futures::future::join_all(
            session_ids
                .iter()
                .map(|session_id| self.interrupt_session_for_delete(*session_id)),
        )
        .await;

        let removed_sessions =
            futures::future::join_all(session_ids.iter().map(|session_id| async {
                let session_id = *session_id;
                let removed = self.remove_session_actor(session_id).await;
                self.clear_deleted_session_runtime_state(session_id).await;
                if let Some(handle) = &removed {
                    handle.shutdown_for_delete().await;
                }
                (session_id, removed.is_some())
            }))
            .await;
        let mut removed_by_id = std::collections::HashMap::new();
        for (session_id, removed) in removed_sessions {
            removed_by_id.insert(session_id, removed);
        }
        let mut deleted_session_ids = Vec::new();
        for plan in plans {
            let session_id = plan.session_id;
            let removed = removed_by_id.get(&session_id).copied().unwrap_or(false);
            let persisted = self
                .deps
                .db
                .get_session(&session_id)
                .map_err(|error| format!("failed to inspect session before delete: {error}"))?;
            let deleted_rollout = self
                .rollout_store
                .delete_session_journal(
                    &session_id,
                    plan.journal_path.as_deref(),
                    plan.session_dir.as_deref(),
                    plan.delete_root_tree,
                )
                .map_err(|error| format!("failed to delete session journal: {error}"))?;
            if persisted.is_some() {
                self.deps
                    .db
                    .clear_pending(&session_id, crate::db::QueueType::Turn)
                    .map_err(|error| format!("failed to clear pending turn queue: {error}"))?;
                self.deps
                    .db
                    .clear_pending(&session_id, crate::db::QueueType::Steer)
                    .map_err(|error| format!("failed to clear pending steer queue: {error}"))?;
                self.deps
                    .db
                    .delete_session(&session_id)
                    .map_err(|error| format!("failed to delete session metadata: {error}"))?;
            }
            if removed || persisted.is_some() || deleted_rollout {
                deleted_session_ids.push(session_id);
            }
        }
        Ok(deleted_session_ids)
    }

    fn session_journal_paths(&self, session_ids: &[SessionId]) -> Result<Vec<PathBuf>, String> {
        let mut keyed = Vec::new();
        for session_id in session_ids {
            let index = self
                .deps
                .db
                .get_session_index(session_id)
                .map_err(|error| format!("failed to read session index: {error}"))?;
            let indexed_path = index.as_ref().and_then(|index| index.rollout_path.clone());
            let path = self
                .rollout_store
                .resolve_session_journal(
                    session_id,
                    indexed_path.as_deref(),
                    crate::persistence::JournalVisibility::IncludeDeleted,
                )
                .map_err(|error| {
                    format!("failed to resolve session journal for {session_id}: {error}")
                })?;
            if let Some(path) = path.filter(|path| path.exists()) {
                let key = crate::journal_path::canonical_journal_key(&path).map_err(|error| {
                    format!("canonical journal key for {}: {error}", path.display())
                })?;
                keyed.push((key, path));
            }
        }
        keyed.sort_by(|left, right| left.0.cmp(&right.0));
        keyed.dedup_by(|left, right| left.0 == right.0);
        Ok(keyed.into_iter().map(|(_, path)| path).collect())
    }

    async fn collect_session_delete_tree(
        &self,
        root_session_id: SessionId,
    ) -> Result<Vec<SessionId>, String> {
        // Cascade only sub-agent children (`parent_session_id` + agent markers).
        // User forks use `fork_from_id` and must remain after the source is deleted.
        // Runtime actors cover live sessions; the SQLite index covers children
        // that were never hydrated after a restart.
        let session_ids_in_runtime: Vec<SessionId> = {
            let sessions = self.sessions.lock().await;
            sessions.keys().cloned().collect()
        };
        let mut agent_children_by_parent = Vec::new();
        for session_id in session_ids_in_runtime {
            let Some(handle) = self.session(session_id).await else {
                continue;
            };
            let Some(summary) = handle.summary().await else {
                continue;
            };
            let is_agent_child = summary.agent_path.is_some()
                || summary.agent_role.is_some()
                || summary.agent_nickname.is_some();
            if is_agent_child && let Some(parent_id) = summary.parent_session_id() {
                agent_children_by_parent.push((session_id, parent_id));
            }
        }
        let rows = self
            .deps
            .db
            .list_sessions()
            .map_err(|error| format!("failed to list session index: {error}"))?;
        for row in rows {
            if row.agent_path.is_some()
                && let Some(parent_id) = row.parent_session_id
            {
                agent_children_by_parent.push((row.session_id, parent_id));
            }
        }
        // Retry after tombstone+DB removal: the index/runtime can no longer
        // see descendants. Raw persisted SessionMeta parent edges keep the
        // whole subtree addressable for cleanup. Damaged journals are fatal.
        let persisted_edges = self
            .rollout_store
            .persisted_agent_edges()
            .map_err(|error| format!("failed to read persisted agent lineage: {error}"))?;
        agent_children_by_parent.extend(persisted_edges);
        agent_children_by_parent.sort_by_key(|(session_id, _parent_id)| session_id.to_string());
        agent_children_by_parent.dedup();

        let mut seen = std::collections::HashSet::new();
        let mut session_ids = Vec::new();
        seen.insert(root_session_id);
        session_ids.push(root_session_id);
        let mut index = 0;
        while index < session_ids.len() {
            let parent_session_id = session_ids[index];
            for (session_id, parent_id) in &agent_children_by_parent {
                if *parent_id == parent_session_id && seen.insert(*session_id) {
                    session_ids.push(*session_id);
                }
            }
            index += 1;
        }
        Ok(session_ids)
    }

    pub(crate) async fn await_session_turn_interrupt_before_delete(
        self: &Arc<Self>,
        session_id: SessionId,
    ) {
        let Some(turn_id) = self.runtime_active_turn_id(session_id).await else {
            return;
        };
        let receiver = self.subscribe_terminal_turn_status(turn_id).await;
        if self.recent_terminal_turn_status(turn_id).await.is_some() {
            return;
        }
        self.signal_active_turn_interrupt(session_id).await;
        if tokio::time::timeout(std::time::Duration::from_secs(5), receiver)
            .await
            .is_err()
            && self.runtime_active_turn_id(session_id).await.is_some()
        {
            tracing::warn!(
                session_id = %session_id,
                turn_id = %turn_id,
                "turn interrupt timed out before session delete"
            );
        }
    }

    async fn clear_deleted_session_runtime_state(&self, session_id: SessionId) {
        self.signal_active_turn_interrupt(session_id).await;
        self.active_turns.clear_runtime_handles(session_id).await;
        if let Some(turn_id) = self
            .active_goal_continuation_turns
            .lock()
            .await
            .remove(&session_id)
        {
            self.goal_continuation_turn_goals
                .lock()
                .await
                .remove(&turn_id);
        }
        self.goal_stores.lock().await.remove(&session_id);
        self.agent_mailboxes.lock().await.remove(&session_id);
        self.agent_output_buffers.lock().await.remove(&session_id);
        self.agent_wait_cursors.lock().await.remove(&session_id);
        {
            let mut registries = self.agent_registries.lock().await;
            registries.remove(&session_id);
            for registry in registries.values_mut() {
                registry.unregister(session_id);
            }
        }
    }
}

#[cfg(test)]
#[path = "session_deletion_tests.rs"]
mod tests;
