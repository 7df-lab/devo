//! Foreground completion acknowledgements share the inbox admission/claim gate.
use super::*;
use crate::db::InboxConsumption;
use serde_json::{Value, json};

impl ServerRuntime {
    pub(super) async fn consume_bash_completion(
        self: &Arc<Self>,
        session_id: SessionId,
        handle_id: &str,
    ) -> anyhow::Result<InboxConsumption> {
        let status = devo_core::tools::host_bash::control(
            session_id.as_str(),
            "bash.poll",
            &json!({"handle_id": handle_id}),
        );
        anyhow::ensure!(status["status"] == "ok", "unknown bash handle");
        anyhow::ensure!(
            status["result"]["running"] == false,
            "bash is still running"
        );
        anyhow::ensure!(
            matches!(
                status["result"]["mode"].as_str(),
                Some("interactive" | "pty")
            ),
            "managed consumption requires an interactive bash handle"
        );
        self.consume_background_completion(session_id, format!("bash:{handle_id}"))
            .await
    }

    async fn consume_background_completion(
        self: &Arc<Self>,
        session_id: SessionId,
        work_id: String,
    ) -> anyhow::Result<InboxConsumption> {
        let handle = self
            .session(session_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        let guard = handle.lock_state_change().await;
        let reservation = self
            .session_turn_reservation_snapshot(session_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("session reservation unavailable"))?;
        let (id, key) = host_messages::HostMessage::managed_completion(
            session_id,
            work_id.clone(),
            String::new(),
        )
        .inbox_identity(session_id);
        let (disposition, removed) = {
            let mut queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending queue poisoned");
            let position = queue.iter().position(|item| item.id == id);
            let disposition = if reservation.ephemeral {
                let identity = (session_id, key);
                let mut receipts = self
                    .ephemeral_inbox_receipts
                    .lock()
                    .expect("inbox receipts poisoned");
                let mut consumed = self
                    .ephemeral_inbox_consumed
                    .lock()
                    .expect("inbox consumption poisoned");
                if consumed.contains(&identity) {
                    InboxConsumption::AlreadyConsumed
                } else if receipts.contains(&identity)
                    && (position.is_none()
                        || self
                            .ephemeral_inbox_claimed
                            .lock()
                            .expect("ephemeral inbox claims poisoned")
                            .contains(&(session_id, id)))
                {
                    InboxConsumption::AlreadyDelivered
                } else {
                    receipts.insert(identity.clone());
                    consumed.insert(identity);
                    InboxConsumption::Consumed
                }
            } else {
                self.deps
                    .db
                    .consume_inbox_completion(&session_id, &key, &id)?
            };
            let removed = if disposition != InboxConsumption::AlreadyDelivered {
                position.and_then(|index| queue.remove(index)).is_some()
            } else {
                false
            };
            (disposition, removed)
        };
        self.complete_background_work(session_id, work_id, String::new());
        if removed && let Some(parent) = reservation.summary.parent_session_id() {
            self.complete_background_work(
                parent,
                format!("agent:{session_id}:queued:{id}"),
                String::new(),
            );
        }
        drop(guard);
        if removed {
            self.broadcast_queue_updated(
                session_id,
                devo_protocol::native::queue::QueueChange::Removed,
                id,
                None,
            )
            .await;
            let runtime = Arc::clone(self);
            tokio::spawn(async move {
                runtime.drain_queue_if_idle(session_id).await;
            });
        }
        Ok(disposition)
    }
}

/// Legacy callbacks lack a host handle; managed acknowledgements must verify
/// ownership and completion before withdrawing their durable owner notification.
pub(super) async fn consume(
    runtime: Option<Arc<ServerRuntime>>,
    session_id: SessionId,
    params: &Value,
) -> Value {
    let Some(identity) = params
        .get("handle_id")
        .filter(|v| v.is_string())
        .or_else(|| params.get("pid").filter(|v| v.is_i64()))
    else {
        return json!({"status":"error", "error":"bash.consumed requires a handle_id or pid"});
    };
    let disposition = if params["managed"] == true {
        let (Some(runtime), Some(handle_id)) = (runtime, identity.as_str()) else {
            return json!({"status":"error", "error":"managed consumption requires runtime and handle_id"});
        };
        match runtime.consume_bash_completion(session_id, handle_id).await {
            Ok(InboxConsumption::Consumed) => "consumed",
            Ok(InboxConsumption::AlreadyConsumed) => "already_consumed",
            Ok(InboxConsumption::AlreadyDelivered) => "already_delivered",
            Err(error) => return json!({"status":"error", "error":error.to_string()}),
        }
    } else {
        "consumed"
    };
    super::kernel_host::consume_bash_notice(session_id.as_str(), identity);
    if params["managed"] == true {
        json!({"status":"ok", "result":{"accepted":true, "disposition":disposition}})
    } else {
        json!({"status":"ok", "result":{"accepted":true}})
    }
}

#[cfg(test)]
mod tests;
