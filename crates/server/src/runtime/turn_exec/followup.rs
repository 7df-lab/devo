use std::sync::Arc;

use chrono::Utc;
use devo_protocol::native::ids::SessionId;

use super::super::*;
use super::types::{ExecuteTurnRequest, QueuedTurnInput};

/// Whether a queued-input drain may pop while a turn is still active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime) enum QueueDrainPolicy {
    /// Pop regardless of active-turn state (post-turn chain, interrupt
    /// recovery): the caller knows the turn is winding down.
    Always,
    /// Pop only when no turn is active, so a race-recovery kick can never
    /// double-admit a turn.
    OnlyWhenIdle,
}

impl ServerRuntime {
    /// Pop the first queued input and start a new turn in a background task.
    /// Used from the interrupt handler where the calling function must return
    /// its response immediately.
    pub(in crate::runtime) async fn spawn_next_turn_from_queue(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> bool {
        self.spawn_next_turn_from_queue_with_policy(session_id, QueueDrainPolicy::Always)
            .await
    }

    /// Starts a queued turn only when the session is idle. Gate-free busy
    /// enqueues (`handle_turn_start_with_queue_policy`) can race the
    /// post-turn drain: if the active turn ended between the pusher's
    /// snapshot and the enqueue and the followup chain already found an
    /// empty queue, this kick is what keeps the entry from stranding.
    pub(in crate::runtime) async fn drain_queue_if_idle(self: &Arc<Self>, session_id: SessionId) {
        let Some(reservation) = self.session_turn_reservation_snapshot(session_id).await else {
            return;
        };
        if reservation.active_turn.is_some()
            || reservation
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned")
                .is_empty()
        {
            return;
        }
        self.spawn_next_turn_from_queue_with_policy(session_id, QueueDrainPolicy::OnlyWhenIdle)
            .await;
    }

    /// After hydrate/resume, start the next queued turn when the session is idle.
    pub(in crate::runtime) async fn resume_pending_queue_if_idle(
        self: &Arc<Self>,
        session_id: SessionId,
    ) {
        self.drain_queue_if_idle(session_id).await;
    }

    async fn spawn_next_turn_from_queue_with_policy(
        self: &Arc<Self>,
        session_id: SessionId,
        policy: QueueDrainPolicy,
    ) -> bool {
        let Some(session_handle) = self.session(session_id).await else {
            return false;
        };
        let state_change_guard = session_handle.lock_state_change().await;
        // A newer turn (for example a message-edit replacement) may have won
        // admission since the old turn released its runtime handles. Never
        // drain or overwrite that execution, even for an Always-policy kick.
        if self.runtime_active_turn_id(session_id).await.is_some() {
            return false;
        }
        let (queued, turn, turn_config) = loop {
            if self.hold_ineligible_inbox_head(session_id).await {
                return false;
            }
            match self.consume_goal_inbox_head(session_id).await {
                Ok(Some(true)) => return true,
                Ok(Some(false)) => continue,
                Ok(None) => {}
                Err(()) => return false,
            }
            let Some(queued) = self
                .pop_next_queued_turn_input(
                    session_id,
                    /*require_idle_session*/
                    matches!(policy, QueueDrainPolicy::OnlyWhenIdle),
                )
                .await
            else {
                return false;
            };
            if let Some((turn, config)) = self.prepare_queued_turn_start(session_id, &queued).await
            {
                break (queued, turn, config);
            }
            // A known invalid binding is rejected before any execution; continue.
        };
        if let Some((parent_session_id, parent_turn_id)) = queued.subagent_usage_owner {
            self.register_subagent_usage_owner(parent_session_id, session_id, parent_turn_id)
                .await;
        }
        if let Err(error) = self
            .activate_queued_turn(session_id, &turn, &turn_config, &queued.display_input)
            .await
        {
            tracing::error!(%session_id, %error, "failed to persist queued turn start");
            self.restore_claimed_inbox_head_locked(session_id, queued.original, turn.turn_id())
                .await;
            return false;
        }
        if let Some((parent_session_id, _)) = self.child_parent_and_path(session_id).await {
            self.complete_background_work(
                parent_session_id,
                format!("agent:{session_id}:queued:{}", queued.queued_item_id),
                String::new(),
            );
        }
        drop(state_change_guard);
        // Roster subscribers observe session lifecycle, not per-session turns.
        // Host messages (including child startup) also enter through this path.
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::session_status_changed(
                session_id,
                SessionStatus::Active,
                Some(turn.native.id),
            ),
        )
        .await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnStarted {
                turn: Box::new(turn.native.clone()),
            },
        )
        .await;
        // `drained` carries queueItemId + startedTurnId, emitted in the same
        // session-actor operation as the matching turn/started so handles
        // bind atomically (01 §4.3).
        self.broadcast_queue_updated(
            session_id,
            devo_protocol::native::queue::QueueChange::Drained,
            queued.queued_item_id,
            Some(turn.native.id),
        )
        .await;
        let runtime = Arc::clone(self);
        drop(
            self.spawn_registered_active_turn_task(session_id, async move {
                runtime
                    .execute_turn(ExecuteTurnRequest {
                        session_id,
                        turn,
                        turn_config,
                        display_input: queued.display_input,
                        client_user_message_id: queued.client_user_message_id,
                        input: queued.input_text,
                        input_messages: queued.input_messages,
                        input_images: queued.input_images,
                        input_image_paths: queued.input_image_paths,
                        collaboration_mode: queued.collaboration_mode,
                        input_mode: queued.host_origin.map_or(
                            TurnInputMode::VisibleUserMessage,
                            TurnInputMode::HostMessage,
                        ),
                        user_message_already_emitted: false,
                    })
                    .await;
            })
            .await,
        );
        true
    }

    /// After a turn completes, chain directly into the next queued input when present.
    pub(crate) async fn chain_queued_followup_turn(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> bool {
        let Some(session_handle) = self.session(session_id).await else {
            return false;
        };
        let state_change_guard = session_handle.lock_state_change().await;
        // Post-turn scheduling is spawned after cleanup, not atomically with
        // it. Another turn can already own admission by the time we get here.
        if self.runtime_active_turn_id(session_id).await.is_some() {
            return false;
        }
        let queue_len_before = session_handle
            .pending_queue_snapshot()
            .await
            .map(|snapshot| snapshot.pending_count)
            .unwrap_or_default();
        let (queued, turn, turn_config) = loop {
            if self.hold_ineligible_inbox_head(session_id).await {
                return false;
            }
            match self.consume_goal_inbox_head(session_id).await {
                Ok(Some(true)) => return true,
                Ok(Some(false)) => continue,
                Ok(None) => {}
                Err(()) => return false,
            }
            let Some(queued) = self
                .pop_next_queued_turn_input(session_id, /*require_idle_session*/ false)
                .await
            else {
                if queue_len_before > 0 {
                    tracing::debug!(%session_id, queue_len_before, "post-turn queue drain finished without an admissible input");
                }
                return false;
            };
            tracing::info!(%session_id, queued_input_id = %queued.queued_item_id, queue_len_before,
                "post-turn chain draining queued follow-up");
            if let Some((turn, config)) = self.prepare_queued_turn_start(session_id, &queued).await
            {
                break (queued, turn, config);
            }
        };
        if let Some((parent_session_id, parent_turn_id)) = queued.subagent_usage_owner {
            self.register_subagent_usage_owner(parent_session_id, session_id, parent_turn_id)
                .await;
        }
        if let Err(error) = self
            .activate_queued_turn(session_id, &turn, &turn_config, &queued.display_input)
            .await
        {
            tracing::error!(%session_id, %error, "failed to persist queued turn start");
            self.restore_claimed_inbox_head_locked(session_id, queued.original, turn.turn_id())
                .await;
            return false;
        }
        if let Some((parent_session_id, _)) = self.child_parent_and_path(session_id).await {
            self.complete_background_work(
                parent_session_id,
                format!("agent:{session_id}:queued:{}", queued.queued_item_id),
                String::new(),
            );
        }
        drop(state_change_guard);
        // Roster subscribers observe session lifecycle, not per-session turns.
        // Host messages (including child startup) also enter through this path.
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::session_status_changed(
                session_id,
                SessionStatus::Active,
                Some(turn.native.id),
            ),
        )
        .await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnStarted {
                turn: Box::new(turn.native.clone()),
            },
        )
        .await;
        self.broadcast_queue_updated(
            session_id,
            devo_protocol::native::queue::QueueChange::Drained,
            queued.queued_item_id,
            Some(turn.native.id),
        )
        .await;
        let runtime = Arc::clone(self);
        let task = self
            .spawn_registered_active_turn_task(session_id, async move {
                Box::pin(runtime.execute_turn(ExecuteTurnRequest {
                    session_id,
                    turn,
                    turn_config,
                    display_input: queued.display_input,
                    client_user_message_id: queued.client_user_message_id,
                    input: queued.input_text,
                    input_messages: queued.input_messages,
                    input_images: queued.input_images,
                    input_image_paths: queued.input_image_paths,
                    collaboration_mode: queued.collaboration_mode,
                    input_mode: queued.host_origin.map_or(
                        TurnInputMode::VisibleUserMessage,
                        TurnInputMode::HostMessage,
                    ),
                    user_message_already_emitted: false,
                }))
                .await;
            })
            .await;
        let _ = task.await;
        true
    }

    async fn pop_next_queued_turn_input(
        self: &Arc<Self>,
        session_id: SessionId,
        require_idle_session: bool,
    ) -> Option<QueuedTurnInput> {
        let session_handle = self.sessions.lock().await.get(&session_id).cloned()?;
        let reservation = session_handle.turn_reservation_snapshot().await?;
        if require_idle_session && reservation.active_turn.is_some() {
            return None;
        }
        let claimed_turn_id = TurnId::new();
        let original = {
            let queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending queue mutex poisoned");
            let item = queue.front()?.clone();
            if crate::runtime::handlers::queue_steer::requires_queue_review(&item) {
                return None;
            }
            if !reservation.ephemeral {
                match self
                    .deps
                    .db
                    .claim_queue_steer(&session_id, &item, &claimed_turn_id)
                {
                    Ok(true) => {}
                    Ok(false) => return None,
                    Err(error) => {
                        tracing::error!(%session_id, %error, "inbox claim failed");
                        return None;
                    }
                }
            }
            if reservation.ephemeral {
                self.ephemeral_inbox_claimed
                    .lock()
                    .expect("ephemeral inbox claims poisoned")
                    .insert((session_id, item.id));
            }
            item
        };
        let popped = session_handle
            .pop_queued_turn_input(require_idle_session)
            .await
            .flatten()?;
        Some(QueuedTurnInput {
            claimed_turn_id,
            original,
            // boundary: pending queue item id is already QueueItemId
            queued_item_id: popped.queued_input_id,
            host_origin: popped.host_origin,
            client_user_message_id: popped.client_user_message_id,
            display_input: popped.display_input,
            input_text: popped.input_text,
            input_messages: popped.input_messages,
            input_images: popped.input_images,
            input_image_paths: popped.input_image_paths,
            collaboration_mode: popped.collaboration_mode,
            model_selection: popped.model_selection,
            subagent_usage_owner: popped.subagent_usage_owner,
        })
    }

    async fn prepare_queued_turn_start(
        self: &Arc<Self>,
        session_id: SessionId,
        queued: &QueuedTurnInput,
    ) -> Option<(crate::turn::RuntimeTurn, devo_core::TurnConfig)> {
        let session_handle = self.sessions.lock().await.get(&session_id).cloned()?;
        let reservation = session_handle.turn_reservation_snapshot().await?;
        let model_override = queued
            .model_selection
            .as_deref()
            .or_else(|| session_model_selection(&reservation.summary));
        let turn_config = match reservation.runtime_context.resolve_session_turn_config(
            &reservation.summary,
            model_override,
            reservation.summary.settings.reasoning_effort.clone(),
        ) {
            Ok(config) => config,
            Err(error) => {
                let message = format!("Queued child turn could not start: {error}");
                if let Some(parent) = reservation.summary.parent_session_id() {
                    let event_id = format!("agent:{session_id}:queued:{}", queued.queued_item_id);
                    self.publish_background_completion(
                        parent,
                        event_id.clone(),
                        super::super::host_messages::HostMessage::parent_steer(
                            session_id,
                            event_id,
                            message.clone(),
                        ),
                    );
                    self.set_agent_status(parent, session_id, SubagentStatus::Failed)
                        .await;
                    self.record_subagent_status_event_with_text(
                        parent,
                        session_id,
                        SubagentStatus::Failed,
                        TurnId::new(),
                        Some(message),
                    )
                    .await;
                }
                if !reservation.ephemeral
                    && let Err(error) = self.deps.db.remove_pending_by_id(
                        &session_id,
                        crate::db::QueueType::Steer,
                        &queued.queued_item_id,
                    )
                {
                    tracing::error!(%session_id, %error, "failed to settle rejected input");
                    self.restore_claimed_inbox_head_locked(
                        session_id,
                        queued.original.clone(),
                        queued.claimed_turn_id,
                    )
                    .await;
                    return None;
                }
                self.broadcast_queue_updated(
                    session_id,
                    devo_protocol::native::queue::QueueChange::Removed,
                    queued.queued_item_id,
                    None,
                )
                .await;
                return None;
            }
        };
        let resolved_request = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());
        let request_model = turn_config.provider_request_model(&resolved_request.request_model);
        let sequence = reservation
            .latest_turn
            .as_ref()
            .map_or(1, |turn| turn.native.sequence + 1);
        let now = Utc::now();
        let native_turn_id = queued.claimed_turn_id;
        let native_session_id = reservation.summary.native.id;
        let native = devo_protocol::native::turn::Turn {
            id: native_turn_id,
            session_id: native_session_id,
            sequence,
            status: devo_protocol::native::turn::TurnStatus::InProgress,
            kind: devo_protocol::native::turn::TurnKind::Regular,
            model: devo_protocol::native::model::ModelBinding {
                provider: turn_config
                    .model_binding_id
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                model: request_model,
                variant: turn_config.variant.clone(),
                reasoning_effort: turn_config
                    .reasoning_effort_selection
                    .as_deref()
                    .and_then(|selection| selection.parse().ok())
                    .or(resolved_request.effective_reasoning_effort),
            },
            collaboration_mode: Some(queued.collaboration_mode),
            started_at: now,
            completed_at: None,
            usage: None,
            error: None,
        };
        let turn = crate::turn::RuntimeTurn::new(
            native,
            crate::turn::RuntimeTurnExtras {
                request_thinking: resolved_request.request_thinking.clone(),
                stop_reason: None,
                failure_reason: None,
            },
        );
        Some((turn, turn_config))
    }

    async fn activate_queued_turn(
        self: &Arc<Self>,
        session_id: SessionId,
        turn: &crate::turn::RuntimeTurn,
        turn_config: &devo_core::TurnConfig,
        display_input: &str,
    ) -> anyhow::Result<()> {
        if let Some(session_handle) = self.session(session_id).await {
            session_handle
                .activate_runtime_queued_turn(turn.clone(), turn_config.clone())
                .await;
            if session_handle
                .turn_persistence_snapshot()
                .await
                .is_some_and(|snapshot| snapshot.rollout_path.is_some())
            {
                self.persist_turn_line_deduped(session_id, turn).await?;
            }
            if let Some(spawn) = session_handle.spawn_snapshot().await {
                self.register_turn_spawn_snapshot(session_id, turn.turn_id(), Arc::new(spawn))
                    .await;
            }
            self.register_active_runtime_turn_execution(
                session_id,
                turn.clone(),
                /*connection_id*/ None,
            )
            .await;
            self.add_active_turn_user_text(session_id, turn.turn_id(), display_input.to_string())
                .await;
            if let Some((parent, _)) = self.child_parent_and_path(session_id).await {
                self.active_turns
                    .copy_connection_from_parent(session_id, parent)
                    .await;
                self.set_agent_status(parent, session_id, SubagentStatus::Running)
                    .await;
            }
        } else {
            anyhow::bail!("session disappeared before queued turn activation");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
