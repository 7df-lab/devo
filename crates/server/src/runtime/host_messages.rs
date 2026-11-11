//! Typed, host-only input ingress. Existing agent and background producers use
//! follow-up delivery: busy turns are not interrupted; idle sessions wake.
//! This is not an RPC and no producer can choose a human or system origin.
use super::*;
use devo_protocol::native::host_message::{
    HostMessageDelivery, HostMessageOrigin, HostMessageSource,
};

pub(super) struct HostMessage {
    origin: HostMessageOrigin,
    text: String,
}

impl HostMessage {
    pub(super) fn agent(sender: SessionId, event_id: String, text: String) -> Self {
        Self {
            origin: HostMessageOrigin {
                event_id,
                source: HostMessageSource::Agent,
                sender: sender.to_string(),
                sender_name: None,
                delivery: HostMessageDelivery::FollowUp,
            },
            text,
        }
    }

    pub(super) fn background(
        session_id: SessionId,
        notice: &serde_json::Value,
        text: String,
    ) -> Self {
        let kind = notice["kind"].as_str().unwrap_or("background");
        let params = &notice["params"];
        let identity = params
            .get("handle_id")
            .or_else(|| params.get("cellId"))
            .or_else(|| params.get("pid"))
            .map(ToString::to_string)
            .unwrap_or_else(|| "unknown".into());
        Self {
            origin: HostMessageOrigin {
                event_id: notice["eventId"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                source: HostMessageSource::BackgroundTask,
                sender: format!("{session_id}:{kind}:{identity}"),
                sender_name: None,
                delivery: HostMessageDelivery::FollowUp,
            },
            text,
        }
    }
}

impl ServerRuntime {
    pub(super) async fn deliver_host_message(
        self: &Arc<Self>,
        session_id: SessionId,
        message: HostMessage,
        queued_metadata: Option<serde_json::Value>,
    ) -> Result<crate::turn::RuntimeTurn, ToolCallError> {
        let HostMessage { mut origin, text } = message;
        if origin.source == HostMessageSource::Agent
            && let Some(sender) = self
                .session(SessionId::from_string(origin.sender.clone()))
                .await
            && let Some(summary) = sender.summary().await
        {
            origin.sender_name = summary.agent_nickname.or(summary.native.title);
        }
        let display_input = text.clone();
        let input_text = text;
        let mut metadata = queued_metadata
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        metadata.insert(
            "host_message_origin".into(),
            serde_json::to_value(&origin).expect("host origin serializes"),
        );
        let queued_metadata = Some(serde_json::Value::Object(metadata));
        let session_handle = self.session(session_id).await.ok_or_else(|| {
            ToolCallError::InvalidInput(format!("session not found: {session_id}"))
        })?;
        let _state_change_guard = session_handle.lock_state_change().await;
        if let Some((parent_session_id, _)) = self.child_parent_and_path(session_id).await
            && self
                .agent_close_requested(parent_session_id, session_id)
                .await
        {
            return Err(ToolCallError::InvalidInput(
                "child agent is closed".to_string(),
            ));
        }
        if let Some(rollout_path) = session_handle.rollout_path().await.flatten()
            && self.rollout_store.journal_is_closed(&rollout_path)
        {
            return Err(ToolCallError::InvalidInput(format!(
                "session journal for {session_id} was deleted"
            )));
        }

        let reservation = self
            .session_turn_reservation_snapshot(session_id)
            .await
            .ok_or_else(|| {
                ToolCallError::InvalidInput(format!(
                    "failed to snapshot session reservation: {session_id}"
                ))
            })?;
        // Resolve admission without round-tripping this actor's own mailbox:
        // the reservation snapshot already carries the parent/root identity.
        let lifecycle_root = if reservation.ephemeral {
            match reservation.parent_session_id {
                Some(parent_session_id) => self
                    .resolve_lifecycle_root(parent_session_id, None)
                    .await
                    .map_err(|error| ToolCallError::InternalError(error.to_string()))?,
                None => session_id,
            }
        } else {
            self.resolve_lifecycle_root(session_id, None)
                .await
                .map_err(|error| match error {
                    crate::runtime::session_deletion::LifecycleRootError::MissingEvidence {
                        ..
                    } => ToolCallError::InvalidInput(format!(
                        "session has no lifecycle evidence: {session_id}"
                    )),
                    other => ToolCallError::InternalError(other.to_string()),
                })?
        };
        let _lifecycle_admission = self
            .begin_lifecycle_operation_for_root(lifecycle_root)
            .map_err(ToolCallError::InvalidInput)?;

        if reservation.max_turns.is_some_and(|max_turns| {
            max_turns == 0
                || reservation
                    .active_turn
                    .as_ref()
                    .is_some_and(|turn| turn.native.sequence >= max_turns)
                || reservation
                    .latest_turn
                    .as_ref()
                    .is_some_and(|turn| turn.native.sequence >= max_turns)
        }) {
            return Err(ToolCallError::InvalidInput(
                "agent maximum turn count reached".to_string(),
            ));
        }

        if let Some(active_turn) = reservation.active_turn.clone() {
            let item = devo_protocol::PendingInputItem::new(
                devo_protocol::PendingInputKind::UserText { text: input_text },
                queued_metadata,
                Utc::now(),
            );
            if !reservation.ephemeral {
                self.deps
                    .db
                    .push_pending(&session_id, QueueType::Turn, &item)
                    .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
            }
            if let Some((parent_session_id, _)) = self.child_parent_and_path(session_id).await {
                self.register_background_work(
                    parent_session_id,
                    format!("agent:{session_id}:queued:{}", item.id),
                );
            }
            session_handle
                .enqueue_pending_turn_input(item.clone())
                .await;
            // Enqueue acknowledgement is a mailbox barrier before the idle kick.
            let _ = session_handle.pending_queue_snapshot().await;
            drop(_state_change_guard);
            self.broadcast_queue_updated(
                session_id,
                devo_protocol::native::queue::QueueChange::Added,
                item.id,
                None,
            )
            .await;
            self.drain_queue_if_idle(session_id).await;
            return Ok(active_turn);
        }

        let turn_config = reservation
            .runtime_context
            .resolve_session_turn_config(
                &reservation.summary,
                session_model_selection(&reservation.summary),
                reservation.summary.settings.reasoning_effort.clone(),
            )
            .map_err(|error| ToolCallError::InvalidInput(error.to_string()))?;
        let resolved_request = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());

