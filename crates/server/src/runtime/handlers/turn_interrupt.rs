use std::sync::Arc;
use std::time::Duration;

use super::super::*;

const TURN_INTERRUPT_TERMINAL_TIMEOUT: Duration = Duration::from_secs(5);

impl ServerRuntime {
    pub(crate) async fn interrupt_turn(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: TurnInterruptParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid interrupt params: {error}"),
                );
            }
        };
        self.handle_turn_interrupt_translated(request_id, params)
            .await
    }

    /// Preempt an active goal-continuation turn because the user submitted a
    /// new message. Continuation turns are unbounded by design (the model
    /// loops until the goal completes), so a queued entry behind one would
    /// wait for a turn boundary that never comes — starving the user's input
    /// indefinitely. Interrupting lets the standard post-turn drain run the
    /// user's message next; the goal itself stays active and resumes after.
    pub(crate) fn preempt_goal_continuation_for_user_input(
        self: &Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
    ) {
        let runtime = Arc::clone(self);
        tracing::info!(
            session_id = %session_id,
            turn_id = %turn_id,
            "preempting goal continuation for user input"
        );
        tokio::spawn(async move {
            let response = runtime
                .interrupt_turn(
                    serde_json::Value::Null,
                    serde_json::to_value(TurnInterruptParams {
                        session_id,
                        turn_id,
                        reason: Some("user input preempted goal continuation".to_string()),
                    })
                    .expect("serialize internal turn interruption"),
                )
                .await;
            if response.get("error").is_some() {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn_id,
                    "failed to preempt goal continuation for user input"
                );
            }
        });
    }

    /// Preempt an active turn parked at an approval checkpoint because the
    /// user submitted a new message. A parked turn never reaches a turn
    /// boundary on its own — the permission request can wait indefinitely —
    /// so rejecting or queueing behind it starves the new input. Unlike
    /// [`Self::preempt_goal_continuation_for_user_input`], this is awaited:
    /// the caller re-reads the reservation afterwards and admits the new
    /// turn directly. The interrupt's cancel token resolves the parked
    /// permission request as `Cancelled` (the approval item is persisted and
    /// the interactive lane cleared by the wait's own error path) and the
    /// turn finalizes as Interrupted. Bounded by
    /// `TURN_INTERRUPT_TERMINAL_TIMEOUT`.
    pub(crate) async fn preempt_approval_parked_turn(
        self: &Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
    ) {
        tracing::info!(
            session_id = %session_id,
            turn_id = %turn_id,
            "preempting approval-parked turn for user input"
        );
        let response = self
            .interrupt_turn(
                serde_json::Value::Null,
                serde_json::to_value(TurnInterruptParams {
                    session_id,
                    turn_id,
                    reason: Some("new user message preempted approval-parked turn".to_string()),
                })
                .expect("serialize internal turn interruption"),
            )
            .await;
        if response.get("error").is_some() {
            tracing::warn!(
                session_id = %session_id,
                turn_id = %turn_id,
                "failed to preempt approval-parked turn for user input"
            );
        }
    }

    async fn handle_turn_interrupt_translated(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: TurnInterruptParams,
    ) -> serde_json::Value {
        let session_id = params.session_id;
        let turn_id = params.turn_id;
        let Some(session_handle) = self.session(session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        // Turns that run on a spawned task finalize themselves when the cancel
        // token fires (`finalize_executed_turn` + `MergeTurn`). Interrupt waits
        // for that terminal status; claiming `active_turn` is only an orphan
        // fallback after the wait times out.
        if self.runtime_active_turn_id(session_id).await != Some(turn_id) {
            let matches_saved = session_handle
                .turn_reservation_snapshot()
                .await
                .and_then(|snapshot| snapshot.latest_turn)
                .is_some_and(|turn| turn.native.id == turn_id);
            match if matches_saved {
                self.cancel_saved_turn(session_id).await
            } else {
                Ok(false)
            } {
                Ok(true) => {
                    return self.turn_interrupt_success(
                        request_id,
                        turn_id,
                        TurnStatus::Interrupted,
                    );
                }
                Ok(false) => {}
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        error.to_string(),
                    );
                }
            }
            if let Some(snapshot) = self.recent_terminal_turn_status(turn_id).await {
                return self.turn_interrupt_success(request_id, turn_id, snapshot.status);
            }
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnNotFound,
                "turn is not active",
            );
        }

        let terminal_rx = self.subscribe_terminal_turn_status(turn_id).await;
        if let Some(snapshot) = self.recent_terminal_turn_status(turn_id).await {
            self.record_terminal_turn_status(turn_id, snapshot.clone())
                .await;
            return self.turn_interrupt_success(request_id, turn_id, snapshot.status);
        }
        // Cancel before mailbox work. All turns run on a spawned task; the
        // cancel token unblocks query, and abort covers stuck tasks. Do not
        // claim `active_turn` until terminal wait times out — claiming while
        // the turn task is still finalizing races with `MergeTurn`.
        // Cancel via a clone rather than `remove`: see the comment in
        // `interrupt_child_runtime_work` for why removing here races with
        // `run_turn_model_query` fetching the same token.
        if let Err(error) = self
            .persist_recovery_disposition(
                session_id,
                turn_id,
                devo_core::durable_execution::RecoveryDisposition::Canceled,
                "Stopped by user.",
            )
            .await
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                error.to_string(),
            );
        }
        self.signal_active_turn_interrupt(session_id).await;

        // Abort clears pending compact + refine first (deadlock class).
        let native_id = session_handle
            .summary()
            .await
            .map(|summary| summary.native.id)
            .unwrap_or_else(|| session_id);
        crate::runtime::compact_host::clear_pending_compact(&native_id);
        crate::runtime::refine::clear_pending_refine(&native_id);

        let removed = self
            .session_interactive
            .drain_pending_user_inputs_for_turn(session_id, turn_id)
            .await;
        let removed_len = removed.len();
        for (request_id, pending) in removed {
            if let Some(persisted) = &pending.persisted {
                let (native_session_id, native_turn_id) = self
                    .native_session_turn_ids(pending.owner_session_id, pending.turn_id)
                    .await;
                self.persist_terminal_user_input_item(
                    native_session_id,
                    native_turn_id,
                    request_id,
                    &pending.questions,
                    devo_protocol::native::item::ItemState::Interrupted,
                    persisted,
                )
                .await;
            }
        }
        if removed_len > 0 {
            tracing::info!(
                session_id = %session_id,
                turn_id = %turn_id,
                removed_len,
                "cleared pending request_user_input requests for interrupted turn"
            );
        }

        Arc::clone(self)
            .interrupt_all_child_agents(session_id)
            .await;

        let snapshot = match tokio::time::timeout(TURN_INTERRUPT_TERMINAL_TIMEOUT, terminal_rx)
            .await
        {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(_)) | Err(_) => {
                if let Some(snapshot) = self.recent_terminal_turn_status(turn_id).await {
                    snapshot
                } else {
                    // Cooperative cancel timed out: hard-abort, then claim
                    // or recover any leftover active_turn without MergeTurn.
                    self.active_turns.abort_task(session_id).await;
                    if let Some(snapshot) = self.recent_terminal_turn_status(turn_id).await {
                        snapshot
                    } else if let Some(interrupted_turn) =
                        session_handle.interrupt_active_turn().await.flatten()
                    {
                        if interrupted_turn.native.id != turn_id {
                            return self.error_response(
                                request_id,
                                ProtocolErrorCode::TurnNotFound,
                                "turn does not exist",
                            );
                        }
                        return self
                            .finalize_claimed_interrupted_turn(
                                request_id,
                                &session_handle,
                                session_id,
                                interrupted_turn,
                            )
                            .await;
                    } else if let Some(orphaned) = self
                        .recover_orphaned_manual_compaction_interrupt(
                            &session_handle,
                            session_id,
                            turn_id,
                        )
                        .await
                    {
                        return self.turn_interrupt_success(request_id, turn_id, orphaned.status);
                    } else {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::TurnNotFound,
                            "turn is not active",
                        );
                    }
                }
            }
        };

        tracing::info!(
            session_id = %params.session_id,
            turn_id = %params.turn_id,
            status = ?snapshot.status,
            "interrupted turn"
        );

        self.turn_interrupt_success(request_id, turn_id, snapshot.status)
    }

    /// Safety net when interrupt abort raced past a compaction task that already
    /// cleared `active_turn` but never recorded a terminal status.
    pub(crate) async fn recover_orphaned_manual_compaction_interrupt(
        self: &Arc<Self>,
        session_handle: &crate::runtime::session_actor::SessionHandle,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> Option<TerminalTurnSnapshot> {
        let mut interrupted_turn = session_handle
            .turn_reservation_snapshot()
            .await?
            .active_turn?;
        if interrupted_turn.turn_id() != turn_id
            || interrupted_turn.native.kind != devo_protocol::native::turn::TurnKind::Compaction
        {
            return None;
        }
        if let Some(snapshot) = self.recent_terminal_turn_status(turn_id).await {
            return Some(snapshot);
        }

        // Wake any compaction task still holding `state_change_gate` inside
        // `compact_history` so admission can proceed after recovery.
        if let Some(cancel_token) = self.active_turns.cancel_token(session_id).await {
            cancel_token.cancel();
        }
        self.active_turns.abort_task(session_id).await;

        interrupted_turn.native.status = devo_protocol::native::turn::TurnStatus::Interrupted;
        interrupted_turn.native.completed_at = Some(Utc::now());
        session_handle
            .set_runtime_session_idle(Some(interrupted_turn.clone()))
            .await;
        self.clear_active_turn_runtime_handles(session_id).await;

        tracing::warn!(
            session_id = %session_id,
            turn_id = %turn_id,
            "recovered orphaned manual compaction interrupt"
        );
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::ContextCompactionFailed {
                session_id: interrupted_turn.native.session_id,
                message: "compaction canceled".to_string(),
            },
        )
        .await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnCompleted {
                turn: Box::new(interrupted_turn.native.clone()),
            },
        )
        .await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnCompleted {
                turn: Box::new(interrupted_turn.native.clone()),
            },
        )
        .await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::session_status_changed(
                session_id,
                SessionStatus::Idle,
                /*active_turn_id*/ None,
            ),
        )
        .await;
        let snapshot = TerminalTurnSnapshot::from_runtime_turn(&interrupted_turn);
        self.record_terminal_turn_status(turn_id, snapshot.clone())
            .await;
        Some(snapshot)
    }

    async fn finalize_claimed_interrupted_turn(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        session_handle: &crate::runtime::session_actor::SessionHandle,
        session_id: SessionId,
        interrupted_turn: crate::turn::RuntimeTurn,
    ) -> serde_json::Value {
        self.clear_active_turn_runtime_handles(session_id).await;

        let deferred = session_handle.take_deferred_items().await;
        if let Some((item_id, item_seq, text)) = deferred.assistant
            && !text.trim().is_empty()
        {
            self.complete_native_item(
                interrupted_turn.native.session_id,
                interrupted_turn.native.id,
                item_id,
                item_seq,
                devo_protocol::native::item::Item::AssistantMessage { text: text.clone() },
            )
            .await;
        }
        if let Some((item_id, item_seq, text)) = deferred.reasoning {
            self.complete_native_item(
                interrupted_turn.native.session_id,
                interrupted_turn.native.id,
                item_id,
                item_seq,
                devo_protocol::native::item::Item::Reasoning {
                    text: text.clone(),
                    provider_payload_ref: None,
                },
            )
            .await;
        }
        if let Some(persistence) = session_handle.turn_persistence_snapshot().await
            && persistence.rollout_path.is_some()
            && let Err(error) = self
                .persist_turn_line_deduped(session_id, &interrupted_turn)
                .await
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist interrupted turn: {error}"),
            );
        }
        tracing::info!(
            session_id = %session_id,
            turn_id = %interrupted_turn.turn_id(),
            status = ?interrupted_turn.native.status,
            "interrupted turn"
        );
        self.finalize_turn_workspace_changes(session_id, &interrupted_turn)
            .await;
        if interrupted_turn.native.kind == devo_protocol::native::turn::TurnKind::Compaction {
            // Manual compact dual-emits compaction lifecycle for existing UI;
            // abort may drop the compaction task before it can emit this itself.
            self.broadcast_notification(
                devo_protocol::native::event::ServerNotification::ContextCompactionFailed {
                    session_id: interrupted_turn.native.session_id,
                    message: "compaction canceled".to_string(),
                },
            )
            .await;
        }
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnCompleted {
                turn: Box::new(interrupted_turn.native.clone()),
            },
        )
        .await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnCompleted {
                turn: Box::new(interrupted_turn.native.clone()),
            },
        )
        .await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::session_status_changed(
                session_id,
                SessionStatus::Idle,
                /*active_turn_id*/ None,
            ),
        )
        .await;
        self.record_terminal_turn_status(
            interrupted_turn.turn_id(),
            TerminalTurnSnapshot::from_runtime_turn(&interrupted_turn),
        )
        .await;

        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            runtime.spawn_next_turn_from_queue(session_id).await;
        });

        self.turn_interrupt_success(
            request_id,
            interrupted_turn.turn_id(),
            TurnStatus::Interrupted,
        )
    }

    fn turn_interrupt_success(
        &self,
        request_id: serde_json::Value,
        turn_id: TurnId,
        status: TurnStatus,
    ) -> serde_json::Value {
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnInterruptResult { turn_id, status },
        })
        .expect("serialize interrupt response")
    }
}
