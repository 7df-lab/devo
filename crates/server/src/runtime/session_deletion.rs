use std::sync::Arc;

use devo_protocol::native::ids::SessionId;

use super::{ServerRuntime, TerminalTurnSnapshot};

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

    pub(crate) async fn delete_session_tree(
        self: &Arc<Self>,
        root_session_id: SessionId,
    ) -> Result<Vec<SessionId>, String> {
        let session_ids = self.collect_session_delete_tree(root_session_id).await;
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
        let mut deleted_session_ids = Vec::new();
        for (session_id, removed) in removed_sessions {
            let persisted = self
                .deps
                .db
                .get_session(&session_id)
                .map_err(|error| format!("failed to inspect session before delete: {error}"))?;
            let deleted_rollout = self
                .rollout_store
                .delete_session_rollouts(&session_id)
                .map_err(|error| format!("failed to delete session rollout: {error}"))?;
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

    async fn collect_session_delete_tree(&self, root_session_id: SessionId) -> Vec<SessionId> {
        // Cascade only sub-agent children (`parent_session_id` + agent markers).
        // User forks use `fork_from_id` and must remain after the source is deleted.
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
        agent_children_by_parent.sort_by_key(|(session_id, _parent_id)| session_id.to_string());

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
        session_ids
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
