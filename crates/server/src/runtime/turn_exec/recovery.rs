//! Explicit same-turn recovery; durable history is not evidence of live execution.

use std::sync::Arc;

use anyhow::{Context, bail, ensure};
use devo_core::durable_execution::{
    ExecutionRecord, ExecutionReplay, RecoveryDisposition, RecoveryState, ToolIntentJournal,
    read_execution_replay,
};
use devo_protocol::native::rpc_turn::{
    TurnRecovery, TurnRecoveryReadParams, TurnRecoveryReadResult, TurnResumeParams,
    TurnResumeResult,
};

use super::super::*;
use super::journal::RolloutToolJournal;

pub(crate) struct SavedTurn {
    pub(crate) recovery: TurnRecovery,
    turn: crate::turn::RuntimeTurn,
    rollout_path: std::path::PathBuf,
    pub(crate) execution: ExecutionReplay,
}

impl ServerRuntime {
    pub(crate) async fn saved_turn(
        &self,
        session_id: SessionId,
    ) -> anyhow::Result<Option<SavedTurn>> {
        if self.active_turns.has_session(session_id).await {
            return Ok(None);
        }
        // Recovery probes fire while switching sessions; a cold actor is not an
        // error — report no recovery until the session is loaded/resumed.
        let Some(handle) = self.session(session_id).await else {
            return Ok(None);
        };
        let snapshot = handle
            .turn_reservation_snapshot()
            .await
            .context("session unavailable")?;
        let Some(turn) = snapshot.latest_turn else {
            return Ok(None);
        };
        if matches!(
            turn.native.status,
            devo_protocol::native::turn::TurnStatus::Completed
                | devo_protocol::native::turn::TurnStatus::WaitingApproval
        ) || turn.native.kind == devo_protocol::native::turn::TurnKind::Compaction
            || matches!(
                turn.extras.failure_reason,
                Some(devo_protocol::TurnFailureReason::MaxTurnRequests)
            )
        {
            return Ok(None);
        }
        let Some(path) = handle
            .turn_persistence_snapshot()
            .await
            .and_then(|value| value.rollout_path)
        else {
            return Ok(None);
        };
        let turn_id = turn.turn_id();
        let path_for_read = path.clone();
        let execution =
            tokio::task::spawn_blocking(move || read_execution_replay(&path_for_read, &turn_id))
                .await??;
        if execution
            .recovery
            .as_ref()
            .is_some_and(|state| state.disposition == RecoveryDisposition::Canceled)
        {
            return Ok(None);
        }
        if turn.native.status == devo_protocol::native::turn::TurnStatus::Interrupted
            && execution.recovery.is_none()
        {
            return Ok(None);
        }
        let (revision, attempt, reason) = execution.recovery.as_ref().map_or_else(
            || (1, 0, "Execution ended unexpectedly.".to_string()),
            |state| (state.revision, state.attempt, state.reason.clone()),
        );
        Ok(Some(SavedTurn {
            recovery: TurnRecovery {
                turn_id: turn.native.id,
                revision,
                attempt,
                reason,
            },
            turn,
            rollout_path: path,
            execution,
        }))
    }

    pub(crate) async fn turn_recovery(
        &self,
        session_id: SessionId,
    ) -> anyhow::Result<Option<TurnRecovery>> {
        Ok(self
            .saved_turn(session_id)
            .await?
            .map(|saved| saved.recovery))
    }

    pub(crate) async fn handle_turn_recovery_read(
        self: &Arc<Self>,
        id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let result = async {
            let params: TurnRecoveryReadParams = serde_json::from_value(params)?;
            let session_id = params.session_id;
            Ok::<_, anyhow::Error>(TurnRecoveryReadResult {
                recovery: self.turn_recovery(session_id).await?,
            })
        }
        .await;
        match result {
            Ok(result) => {
                serde_json::to_value(SuccessResponse { id, result }).expect("recovery response")
            }
            Err(error) => {
                self.error_response(id, ProtocolErrorCode::InvalidParams, error.to_string())
            }
        }
    }

