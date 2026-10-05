use super::*;

impl ServerRuntime {
    pub(super) async fn subscribe_terminal_turn_status(
        &self,
        turn_id: TurnId,
    ) -> oneshot::Receiver<TerminalTurnSnapshot> {
        let (sender, receiver) = oneshot::channel();
        self.acp_prompt_waiters
            .lock()
            .await
            .entry(turn_id)
            .or_default()
            .push(sender);
        receiver
    }

    pub(super) async fn recent_terminal_turn_status(
        &self,
        turn_id: TurnId,
    ) -> Option<TerminalTurnSnapshot> {
        self.terminal_turn_statuses
            .lock()
            .await
            .iter()
            .rev()
            .find_map(|(completed_turn_id, status)| {
                (*completed_turn_id == turn_id).then(|| status.clone())
            })
    }

    pub(super) async fn record_terminal_turn_status(
        &self,
        turn_id: TurnId,
        snapshot: TerminalTurnSnapshot,
    ) {
        {
            let mut statuses = self.terminal_turn_statuses.lock().await;
            statuses.retain(|(completed_turn_id, _)| *completed_turn_id != turn_id);
            statuses.push_back((turn_id, snapshot.clone()));
            while statuses.len() > TERMINAL_TURN_STATUS_LIMIT {
                statuses.pop_front();
            }
        }

        let waiters = self.acp_prompt_waiters.lock().await.remove(&turn_id);
        if let Some(waiters) = waiters {
            for waiter in waiters {
                let _ = waiter.send(snapshot.clone());
            }
        }
    }

    pub(super) async fn runtime_active_turn_id(
        &self,
        session_id: devo_protocol::native::ids::SessionId,
    ) -> Option<devo_protocol::native::ids::TurnId> {
        self.active_turns.active_turn_id(session_id).await
    }

    pub(super) async fn add_active_turn_user_text(
        &self,
        session_id: devo_protocol::native::ids::SessionId,
        turn_id: devo_protocol::native::ids::TurnId,
        text: String,
    ) {
        self.active_turns
            .add_active_user_text(session_id, turn_id, text)
            .await;
    }

    pub(super) async fn register_runtime_active_turn(
        &self,
        session_id: devo_protocol::native::ids::SessionId,
        turn: crate::turn::RuntimeTurn,
    ) {
        self.active_turns
            .register_turn(session_id, turn.native)
            .await;
    }

    pub(super) async fn clear_active_turn_interrupt_handles(
        &self,
        session_id: devo_protocol::native::ids::SessionId,
    ) {
        self.active_turns.clear_interrupt_handles(session_id).await;
    }

    pub(super) async fn clear_active_turn_runtime_handles(
        &self,
        session_id: devo_protocol::native::ids::SessionId,
    ) {
        self.active_turns.clear_runtime_handles(session_id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use anyhow::Result;
    use devo_protocol::ErrorResponse;
    use devo_protocol::SuccessResponse;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    fn build_runtime(data_root: &std::path::Path) -> Arc<ServerRuntime> {
        crate::test_support::TestRuntime::noop()
            .db_file("turn_reservation.db")
            .runtime(data_root)
    }

    async fn start_session(runtime: &Arc<ServerRuntime>, cwd: std::path::PathBuf) -> SessionId {
        let value = runtime
            .start_session_with_registry(
                /*connection_id*/ 1,
                serde_json::json!(1),
                SessionStartParams {
                    cwd,
                    additional_directories: Vec::new(),
                    ephemeral: false,
                    title: None,
                    model: None,
                    model_binding_id: None,
                },
                None,
            )
            .await;
        let response: SuccessResponse<SessionStartResult> =
            serde_json::from_value(value).expect("session start response");
        SessionId::from(response.result.session.id.as_str())
    }

    #[tokio::test]
    async fn reject_active_turn_policy_does_not_enqueue_input() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = start_session(&runtime, data_root.path().to_path_buf()).await;
        let session_handle = runtime.session(session_id).await.expect("session");
        let reservation = session_handle
            .turn_reservation_snapshot()
            .await
            .expect("turn reservation snapshot");
        let turn_config = reservation.runtime_context.resolve_turn_config(None, None);
        let native_turn_id = devo_protocol::native::ids::TurnId::new();
        let native_session_id = reservation.summary.native.id;
        let active_turn = crate::turn::RuntimeTurn::new(
            devo_protocol::native::turn::Turn {
                id: native_turn_id,
                session_id: native_session_id,
                sequence: 1,
                status: devo_protocol::native::turn::TurnStatus::InProgress,
                kind: devo_protocol::native::turn::TurnKind::Regular,
                model: devo_protocol::native::model::ModelBinding {
                    provider: "unknown".to_string(),
                    model: "test-model".to_string(),
                    variant: None,
                    reasoning_effort: None,
                },
                collaboration_mode: None,
                started_at: Utc::now(),
                completed_at: None,
                usage: None,
                error: None,
            },
            crate::turn::RuntimeTurnExtras {
                request_thinking: None,
                stop_reason: None,
                failure_reason: None,
            },
        );
        session_handle
            .begin_runtime_turn(active_turn, turn_config)
            .await;

        let value = runtime
            .handle_turn_start_with_queue_policy(
                None,
                serde_json::json!(2),
                TurnStartParams {
                    session_id,
                    input: vec![devo_protocol::native::item::UserInput::Text {
                        text: "must not queue".to_string(),
                    }],
                    model: None,
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    sandbox: None,
                    approval_policy: None,
                    cwd: None,
                    collaboration_mode: devo_protocol::CollaborationMode::Build,
                    execution_mode: devo_protocol::TurnExecutionMode::Regular,
                },
                TurnStartQueuePolicy::RejectActive,
            )
            .await;
        let response: ErrorResponse = serde_json::from_value(value).expect("error response");
        let queued_len = session_handle
            .pending_queue_snapshot()
            .await
            .map(|snapshot| snapshot.pending_count)
            .unwrap_or(0);

        assert_eq!(response.error.code, ProtocolErrorCode::TurnAlreadyRunning);
        assert_eq!(queued_len, 0);

        Ok(())
    }
}
