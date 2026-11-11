//! Identity-based queue transfer. Admission is single-use, not exactly-once
//! execution: a crash can leave uncertain input held for explicit review.
use std::sync::Arc;

use devo_core::PendingInputItem;
use devo_protocol::native::queue::QueueChange;
use devo_protocol::native::rpc_turn::{
    SessionQueueRetryParams, SessionQueueRetryResult, SessionQueueSteerParams,
};
use serde_json::{Value, json};

use super::super::*;

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
        let _ = params;
        self.error_response(
            request_id,
            ProtocolErrorCode::InvalidParams,
            "queue delivery mode is fixed at admission; strict FIFO does not permit promotion",
        )
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
            if queue[index]
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("maxTurnsReached"))
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "agent turn limit reached; this input cannot be retried",
                );
            }
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
        // Explicit retry is the user's resume decision. Keep it serialized
        // with interruption so a later interrupt can establish a new barrier.
        self.resume_background_input(params.session_id);
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
