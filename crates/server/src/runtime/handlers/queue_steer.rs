//! Identity-based queue transfer. Admission is single-use, not exactly-once
//! execution: a crash can leave uncertain input held for explicit review.
use std::sync::Arc;

use devo_core::{PendingInputItem, PendingInputKind};
use devo_protocol::native::queue::QueueChange;
use devo_protocol::native::rpc_turn::{
    SessionQueueRetryParams, SessionQueueRetryResult, SessionQueueSteerParams,
    SessionQueueSteerResult,
};
use serde_json::{Value, json};

use super::super::*;
use super::queue::native_queue_entries;

pub(crate) fn requires_queue_review(item: &PendingInputItem) -> bool {
    item.metadata
        .as_ref()
        .and_then(|m| m.get("queueSteerRecoveryRequired"))
        .and_then(Value::as_bool)
        == Some(true)
}

pub(crate) fn hold_for_review(item: &mut PendingInputItem) {
    let mut metadata = match item.metadata.take() {
        Some(Value::Object(object)) => object,
        None => serde_json::Map::new(),
        Some(value) => serde_json::Map::from_iter([("queueSteerOriginalMetadata".into(), value)]),
    };
    metadata.insert("queueSteerRecoveryRequired".into(), json!(true));
    item.metadata = Some(Value::Object(metadata));
}