    pub(crate) async fn recovery_gate(&self, session_id: SessionId) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.recovery_gates
                .lock()
                .await
                .entry(session_id)
                .or_default(),
        )
    }

    /// Persist the user's decision even when no cancellation token exists.
    pub(crate) async fn cancel_saved_turn(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> anyhow::Result<bool> {
        let gate = self.recovery_gate(session_id).await;
        let _guard = gate.lock().await;
        let Some(mut saved) = self.saved_turn(session_id).await? else {
            return Ok(false);
        };
        let journal = RolloutToolJournal::new(
            Arc::clone(self),
            saved.rollout_path,
            session_id,
            saved.turn.turn_id(),
        );
        journal
            .commit(ExecutionRecord::Recovery {
                state: RecoveryState {
                    revision: saved.recovery.revision + 1,
                    attempt: saved.recovery.attempt,
                    disposition: RecoveryDisposition::Canceled,
                    reason: "Canceled by user.".into(),
                    idempotency_key: None,
                },
            })
            .await?;
        saved.turn.native.status = devo_protocol::native::turn::TurnStatus::Interrupted;
        saved.turn.native.completed_at = Some(chrono::Utc::now());
        self.persist_runtime_turn_line_deduped(session_id, &saved.turn)
            .await?;
        if let Some(handle) = self.session(session_id).await {
            handle
                .set_runtime_session_idle(Some(saved.turn.clone()))
                .await;
        }
        self.record_terminal_turn_status(
            saved.turn.turn_id(),
            TerminalTurnSnapshot::from_runtime_turn(&saved.turn),
        )
        .await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnCompleted {
                turn: Box::new(saved.turn.native.clone()),
            },
        )
        .await;
        self.broadcast_recovery_state(session_id).await;
        super::spawn_post_turn_scheduling(
            Arc::clone(self),
            session_id,
            /*should_auto_continue_goal*/ false,
        );
        Ok(true)
    }

    pub(crate) async fn persist_recovery_disposition(
        self: &Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
        disposition: RecoveryDisposition,
        reason: &str,
    ) -> anyhow::Result<()> {
        let gate = self.recovery_gate(session_id).await;
        let _guard = gate.lock().await;
        let handle = self
            .session(session_id)
            .await
            .context("session unavailable")?;
        let Some(path) = handle
            .turn_persistence_snapshot()
            .await
            .and_then(|snapshot| snapshot.rollout_path)
        else {
            return Ok(());
        };
        let journal_path = path.clone();
        let turn_id_for_read = turn_id;
        let replay =
            tokio::task::spawn_blocking(move || read_execution_replay(&path, &turn_id_for_read))
                .await??;
        let previous = replay.recovery;
        if previous
            .as_ref()
            .is_some_and(|state| state.disposition == RecoveryDisposition::Canceled)
        {
            return Ok(());
        }
        RolloutToolJournal::new(Arc::clone(self), journal_path, session_id, turn_id)
            .commit(ExecutionRecord::Recovery {
                state: RecoveryState {
                    revision: previous.as_ref().map_or(1, |state| state.revision + 1),
                    attempt: previous.as_ref().map_or(0, |state| state.attempt),
                    disposition,
                    reason: reason.into(),
                    idempotency_key: None,
                },
            })
            .await
    }

    pub(crate) async fn handle_turn_resume(
        self: &Arc<Self>,
        connection_id: u64,
        id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let result = async {
            let params: TurnResumeParams = serde_json::from_value(params)?;
            self.resume_saved_turn(connection_id, params).await
        }
        .await;
        match result {
            Ok(result) => {
                serde_json::to_value(SuccessResponse { id, result }).expect("resume response")
            }
            Err(error) => {
                self.error_response(id, ProtocolErrorCode::InvalidParams, error.to_string())
            }
        }
    }

    async fn resume_saved_turn(
        self: &Arc<Self>,
        connection_id: u64,
        params: TurnResumeParams,
    ) -> anyhow::Result<TurnResumeResult> {
        ensure!(
            !params.idempotency_key.trim().is_empty(),
            "idempotency key is required"
        );
        let session_id = params.session_id;
        let gate = self.recovery_gate(session_id).await;
        let _guard = gate.lock().await;
        let key = (session_id, format!("recovery:{}", params.idempotency_key));
        if let Some(result) = self.recovery_idempotency.lock().await.get(&key).cloned() {
            ensure!(
                result.turn.id == params.expected_turn_id,
                "idempotency key conflicts with another turn"
            );
            return Ok(result);
        }
        let saved = self
            .saved_turn(session_id)
            .await?
            .context("turn is not recoverable or is already running")?;
        ensure!(
            saved.recovery.turn_id == params.expected_turn_id
                && saved.recovery.revision == params.recovery_revision,
            "recovery state changed; refresh the session"
        );
        let handle = self
            .session(session_id)
            .await
            .context("session unavailable")?;
        let snapshot = handle
            .turn_reservation_snapshot()
            .await
            .context("session unavailable")?;
        let turn_config = snapshot.runtime_context.resolve_turn_config(
            saved
                .turn
                .model_binding_id()
                .or(Some(saved.turn.logical_model())),
            saved.turn.reasoning_effort_selection(),
        );
        let mut turn = saved.turn;
        turn.native.status = devo_protocol::native::turn::TurnStatus::InProgress;
        turn.native.completed_at = None;
        turn.native.error = None;
        turn.extras.failure_reason = None;
        turn.extras.stop_reason = None;
        if !self
            .active_turns
            .try_claim_session(session_id, turn.native.clone())
            .await
        {
            bail!("turn already active");
        }
        let attempt = saved.recovery.attempt + 1;
        let journal = RolloutToolJournal::new(
            Arc::clone(self),
            saved.rollout_path,
            session_id,
            turn.turn_id(),
        );
        let committed = async {
            journal
                .commit(ExecutionRecord::Outcomes {
                    results: saved.execution.interrupted_outcomes(),
                })
                .await?;
            journal
                .commit(ExecutionRecord::Recovery {
                    state: RecoveryState {
                        revision: saved.recovery.revision + 1,
                        attempt,
                        disposition: RecoveryDisposition::Resuming,
                        reason: "Previous execution ended unexpectedly.".into(),
                        idempotency_key: Some(params.idempotency_key),
                    },
                })
                .await?;
            self.persist_runtime_turn_line_deduped(session_id, &turn)
                .await
        }
        .await;
        if let Err(error) = committed {
            self.clear_active_turn_runtime_handles(session_id).await;
            return Err(error);
        }
        self.terminal_turn_statuses
            .lock()
            .await
            .retain(|(id, _)| *id != turn.turn_id());
        handle
            .begin_runtime_turn(turn.clone(), turn_config.clone())
            .await;
        let mut items = saved.execution.items.clone();
        items.extend(saved.execution.interrupted_outcomes());
        let restored = saved
            .execution
            .has_checkpoint
            .then(|| devo_core::history::response_items_to_messages(&items));
        let runtime = Arc::clone(self);
        let running_turn = turn.clone();
        let broadcast_session_id = session_id;
        self.spawn_active_runtime_turn_task(
            session_id,
            turn.clone(),
            Some(connection_id),
            async move {
                let resume_session_id = session_id;
                if let Err(error) = runtime
                    .run_recovered_turn(resume_session_id, running_turn, turn_config, restored)
                    .await
                {
                    tracing::error!(%resume_session_id, %error, "turn recovery failed");
                }
                runtime
                    .clear_active_turn_runtime_handles(resume_session_id)
                    .await;
                runtime.broadcast_recovery_state(resume_session_id).await;
                super::spawn_post_turn_scheduling(
                    Arc::clone(&runtime),
                    resume_session_id,
                    /*should_auto_continue_goal*/ true,
                );
            },
        )
        .await;
        let native = turn.native.clone();
        self.recovery_idempotency.lock().await.insert(
            key,
            TurnResumeResult {
                turn: native.clone(),
                attempt,
            },
        );
        self.broadcast_recovery_notification(
            broadcast_session_id,
            devo_protocol::native::event::ServerNotification::TurnResumed {
                turn: Box::new(native.clone()),
                attempt,
            },
        )
        .await;
        Ok(TurnResumeResult {
            turn: native,
            attempt,
        })
    }

    async fn run_recovered_turn(
        self: &Arc<Self>,
        session_id: SessionId,
        turn: crate::turn::RuntimeTurn,
        turn_config: devo_core::TurnConfig,
        restored: Option<Vec<devo_core::Message>>,
    ) -> anyhow::Result<()> {
        let handle = self
            .session(session_id)
            .await
            .context("session unavailable")?;
        let mut working = handle
            .checkout_turn_working_set(turn.clone())
            .await
            .context("turn checkout failed")?;
        if let Some(messages) = restored {
            working.state.core.set_prompt_messages(messages);
        }
        working.state.core.last_turn_interrupted = false;
        let collaboration_mode = working.state.core.collaboration_mode;
        self.register_turn_spawn_snapshot(
            session_id,
            turn.turn_id(),
            Arc::new(working.state.spawn_snapshot()),
        )
        .await;
        self.register_active_stream(session_id, Arc::clone(&working.state.stream))
            .await;
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(super::QUERY_EVENT_CHANNEL_CAPACITY);
        let registry = self.tool_registry_for_actor_state(&working.state);
        let parent = working.state.parent_session_id();
        let event_task = super::spawn_turn_event_stream(
            Arc::clone(self),
            Arc::clone(&working.state.stream),
            session_id,
            turn.clone(),
            collaboration_mode,
            registry,
            parent,
            None,
            event_rx,
        );
        let query_outcome = self
            .run_turn_model_query(super::TurnModelQueryParams {
                state: &mut working.state,
                turn_id: turn.turn_id(),
                turn_config: &turn_config,
                input: "",
                input_messages: &[],
                input_images: &[],
                collaboration_mode,
                input_mode: TurnInputMode::Recovery,
                usage_parent_session_id: parent.as_ref().map(|id| *id),
                event_tx,
            })
            .await;
        let event_summary = event_task.await.ok();
        self.finalize_executed_turn(super::FinalizeTurnParams {
            state: &mut working.state,
            session_id,
            turn,
            query_outcome,
            event_summary,
            usage_parent_session_id: parent.as_ref().map(|id| *id),
        })
        .await;
        let inline = working.state.stream.lock().await.turn_inline.take();
        if let Some(inline) = inline {
            inline.merge_into(&mut working.state);
        }
        handle.merge_turn(working).await;
        Ok(())
    }
}
