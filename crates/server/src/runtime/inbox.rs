//! Single inbox admission serialization point. No await separates durable and live order.
use super::*;

impl ServerRuntime {
    pub(super) fn persist_inbox_item(
        &self,
        session_id: SessionId,
        ephemeral: bool,
        inbox: &Arc<std::sync::Mutex<std::collections::VecDeque<devo_protocol::PendingInputItem>>>,
        item: &devo_protocol::PendingInputItem,
    ) -> anyhow::Result<bool> {
        let mut queue = inbox.lock().expect("pending queue mutex poisoned");
        if queue.iter().any(|existing| {
            existing.id == item.id
                || item
                    .host_message_origin()
                    .zip(existing.host_message_origin())
                    .is_some_and(|(a, b)| a.sender == b.sender && a.event_id == b.event_id)
        }) {
            return Ok(false);
        }
        if ephemeral {
            if !self
                .ephemeral_inbox_receipts
                .lock()
                .expect("ephemeral inbox receipts poisoned")
                .insert((session_id, item.admission_key()))
            {
                return Ok(false);
            }
        } else if !self.deps.db.admit_inbox(&session_id, item)? {
            return Ok(false);
        }
        queue.push_back(item.clone());
        Ok(true)
    }
}

impl ServerRuntime {
    /// Transfer only the eligible head prefix. Claim receipts make uncertain
    /// crashes visible as held input, rather than silently replaying side effects.
    pub(super) async fn claim_inbox_steers(
        self: &Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> anyhow::Result<()> {
        use devo_protocol::native::host_message::HostMessageDelivery;
        let Some(handle) = self.session(session_id).await else {
            return Ok(());
        };
        let guard = handle.lock_state_change().await;
        let Some(reservation) = self.session_turn_reservation_snapshot(session_id).await else {
            return Ok(());
        };
        if reservation
            .active_turn
            .as_ref()
            .is_none_or(|turn| turn.turn_id() != turn_id)
            || handle.queue_steer_closed(turn_id)
        {
            return Ok(());
        }
        let claimed = {
            let mut queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending queue mutex poisoned");
            let mut claimed = Vec::new();
            while let Some(item) = queue.front() {
                if item.delivery() != HostMessageDelivery::Steer
                    || crate::runtime::handlers::queue_steer::requires_queue_review(item)
                {
                    break;
                }
                if !reservation.ephemeral
                    && !self
                        .deps
                        .db
                        .claim_queue_steer(&session_id, item, &turn_id)?
                {
                    break;
                }
                let item = queue.pop_front().expect("head checked");
                reservation
                    .steer_input_queue
                    .lock()
                    .expect("steer queue mutex poisoned")
                    .push_back(item.clone());
                claimed.push(item);
            }
            claimed
        };
        drop(guard);
        for item in claimed {
            let content = match &item.kind {
                devo_protocol::PendingInputKind::UserText { text } => {
                    vec![devo_protocol::native::item::UserInput::Text { text: text.clone() }]
                }
                devo_protocol::PendingInputKind::UserInput { input, .. } => input.clone(),
                _ => Vec::new(),
            };
            let native = devo_protocol::native::item::Item::UserMessage {
                origin: item.host_message_origin(),
                client_user_message_id: Some(
                    item.metadata
                        .as_ref()
                        .and_then(|m| m.get("clientUserMessageId"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("inbox:{}", item.id)),
                ),
                content,
                entry: devo_protocol::native::item::UserMessageEntry::Steer,
            };
            let (id, seq) = self
                .start_native_item(session_id, turn_id, native.clone())
                .await;
            if let Err(error) = self
                .complete_native_item_checked(session_id, turn_id, id, seq, native)
                .await
            {
                // Nothing from this prefix has reached the model yet. Preserve
                // every claimed item, including any partially written history,
                // as a visible barrier rather than settling it as consumed.
                {
                    let mut inflight = reservation
                        .steer_input_queue
                        .lock()
                        .expect("steer queue mutex poisoned");
                    let mut pending = reservation
                        .pending_turn_queue
                        .lock()
                        .expect("pending queue mutex poisoned");
                    for mut input in inflight.drain(..).rev() {
                        crate::runtime::handlers::queue_steer::hold_for_review(&mut input);
                        pending.push_front(input);
                    }
                }
                self.broadcast_queue_updated(
                    session_id,
                    devo_protocol::native::queue::QueueChange::Updated,
                    item.id,
                    None,
                )
                .await;
                return Err(error);
            }
            self.broadcast_queue_updated(
                session_id,
                devo_protocol::native::queue::QueueChange::Drained,
                item.id,
                Some(turn_id),
            )
            .await;
        }
        Ok(())
    }
}

impl ServerRuntime {
    /// Restore a claimed but unstarted input as an explicit FIFO barrier.
    /// The durable claim is retained: recovery must never silently execute it.
    pub(super) async fn restore_claimed_inbox_head(
        self: &Arc<Self>,
        session_id: SessionId,
        item: devo_protocol::PendingInputItem,
        turn_id: TurnId,
    ) {
        let Some(handle) = self.session(session_id).await else {
            return;
        };
        let _gate = handle.lock_state_change().await;
        self.restore_claimed_inbox_head_locked(session_id, item, turn_id)
            .await;
    }

    /// Caller already holds the session state-change gate.
    pub(super) async fn restore_claimed_inbox_head_locked(
        self: &Arc<Self>,
        session_id: SessionId,
        mut item: devo_protocol::PendingInputItem,
        turn_id: TurnId,
    ) {
        let Some(handle) = self.session(session_id).await else {
            return;
        };
        crate::runtime::handlers::queue_steer::hold_for_review(&mut item);
        if let Some(reservation) = self.session_turn_reservation_snapshot(session_id).await {
            let mut queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending queue mutex poisoned");
            if !queue.iter().any(|entry| entry.id == item.id) {
                queue.push_front(item.clone());
            }
        }
        let _ = handle.clear_active_turn_if_matches(turn_id).await;
        self.broadcast_queue_updated(
            session_id,
            devo_protocol::native::queue::QueueChange::Added,
            item.id,
            None,
        )
        .await;
    }
}

impl ServerRuntime {
    /// Caller holds the session state-change gate. A pre-admitted input cannot
    /// bypass the agent's execution budget when earlier turns finish.
    pub(super) async fn hold_ineligible_inbox_head(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> bool {
        let Some(reservation) = self.session_turn_reservation_snapshot(session_id).await else {
            return true;
        };
        let reached = reservation.max_turns.is_some_and(|limit| {
            limit == 0
                || reservation
                    .active_turn
                    .as_ref()
                    .into_iter()
                    .chain(reservation.latest_turn.as_ref())
                    .any(|turn| turn.native.sequence >= limit)
        });
        let suspended = self.background_input_suspended(session_id);
        if !reached && !suspended {
            return false;
        }
        let ids = {
            let mut queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending queue mutex poisoned");
            if queue.is_empty() {
                return false;
            }
            if !reached
                && queue
                    .front()
                    .is_none_or(|item| item.host_message_origin().is_none())
            {
                return false;
            }
            let count = if reached { queue.len() } else { 1 };
            for item in queue.iter_mut().take(count) {
                crate::runtime::handlers::queue_steer::hold_for_review(item);
                item.metadata
                    .as_mut()
                    .and_then(serde_json::Value::as_object_mut)
                    .expect("hold metadata object")
                    .insert(
                        if reached {
                            "maxTurnsReached"
                        } else {
                            "ownerInterrupted"
                        }
                        .into(),
                        true.into(),
                    );
                if !reservation.ephemeral
                    && let Err(error) = self.deps.db.update_pending_content(
                        &session_id,
                        crate::db::QueueType::Turn,
                        item,
                    )
                {
                    tracing::error!(%session_id, %error, "failed to persist turn-limit barrier");
                }
            }
            queue
                .iter()
                .take(count)
                .map(|item| item.id)
                .collect::<Vec<_>>()
        };
        if let Some(parent) = reservation.summary.parent_session_id() {
            for id in &ids {
                self.complete_background_work(
                    parent,
                    format!("agent:{session_id}:queued:{id}"),
                    String::new(),
                );
            }
        }
        for id in ids {
            self.broadcast_queue_updated(
                session_id,
                devo_protocol::native::queue::QueueChange::Updated,
                id,
                None,
            )
            .await;
        }
        true
    }
}
