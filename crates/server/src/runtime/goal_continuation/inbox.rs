//! Goal eligibility produces an ordinary, durable FIFO entry. Consumption still
//! uses the hidden GoalContinuation turn contract, never synthetic human input.
use super::*;
use futures::future::BoxFuture;

impl ServerRuntime {
    pub(in crate::runtime) fn maybe_start_goal_continuation_turn(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let Some(handle) = self.session(session_id).await else {
                return;
            };
            if self
                .pause_goal_continuation_after_failed_turn(session_id, &handle)
                .await
            {
                return;
            }
            let guard = handle.lock_state_change().await;
            if !self.session_allows_goal_continuation(session_id).await {
                return;
            }
            let Some(candidate) = self.goal_continuation_candidate(session_id).await else {
                return;
            };
            let Some(reservation) = handle.turn_reservation_snapshot().await else {
                return;
            };
            let latest = reservation.latest_turn.as_ref().map(|turn| turn.native.id);
            if self.defer_goal_for_background_work(session_id, candidate.goal_id, latest) {
                return;
            }
            let mut item = devo_protocol::PendingInputItem::new(
                devo_protocol::PendingInputKind::UserText {
                    text: String::new(),
                },
                Some(serde_json::json!({"goalContinuation": {
                    "goalId": candidate.goal_id, "updatedAt": candidate.goal.updated_at
                }})),
                Utc::now(),
            );
            // The persisted item identity is stable for retries of this admission.
            // A goal deferred again by newly registered background work is a new
            // eligibility event, so it must not reuse a retired admission receipt.
            item.metadata
                .as_mut()
                .and_then(serde_json::Value::as_object_mut)
                .expect("goal metadata object")
                .insert(
                    "host_message_origin".into(),
                    serde_json::json!({
                        "eventId": item.id.to_string(), "source": "backgroundTask",
                        "sender": format!("goal:{}", candidate.goal_id),
                        "senderName": "Goal continuation", "delivery": "followUp"
                    }),
                );
            let admitted = match self.persist_inbox_item(
                session_id,
                reservation.ephemeral,
                &reservation.pending_turn_queue,
                &item,
            ) {
                Ok(admitted) => admitted,
                Err(error) => {
                    tracing::warn!(%session_id, %error, "failed to persist goal inbox input");
                    return;
                }
            };
            drop(guard);
            if admitted {
                self.broadcast_queue_updated(
                    session_id,
                    devo_protocol::native::queue::QueueChange::Added,
                    item.id,
                    None,
                )
                .await;
            }
            self.drain_queue_if_idle(session_id).await;
        })
    }

    /// Caller holds state_change_gate. None means a normal input is at the head;
    /// Some(false) means an obsolete goal was removed; Err holds uncertain input.
    pub(in crate::runtime) fn consume_goal_inbox_head(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> BoxFuture<'_, Result<Option<bool>, ()>> {
        Box::pin(async move {
            let Some(session_handle) = self.session(session_id).await else {
                return Err(());
            };
            let Some(reservation) = session_handle.turn_reservation_snapshot().await else {
                return Err(());
            };
            let item = reservation
                .pending_turn_queue
                .lock()
                .expect("pending queue mutex poisoned")
                .front()
                .cloned();
            let Some(item) = item else {
                return Ok(None);
            };
            let Some(marker) = item
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("goalContinuation"))
            else {
                return Ok(None);
            };
            if crate::runtime::handlers::queue_steer::requires_queue_review(&item) {
                return Err(());
            }
            if reservation.active_turn.is_some()
                || self
                    .session_interactive
                    .has_pending_interactive(session_id)
                    .await
            {
                return Err(());
            }
            let candidate = self.goal_continuation_candidate(session_id).await;
            let current = candidate.as_ref().is_some_and(|candidate| {
                marker.get("goalId") == Some(&serde_json::json!(candidate.goal_id))
                    && marker.get("updatedAt")
                        == Some(&serde_json::json!(candidate.goal.updated_at))
            }) && session_handle.collaboration_mode().await
                != Some(devo_protocol::CollaborationMode::Plan);
            let waiting = candidate.as_ref().is_some_and(|candidate| {
                self.defer_goal_for_background_work(
                    session_id,
                    candidate.goal_id,
                    reservation.latest_turn.as_ref().map(|turn| turn.native.id),
                )
            });
            if !current || waiting {
                {
                    let mut queue = reservation
                        .pending_turn_queue
                        .lock()
                        .expect("pending queue mutex poisoned");
                    if queue.front().map(|head| head.id) != Some(item.id) {
                        return Err(());
                    }
                    if !reservation.ephemeral
                        && let Err(error) = self.deps.db.remove_pending_by_id(
                            &session_id,
                            crate::db::QueueType::Turn,
                            &item.id,
                        )
                    {
                        tracing::warn!(%session_id, %error, "failed to discard obsolete goal input");
                        return Err(());
                    }
                    queue.pop_front();
                }
                self.broadcast_queue_updated(
                    session_id,
                    devo_protocol::native::queue::QueueChange::Removed,
                    item.id,
                    None,
                )
                .await;
                return Ok(Some(false));
            }
            let candidate = candidate.expect("validated goal candidate");
            let requested_model = session_model_selection(&reservation.summary);
            let requested_reasoning_effort_selection =
                reservation.summary.settings.reasoning_effort.clone();
            let turn_config = match reservation.runtime_context.resolve_session_turn_config(
                &reservation.summary,
                requested_model,
                requested_reasoning_effort_selection,
            ) {
                Ok(config) => config,
                Err(error) => {
                    tracing::warn!(%session_id, %error, "child goal model is unavailable");

                    let mut held = item.clone();
                    crate::runtime::handlers::queue_steer::hold_for_review(&mut held);
                    if !reservation.ephemeral
                        && let Err(persist_error) = self.deps.db.update_pending_content(
                            &session_id,
                            crate::db::QueueType::Turn,
                            &held,
                        )
                    {
                        tracing::error!(%session_id, %persist_error, "failed to hold invalid goal input");
                    }
                    if let Some(head) = reservation
                        .pending_turn_queue
                        .lock()
                        .expect("pending queue mutex poisoned")
                        .front_mut()
                        && head.id == held.id
                    {
                        *head = held.clone();
                    }
                    if let Some(parent) = reservation.summary.parent_session_id() {
                        self.set_agent_status(parent, session_id, SubagentStatus::Failed)
                            .await;
                        let runtime = Arc::clone(self);
                        let error_message = error.to_string();
                        tokio::spawn(async move {
                            // Let the caller release admission before external
                            // SubagentStop hooks or parent notification work.
                            if let Some(handle) = runtime.session(session_id).await {
                                let gate = handle.lock_state_change().await;
                                drop(gate);
                            }
                            let _ = runtime
                                .deliver_host_message(
                                    parent,
                                    super::super::host_messages::HostMessage::parent_steer(
                                        session_id,
                                        format!("goal:{}:invalid-model", held.id),
                                        format!("Child goal cannot start: {error_message}"),
                                    ),
                                    None,
                                )
                                .await;
                            runtime.run_subagent_stop_hook(session_id).await;
                        });
                    }
                    self.broadcast_queue_updated(
                        session_id,
                        devo_protocol::native::queue::QueueChange::Updated,
                        item.id,
                        None,
                    )
                    .await;
                    return Err(());
                }
            };
            let resolved_request = turn_config.model.resolve_reasoning_effort_selection(
                turn_config.reasoning_effort_selection.as_deref(),
            );
            let request_model = turn_config.provider_request_model(&resolved_request.request_model);

            let now = Utc::now();
            let native_turn_id = devo_protocol::native::ids::TurnId::new();
            let turn_id = native_turn_id;
            let native_session_id = reservation.summary.native.id;
            let runtime_turn = crate::turn::RuntimeTurn {
                native: devo_protocol::native::turn::Turn {
                    id: native_turn_id,
                    session_id: native_session_id,
                    sequence: reservation
                        .latest_turn
                        .as_ref()
                        .map_or(1, |turn| turn.native.sequence + 1),
                    kind: devo_protocol::native::turn::TurnKind::GoalContinuation,
                    status: devo_protocol::native::turn::TurnStatus::InProgress,
                    model: devo_protocol::native::model::ModelBinding {
                        provider: turn_config
                            .model_binding_id
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string()),
                        model: request_model,
                        variant: turn_config.variant.clone(),
                        reasoning_effort: resolved_request.effective_reasoning_effort,
                    },
                    collaboration_mode: None,
                    started_at: now,
                    completed_at: None,
                    error: None,
                    usage: None,
                },
                extras: crate::turn::RuntimeTurnExtras {
                    request_thinking: resolved_request.request_thinking.clone(),
                    stop_reason: None,
                    failure_reason: None,
                },
            };
            {
                let mut queue = reservation
                    .pending_turn_queue
                    .lock()
                    .expect("pending queue mutex poisoned");
                if queue.front().map(|head| head.id) != Some(item.id) {
                    return Err(());
                }
                if !reservation.ephemeral {
                    match self.deps.db.claim_queue_steer(&session_id, &item, &turn_id) {
                        Ok(true) => {}
                        Ok(false) => return Err(()),
                        Err(error) => {
                            tracing::warn!(%session_id, %error, "failed to claim goal inbox input");
                            return Err(());
                        }
                    }
                }
                queue.pop_front();
            }
            session_handle
                .begin_runtime_turn(runtime_turn.clone(), turn_config.clone())
                .await;
            self.active_goal_continuation_turns
                .lock()
                .await
                .insert(session_id, turn_id);
            self.goal_continuation_turn_goals
                .lock()
                .await
                .insert(turn_id, candidate.goal_id);
            if !self
                .mark_goal_continuation_turn_started(session_id, &candidate.goal_id, turn_id)
                .await
            {
                self.clear_goal_continuation_turn_reservation(&session_handle, turn_id)
                    .await;
                self.restore_claimed_inbox_head_locked(session_id, item.clone(), turn_id)
                    .await;
                return Err(());
            }

            if let Err(error) = self
                .append_goal_continuation_turn_start(session_id, &runtime_turn)
                .await
            {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn_id,
                    error = %error,
                    "failed to persist goal continuation turn start"
                );
                self.clear_goal_continuation_turn_reservation(&session_handle, turn_id)
                    .await;
                self.restore_claimed_inbox_head_locked(session_id, item.clone(), turn_id)
                    .await;
                return Err(());
            }
            if !goal_continuation_turn_still_current(
                self,
                &session_handle,
                session_id,
                turn_id,
                &candidate.goal_id,
            )
            .await
            {
                self.clear_goal_continuation_turn_reservation(&session_handle, turn_id)
                    .await;
                self.restore_claimed_inbox_head_locked(session_id, item.clone(), turn_id)
                    .await;
                return Err(());
            }

            self.broadcast_queue_updated(
                session_id,
                devo_protocol::native::queue::QueueChange::Drained,
                item.id,
                Some(turn_id),
            )
            .await;
            self.broadcast_notification(
                devo_protocol::native::event::ServerNotification::session_status_changed(
                    session_id,
                    SessionStatus::Active,
                    /*active_turn_id*/ None,
                ),
            )
            .await;
            if !goal_continuation_turn_still_current(
                self,
                &session_handle,
                session_id,
                turn_id,
                &candidate.goal_id,
            )
            .await
            {
                self.clear_goal_continuation_turn_reservation(&session_handle, turn_id)
                    .await;
                self.restore_claimed_inbox_head_locked(session_id, item.clone(), turn_id)
                    .await;
                return Err(());
            }
            self.broadcast_notification(
                devo_protocol::native::event::ServerNotification::TurnStarted {
                    turn: Box::new(runtime_turn.native.clone()),
                },
            )
            .await;
            if !goal_continuation_turn_still_current(
                self,
                &session_handle,
                session_id,
                turn_id,
                &candidate.goal_id,
            )
            .await
            {
                self.clear_goal_continuation_turn_reservation(&session_handle, turn_id)
                    .await;
                self.restore_claimed_inbox_head_locked(session_id, item.clone(), turn_id)
                    .await;
                return Err(());
            }

            let runtime = Arc::clone(self);
            let task_turn = runtime_turn.clone();
            let task_turn_config = turn_config.clone();
            let task_goal = candidate.goal.to_thread_goal();
            let spawn_session_id = session_id;
            let task_started = {
                let tracked_turns = self.active_goal_continuation_turns.lock().await;
                let still_reserved = tracked_turns
                    .get(&session_id)
                    .is_some_and(|tracked_turn_id| *tracked_turn_id == turn_id);
                if !still_reserved {
                    false
                } else {
                    let still_active_turn = session_handle
                        .active_turn_id()
                        .await
                        .is_some_and(|active_turn_id| active_turn_id == Some(turn_id));
                    if !still_active_turn {
                        false
                    } else {
                        self.register_active_runtime_turn_execution(session_id, runtime_turn, None)
                            .await;
                        let task = tokio::spawn(async move {
                            runtime
                                .execute_turn(ExecuteTurnRequest {
                                    session_id: spawn_session_id,
                                    turn: task_turn,
                                    turn_config: task_turn_config,
                                    display_input: String::new(),
                                    client_user_message_id: None,
                                    input: String::new(),
                                    input_messages: Vec::new(),
                                    input_images: Vec::new(),
                                    input_image_paths: Vec::new(),
                                    collaboration_mode: devo_protocol::CollaborationMode::Build,
                                    input_mode: TurnInputMode::HiddenGoalContinuation {
                                        goal: task_goal,
                                    },
                                    user_message_already_emitted: false,
                                })
                                .await;
                        });
                        self.active_turns
                            .set_abort_handle(session_id, task.abort_handle())
                            .await;
                        true
                    }
                }
            };
            if !task_started {
                self.clear_goal_continuation_turn_reservation(&session_handle, turn_id)
                    .await;
                self.restore_claimed_inbox_head_locked(session_id, item.clone(), turn_id)
                    .await;
            }
            Ok(Some(task_started))
        })
    }
}