        let request_model = turn_config.provider_request_model(&resolved_request.request_model);
        let now = Utc::now();
        let sequence = reservation
            .latest_turn
            .as_ref()
            .map_or(1, |turn| turn.native.sequence + 1);
        let native_turn_id = devo_protocol::native::ids::TurnId::new();
        let turn_id = native_turn_id;
        let native_session_id = reservation.summary.native.id;
        let runtime_turn = crate::turn::RuntimeTurn {
            native: devo_protocol::native::turn::Turn {
                id: native_turn_id,
                session_id: native_session_id,
                sequence,
                kind: devo_protocol::native::turn::TurnKind::Regular,
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
                collaboration_mode: Some(devo_protocol::CollaborationMode::Build),
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

        session_handle
            .begin_runtime_turn(runtime_turn.clone(), turn_config.clone())
            .await;

        if let Err(error) = self.append_turn_start(session_id, &runtime_turn).await {
            let _ = session_handle.clear_active_turn_if_matches(turn_id).await;
            return Err(error);
        }

        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::session_status_changed(
                session_id,
                SessionStatus::Active,
                /*active_turn_id*/ None,
            ),
        )
        .await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnStarted {
                turn: Box::new(runtime_turn.native.clone()),
            },
        )
        .await;

        self.add_active_turn_user_text(session_id, turn_id, display_input.clone())
            .await;
        let runtime = Arc::clone(self);
        let turn_for_task = runtime_turn.clone();
        let turn_config_for_task = turn_config.clone();
        if let Some(parent_session_id) = reservation.parent_session_id {
            self.active_turns
                .copy_connection_from_parent(session_id, parent_session_id)
                .await;
        }
        self.spawn_active_runtime_turn_task(session_id, runtime_turn.clone(), None, async move {
            runtime
                .execute_turn(ExecuteTurnRequest {
                    session_id,
                    turn: turn_for_task,
                    turn_config: turn_config_for_task,
                    display_input,
                    client_user_message_id: None,
                    input: input_text,
                    input_messages: Vec::new(),
                    input_images: Vec::new(),
                    input_image_paths: Vec::new(),
                    collaboration_mode: devo_protocol::CollaborationMode::Build,
                    input_mode: TurnInputMode::HostMessage(origin),
                    user_message_already_emitted: false,
                })
                .await;
        })
        .await;
        Ok(runtime_turn)
    }
}

#[cfg(test)]
mod tests;
