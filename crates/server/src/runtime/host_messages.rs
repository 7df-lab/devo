//! Typed, host-only input ingress. Existing agent and background producers use
//! follow-up delivery, except explicit self messages use safe-boundary steering.
//! All delivery preserves inbox order; idle sessions wake.
//! This is not an RPC and no producer can choose a human or system origin.
use super::*;
use devo_protocol::native::host_message::{
    HostMessageDelivery, HostMessageOrigin, HostMessageSource,
};

#[derive(Debug)]
pub(super) struct HostDeliveryReceipt {
    pub(super) queue_item_id: devo_protocol::QueueItemId,
}

#[derive(Clone)]
pub(super) struct HostMessage {
    origin: HostMessageOrigin,
    text: String,
}

impl HostMessage {
    /// Explicit self input may steer only at an eligible FIFO model boundary.
    pub(super) fn self_steer(sender: SessionId, event_id: String, text: String) -> Self {
        Self {
            origin: HostMessageOrigin {
                event_id,
                source: HostMessageSource::Agent,
                sender: sender.to_string(),
                sender_name: None,
                delivery: HostMessageDelivery::Steer,
            },
            text,
        }
    }

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

    pub(super) fn child_completion(
        child_id: SessionId,
        turn_id: TurnId,
        status: &str,
        text: String,
    ) -> Self {
        Self::agent(
            child_id,
            format!("child:{child_id}:{turn_id}:{status}"),
            text,
        )
    }

    pub(super) fn managed_completion(owner: SessionId, work_id: String, text: String) -> Self {
        Self {
            origin: HostMessageOrigin {
                event_id: format!("background:{owner}:{work_id}:completed"),
                source: HostMessageSource::BackgroundTask,
                sender: format!("{owner}:{work_id}"),
                sender_name: None,
                delivery: HostMessageDelivery::FollowUp,
            },
            text,
        }
    }

    pub(super) fn inbox_identity(&self, owner: SessionId) -> (devo_protocol::QueueItemId, String) {
        let origin = &self.origin;
        (
            devo_protocol::QueueItemId::from_string(format!(
                "qitem_host:{}",
                serde_json::to_string(&(owner, &origin.sender, &origin.event_id))
                    .expect("host inbox identity serializes")
            )),
            serde_json::to_string(&("host", &origin.sender, &origin.event_id))
                .expect("host admission identity serializes"),
        )
    }

    pub(super) fn schedule(
        session_id: SessionId,
        occurrence_id: String,
        text: String,
        delivery: HostMessageDelivery,
    ) -> Self {
        Self {
            origin: HostMessageOrigin {
                event_id: occurrence_id,
                source: HostMessageSource::Schedule,
                sender: session_id.to_string(),
                sender_name: None,
                delivery,
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
        let work_id = match kind {
            "async_bash_completion" => params["handle_id"].as_str().map(|id| format!("bash:{id}")),
            "async_python_completion" => params["workId"].as_str().map(str::to_owned),
            _ => None,
        };
        if let Some(work_id) = work_id {
            return Self::managed_completion(session_id, work_id, text);
        }
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
                    .unwrap_or_else(|| {
                        if params.get("handle_id").is_some() || params.get("cellId").is_some() {
                            format!("{session_id}:{kind}:{identity}:completed")
                        } else {
                            // Legacy PID-only callbacks have no process generation;
                            // inventing durable dedup here would suppress PID reuse.
                            uuid::Uuid::new_v4().to_string()
                        }
                    }),
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
    ) -> Result<HostDeliveryReceipt, ToolCallError> {
        let (queue_item_id, _) = message.inbox_identity(session_id);
        let HostMessage { mut origin, text } = message;
        if origin.source == HostMessageSource::Agent
            && let Some(sender) = self
                .session(SessionId::from_string(origin.sender.clone()))
                .await
            && let Some(summary) = sender.summary().await
        {
            origin.sender_name = summary.agent_nickname.or(summary.native.title);
        }
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

        let turn_limit_reached = reservation.max_turns.is_some_and(|max_turns| {
            max_turns == 0
                || reservation
                    .active_turn
                    .as_ref()
                    .is_some_and(|turn| turn.native.sequence >= max_turns)
                || reservation
                    .latest_turn
                    .as_ref()
                    .is_some_and(|turn| turn.native.sequence >= max_turns)
        });

        let interrupted_owner = self.background_input_suspended(session_id)
            || self
                .active_turns
                .cancel_token(session_id)
                .await
                .is_some_and(|token| token.is_cancelled())
            || (reservation.active_turn.is_none()
                && reservation.latest_turn.as_ref().is_some_and(|turn| {
                    turn.native.status == devo_protocol::native::turn::TurnStatus::Interrupted
                }));
        let mut item = devo_protocol::PendingInputItem::new(
            devo_protocol::PendingInputKind::UserText { text: input_text },
            queued_metadata,
            Utc::now(),
        );
        item.id = queue_item_id;
        item.set_delivery(origin.delivery);
        if turn_limit_reached || interrupted_owner {
            // Preserve a completed producer's result without starting another
            // model turn or keeping its background ticket pending forever.
            crate::runtime::handlers::queue_steer::hold_for_review(&mut item);
            item.metadata
                .get_or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .expect("inbox metadata is an object")
                .insert(
                    if turn_limit_reached {
                        "maxTurnsReached"
                    } else {
                        "ownerInterrupted"
                    }
                    .into(),
                    serde_json::json!(true),
                );
        }
        let admitted = self
            .persist_inbox_item(
                session_id,
                reservation.ephemeral,
                &reservation.pending_turn_queue,
                &item,
            )
            .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
        if admitted
            && !turn_limit_reached
            && !interrupted_owner
            && let Some((parent_session_id, _)) = self.child_parent_and_path(session_id).await
        {
            self.register_background_work(
                parent_session_id,
                format!("agent:{session_id}:queued:{}", item.id),
            );
        }
        drop(_state_change_guard);
        if admitted {
            self.broadcast_queue_updated(
                session_id,
                devo_protocol::native::queue::QueueChange::Added,
                item.id,
                None,
            )
            .await;
        }
        // Queue acceptance is not a claim that the model has consumed the input.
        // Both idle and busy delivery retain the same persist-first ordering.
        self.drain_queue_if_idle(session_id).await;
        Ok(HostDeliveryReceipt {
            queue_item_id: item.id,
        })
    }
}

#[cfg(test)]
mod tests;