impl ServerRuntime {
    pub(crate) async fn handle_session_queue_steer(
        self: &Arc<Self>,
        request_id: Value,
        params: Value,
    ) -> Value {
        let params: SessionQueueSteerParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/queue/steer params: {error}"),
                );
            }
        };
        let Some(handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        // The same gate protects queue draining, update/remove, and the close
        // of steering admission. Never wait on a model or outbound connection.
        let guard = handle.lock_state_change().await;
        let Some(reservation) = self
            .session_turn_reservation_snapshot(params.session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let receipt = if reservation.ephemeral {
            Ok(handle.ephemeral_queue_steer_receipt(params.queue_item_id))
        } else {
            self.deps
                .db
                .queue_steer_receipt(&params.session_id, &params.queue_item_id)
        };
        match receipt {
            Ok(Some(turn_id)) => {
                return json!({"id":request_id,"result":SessionQueueSteerResult::Accepted {
                    queue_item_id: params.queue_item_id, turn_id,
                }});
            }
            Ok(None) => {}
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("could not read queue transfer receipt: {error}"),
                );
            }
        }
        let active_turn = reservation.active_turn.as_ref().filter(|turn| {
            turn.turn_id() == params.expected_turn_id
                && !handle.queue_steer_closed(turn.turn_id())
                && turn.native.kind == devo_protocol::native::turn::TurnKind::Regular
        });
        let approval_pending = self
            .session_interactive
            .has_pending_approval_for_host(params.session_id)
            .await;
        let transferred = {
            let mut queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending queue mutex poisoned");
            let Some(index) = queue
                .iter()
                .position(|item| item.id == params.queue_item_id)
            else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::QueueItemNotFound,
                    "queue item is no longer queued",
                );
            };
            // Host follow-ups retain their original delivery and authority boundary.
            // Never turn them into a human steering item through this RPC.
            if queue[index].host_message_origin().is_some() {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "host notifications cannot be steered as human input",
                );
            }
            if active_turn.is_none() || approval_pending || requires_queue_review(&queue[index]) {
                let entry = native_queue_entries(&queue).remove(index);
                return json!({"id":request_id,"result":SessionQueueSteerResult::KeptQueued { entry }});
            }
            let turn_id = active_turn.expect("active turn checked").turn_id();
            let item = queue[index].clone();
            if !reservation.ephemeral {
                match self
                    .deps
                    .db
                    .claim_queue_steer(&params.session_id, &item, &turn_id)
                {
                    Ok(true) => {}
                    Ok(false) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::QueueItemNotFound,
                            "queue item could not be claimed",
                        );
                    }
                    Err(error) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InternalError,
                            format!("could not persist queue transfer: {error}"),
                        );
                    }
                }
            } else {
                handle.remember_ephemeral_queue_steer(item.id, turn_id);
            }
            // No await separates durable claim, original removal, and inbox
            // insertion. A crash in this span is recovered as held input.
            queue.remove(index);
            reservation
                .steer_input_queue
                .lock()
                .expect("steer queue mutex poisoned")
                .push_back(item.clone());
            (item, turn_id)
        };
        drop(guard);
        let (item, turn_id) = transferred;
        self.broadcast_queue_updated(params.session_id, QueueChange::Promoted, item.id, None)
            .await;
        // Record only acceptance, with a stable user-message identity. This
        // bubble is never used as evidence that the model consumed the input.
        let (text, image_paths) = match &item.kind {
            PendingInputKind::UserInput {
                input,
                display_text,
                ..
            } => (
                display_text.clone(),
                input
                    .iter()
                    .filter_map(|part| {
                        if let devo_protocol::native::item::UserInput::LocalImage { path, .. } =
                            part
                        {
                            Some(path.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>(),
            ),
            PendingInputKind::UserText { text } => (text.clone(), Vec::new()),
            _ => (String::new(), Vec::new()),
        };
        let client_id = item
            .metadata
            .as_ref()
            .and_then(|m| m.get("clientUserMessageId"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("queue-steer:{}", item.id));
        let native_item = crate::runtime::items::native_user_message_item_with_client_message_id(
            text,
            &image_paths,
            devo_protocol::native::item::UserMessageEntry::Steer,
            client_id,
        );
        let (item_id, item_seq) = self
            .start_native_item(params.session_id, turn_id, native_item.clone())
            .await;
        self.complete_native_item(params.session_id, turn_id, item_id, item_seq, native_item)
            .await;
        json!({"id":request_id,"result":SessionQueueSteerResult::Accepted { queue_item_id:item.id, turn_id }})
    }

    pub(crate) async fn handle_session_queue_retry(
        self: &Arc<Self>,
        request_id: Value,
        params: Value,
    ) -> Value {
        let params: SessionQueueRetryParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/queue/retry params: {error}"),
                );
            }
        };
        let Some(handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let guard = handle.lock_state_change().await;
        let Some(reservation) = self
            .session_turn_reservation_snapshot(params.session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        {
            let mut queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending queue mutex poisoned");
            let Some(index) = queue
                .iter()
                .position(|item| item.id == params.queue_item_id)
            else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::QueueItemNotFound,
                    "queue item is no longer queued",
                );
            };
            if !requires_queue_review(&queue[index]) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "only held recovery input can be explicitly retried",
                );
            }
            let mut replacement = queue[index].clone();
            replacement.id = devo_core::QueueItemId::new();
            let metadata = replacement.metadata.get_or_insert_with(|| json!({}));
            metadata
                .as_object_mut()
                .expect("held metadata is an object")
                .remove("queueSteerRecoveryRequired");
            metadata["clientUserMessageId"] = json!(format!("queue-retry:{}", replacement.id));
            if !reservation.ephemeral {
                // A failed receipt read can conservatively hold an ordinary
                // Turn only in memory. Persist that review barrier once storage
                // recovers before honoring this explicit user decision.
                if let Err(error) = self.deps.db.update_pending_content(
                    &params.session_id,
                    QueueType::Turn,
                    &queue[index],
                ) {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("could not reconcile held queue input: {error}"),
                    );
                }
                let ordered: Vec<_> = queue
                    .iter()
                    .enumerate()
                    .map(|(position, item)| {
                        if position == index {
                            replacement.id
                        } else {
                            item.id
                        }
                    })
                    .collect();
                match self.deps.db.retry_held_queue_steer(
                    &params.session_id,
                    &params.queue_item_id,
                    &replacement,
                    &ordered,
                ) {
                    Ok(true) => {}
                    Ok(false) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::QueueItemNotFound,
                            "held queue item could not be released",
                        );
                    }
                    Err(error) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InternalError,
                            format!("could not persist explicit queue retry: {error}"),
                        );
                    }
                }
            }
            queue[index] = replacement;
        }
        drop(guard);
        self.broadcast_queue_updated(
            params.session_id,
            QueueChange::Updated,
            params.queue_item_id,
            None,
        )
        .await;
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            runtime.drain_queue_if_idle(params.session_id).await;
        });
        json!({"id":request_id,"result":SessionQueueRetryResult {}})
    }
}
