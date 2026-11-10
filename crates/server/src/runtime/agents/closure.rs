//! Explicit child closure durably abandons recovery before acknowledging success.

use super::*;

impl ServerRuntime {
    pub(super) async fn close_child_agent(
        self: &Arc<Self>,
        parent_session_id: SessionId,
        child_session_id: SessionId,
    ) -> Result<String, ToolCallError> {
        let already_terminal = {
            let mut registries = self.agent_registries.lock().await;
            let Some(registry) = registries.get_mut(&parent_session_id) else {
                return Err(ToolCallError::InvalidInput(format!(
                    "agent not found: {child_session_id}"
                )));
            };
            let Some(metadata) = registry.agents.get_mut(&child_session_id) else {
                return Err(ToolCallError::InvalidInput(format!(
                    "agent not found: {child_session_id}"
                )));
            };
            let terminal = matches!(
                metadata.status,
                SubagentStatus::Completed
                    | SubagentStatus::Failed
                    | SubagentStatus::Interrupted
                    | SubagentStatus::Canceled
                    | SubagentStatus::Closed
            );
            metadata.close_requested = true;
            if !terminal {
                metadata.status = SubagentStatus::Closed;
                metadata.closed_at = Some(Utc::now());
            }
            terminal
        };
        // A running turn's cancellation is now observed by the session actor's
        // own turn-completion handling (`handle_subagent_turn_completed_for_actor_state`),
        // which independently resolves and records the terminal "closed" status
        // once it sees `close_requested`. Track whether a turn was actually in
        // flight so we don't also send a duplicate closed notification below.
        self.discard_child_queued_work(parent_session_id, child_session_id)
            .await;
        let active_turn_id = self.active_turns.active_turn_id(child_session_id).await;
        let interrupted_turn = self.interrupt_child_runtime_work(child_session_id).await;
        // Closing is deliberate abandonment, not a resumable shutdown. Persist
        // it even for an already-failed child, before either success return.
        // The execution registry can be gone after merge, so fall back to the
        // actor's latest turn. Retain the admitted id for hard-abort recovery.
        let closed_turn_id = if let Some(turn_id) = active_turn_id {
            Some(turn_id)
        } else if let Some(turn) = interrupted_turn.as_ref() {
            Some(turn.turn_id())
        } else if let Some(handle) = self.session(child_session_id).await {
            handle
                .turn_reservation_snapshot()
                .await
                .and_then(|snapshot| snapshot.latest_turn.map(|turn| turn.turn_id()))
        } else {
            None
        };
        if let Some(turn_id) = closed_turn_id {
            // Recovery writes serialize under the recovery gate. Canceled is
            // sticky there: a late failure/shutdown Available write cannot
            // resurrect a turn after this close has acknowledged success.
            self.persist_recovery_disposition(
                child_session_id,
                turn_id,
                devo_core::durable_execution::RecoveryDisposition::Canceled,
                "Child agent explicitly closed.",
            )
            .await
            .map_err(|error| {
                ToolCallError::InternalError(format!("persist closed child recovery: {error}"))
            })?;
        }
        if already_terminal && interrupted_turn.is_none() {
            let status = self
                .resolve_child_agent(parent_session_id, child_session_id.as_ref())
                .await?
                .status
                .as_str()
                .to_string();
            return Ok(status);
        }

        if let Some(turn) = interrupted_turn {
            self.broadcast_notification(
                devo_protocol::native::event::ServerNotification::TurnCompleted {
                    turn: Box::new(turn.native.clone()),
                },
            )
            .await;
            self.handle_subagent_turn_completed(child_session_id, &turn)
                .await;
        } else if let Some(turn_id) = active_turn_id {
            // Hard-abort recovery may have no actor-owned turn left to return.
            if self.recent_terminal_turn_status(turn_id).await.is_none() {
                self.record_subagent_status_event(
                    parent_session_id,
                    child_session_id,
                    SubagentStatus::Closed,
                    turn_id,
                )
                .await;
            }
        } else {
            self.send_closed_notification(parent_session_id, child_session_id)
                .await;
        }
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::session_status_changed(
                child_session_id,
                SessionStatus::Idle,
                /*active_turn_id*/ None,
            ),
        )
        .await;
        Ok(SubagentStatus::Closed.as_str().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devo_core::durable_execution::{RecoveryDisposition, read_execution_replay_for_session};
    use devo_core::tools::AgentToolCoordinator;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn explicit_close_wins_over_concurrent_and_late_recovery_writes() {
        let home = tempfile::TempDir::new().unwrap();
        let runtime = crate::test_support::TestRuntime::noop().runtime(home.path());
        let started = runtime
            .start_session_with_registry(
                /*connection_id*/ 0,
                serde_json::json!(1),
                devo_protocol::SessionStartParams {
                    cwd: home.path().to_path_buf(),
                    additional_directories: Vec::new(),
                    ephemeral: false,
                    title: None,
                    model: None,
                    model_binding_id: None,
                },
                /*tool_registry*/ None,
            )
            .await;
        let parent: SessionId =
            serde_json::from_value(started["result"]["session"]["id"].clone()).unwrap();
        let child = Arc::clone(&runtime)
            .spawn_agent(
                serde_json::from_value(serde_json::json!({
                    "session_id": parent, "message": "task", "nickname": "worker",
                    "fork_turns": "none"
                }))
                .unwrap(),
            )
            .await
            .unwrap()
            .child_session_id;
        let handle = runtime.session(child).await.unwrap();
        let turn_id = tokio::time::timeout(Duration::from_secs(/*secs*/ 5), async {
            loop {
                if runtime.active_turns.active_turn_id(child).await.is_none()
                    && let Some(turn) = handle
                        .turn_reservation_snapshot()
                        .await
                        .unwrap()
                        .latest_turn
                {
                    break turn.turn_id();
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(runtime.turn_recovery(child).await.unwrap().is_some());

        // This is the same recovery writer used by failure finalization and
        // shutdown. Let it race a real close, then write again after close's
        // success. Neither ordering may restore Available over Canceled.
        let (closed, recovered) = tokio::join!(
            runtime.close_child_agent(parent, child),
            runtime.persist_recovery_disposition(
                child,
                turn_id,
                RecoveryDisposition::Available,
                "Concurrent failure finalization.",
            ),
        );
        closed.unwrap();
        recovered.unwrap();
        runtime
            .persist_recovery_disposition(
                child,
                turn_id,
                RecoveryDisposition::Available,
                "Late shutdown recovery writer.",
            )
            .await
            .unwrap();
        let path = handle.rollout_path().await.unwrap().unwrap();
        let replay = read_execution_replay_for_session(&path, &turn_id, &child).unwrap();
        assert_eq!(
            replay.recovery.unwrap().disposition,
            RecoveryDisposition::Canceled
        );
        assert!(runtime.turn_recovery(child).await.unwrap().is_none());
        runtime.shutdown().await;
    }
}
