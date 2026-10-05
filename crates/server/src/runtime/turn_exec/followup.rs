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
        let Some(queued) = self
            .pop_next_queued_turn_input(
                session_id,
                /*require_idle_session*/ matches!(policy, QueueDrainPolicy::OnlyWhenIdle),
            )
            .await
        else {
            return false;
        };
        let Some((turn, turn_config)) = self.prepare_queued_turn_start(session_id, &queued).await
        else {
            return false;
        };
        if let Some((parent_session_id, parent_turn_id)) = queued.subagent_usage_owner {
            self.register_subagent_usage_owner(parent_session_id, session_id, parent_turn_id)
                .await;
        }
        self.activate_queued_turn(session_id, &turn, &turn_config, &queued.display_input)
            .await;
        drop(state_change_guard);
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
        tokio::spawn(async move {
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
                    input_mode: TurnInputMode::VisibleUserMessage,
                    user_message_already_emitted: false,
                })
                .await;
        });
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
        let queue_len_before = session_handle
            .pending_queue_snapshot()
            .await
            .map(|snapshot| snapshot.pending_count)
            .unwrap_or_default();
        let Some(queued) = self
            .pop_next_queued_turn_input(session_id, /*require_idle_session*/ false)
            .await
        else {
            if queue_len_before > 0 {
                tracing::warn!(
                    session_id = %session_id,
                    queue_len_before,
                    "post-turn chain found a non-empty queue but popped nothing (entries un-drainable?)"
                );
            }
            return false;
        };
        tracing::info!(
            session_id = %session_id,
            queued_input_id = %queued.queued_item_id,
            queue_len_before,
            "post-turn chain draining queued follow-up"
        );
        let Some((turn, turn_config)) = self.prepare_queued_turn_start(session_id, &queued).await
        else {
            return false;
        };
        if let Some((parent_session_id, parent_turn_id)) = queued.subagent_usage_owner {
            self.register_subagent_usage_owner(parent_session_id, session_id, parent_turn_id)
                .await;
        }
        self.activate_queued_turn(session_id, &turn, &turn_config, &queued.display_input)
            .await;
        drop(state_change_guard);
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
        Box::pin(Arc::clone(self).execute_turn(ExecuteTurnRequest {
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
            input_mode: TurnInputMode::VisibleUserMessage,
            user_message_already_emitted: false,
        }))
        .await;
        true
    }

    async fn pop_next_queued_turn_input(
        self: &Arc<Self>,
        session_id: SessionId,
        require_idle_session: bool,
    ) -> Option<QueuedTurnInput> {
        let session_handle = self.sessions.lock().await.get(&session_id).cloned()?;
        let popped = session_handle
            .pop_queued_turn_input(require_idle_session)
            .await
            .flatten()?;
        if let Err(error) = self.deps.db.remove_pending_by_id(
            &session_id,
            crate::db::QueueType::Turn,
            &popped.queued_input_id,
        ) {
            tracing::warn!(
                session_id = %session_id,
                queued_input_id = %popped.queued_input_id,
                error = %error,
                "failed to remove dequeued turn input from database"
            );
        }
        Some(QueuedTurnInput {
            // boundary: pending queue item id is already QueueItemId
            queued_item_id: popped.queued_input_id,
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
        let turn_config = reservation.runtime_context.resolve_turn_config(
            model_override,
            reservation.summary.settings.reasoning_effort.clone(),
        );
        let resolved_request = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());
        let request_model = turn_config.provider_request_model(&resolved_request.request_model);
        let sequence = reservation
            .latest_turn
            .as_ref()
            .map_or(1, |turn| turn.native.sequence + 1);
        let now = Utc::now();
        let native_turn_id = devo_protocol::native::ids::TurnId::new();
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
                variant: None,
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
    ) {
        if let Some(session_handle) = self.sessions.lock().await.get(&session_id).cloned() {
            session_handle
                .activate_runtime_queued_turn(turn.clone(), turn_config.clone())
                .await;
            self.add_active_turn_user_text(session_id, turn.turn_id(), display_input.to_string())
                .await;
        }
    }
}
