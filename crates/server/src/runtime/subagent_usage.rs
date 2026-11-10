//! The durable per-call ledger is the only spend accumulator. Streaming events
//! update local context only; no parent-turn baseline can overwrite session spend.
use super::*;
use devo_protocol::native::usage::{TurnUsage, UsageAttribution};

impl ServerRuntime {
    pub(super) fn start_usage_updates(self: &Arc<Self>) {
        let Ok(executor) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        self.usage_ledger.set_updates(sender);
        let weak = Arc::downgrade(self);
        executor.spawn(async move {
            while let Some(session_id) = receiver.recv().await {
                let Some(runtime) = weak.upgrade() else {
                    break;
                };
                // One consumer publishes snapshots in order, rereading the ledger
                // rather than retaining stale snapshots across another turn.
                for affected in runtime.usage_ledger.affected_sessions(session_id) {
                    runtime.publish_recorded_usage(affected).await;
                }
            }
        });
    }

    pub(super) async fn register_subagent_usage_owner(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
        parent_turn_id: Option<TurnId>,
    ) {
        self.usage_ledger.register_owner(
            child_session_id,
            UsageAttribution {
                session_id: parent_session_id,
                turn_id: parent_turn_id,
            },
        );
    }

    pub(super) async fn active_turn_id_for_session(&self, session_id: SessionId) -> Option<TurnId> {
        if let Some(turn_id) = self.runtime_active_turn_id(session_id).await {
            return Some(turn_id);
        }
        self.session(session_id)
            .await?
            .active_turn_id()
            .await
            .flatten()
    }

    async fn publish_recorded_usage(&self, session_id: SessionId) {
        // This gate also orders per-request context notifications, so an old
        // notification cannot follow a newer cumulative usage snapshot.
        let _publication = self.usage_publication.lock().await;
        let Some(usage) = self.usage_ledger.session_usage(session_id) else {
            return;
        };
        if let Some(stream) = self.active_stream_state(session_id).await {
            let mut stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_mut() {
                inline.summary.usage = usage.clone();
                inline.hook_context.summary.usage = usage.clone();
                inline.active_turn_usage =
                    Some(self.usage_ledger.turn_usage(session_id, inline.turn_id));
            }
        }
        // Reliable, short mailbox command; it reads the ledger at application
        // time, including if MergeTurn overtakes this send.
        if let Some(handle) = self.session(session_id).await {
            handle.refresh_usage().await;
        }
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::SessionUsageUpdated {
                session_id,
                usage: Box::new(usage),
            },
        )
        .await;
    }

    pub(super) async fn publish_query_usage(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        usage: TurnUsage,
        context_window: Option<u64>,
        delivery_policy: super::outbound::OutboundDeliveryPolicy,
    ) {
        // Missing usage is a boundary, not a zero context measurement. The
        // terminal record has already counted the attempt, even without tokens.
        if usage.query.metered_call_count == 0 {
            return;
        }
        let _publication = self.usage_publication.lock().await;
        if let Some(stream) = self.active_stream_state(session_id).await {
            let mut stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_mut()
                && inline.turn_id == turn_id
            {
                inline.summary.last_query_usage = Some(usage.clone());
                inline.summary.last_query_total_tokens = usage.display_total_tokens() as usize;
                if let Some(total) = self.usage_ledger.session_usage(session_id) {
                    inline.summary.usage = total;
                }
                inline.hook_context.summary = inline.summary.clone();
                inline.active_turn_usage = Some(self.usage_ledger.turn_usage(session_id, turn_id));
            }
        }
        self.broadcast_notification_with_policy(
            devo_protocol::native::event::ServerNotification::TurnUsageUpdated {
                session_id,
                turn_id,
                last_query_input_tokens: usage.query.input_tokens,
                usage,
                session_totals: self
                    .usage_ledger
                    .session_usage(session_id)
                    .map(|usage| usage.total),
                context_window,
            },
            delivery_policy,
        )
        .await;
    }

    /// Called only by the actor, or on its checked-out turn working copy.
    pub(crate) fn refresh_actor_usage(&self, state: &mut session_actor::SessionActorState) {
        let session_id = state.summary.session_id();
        if let Some(usage) = self.usage_ledger.session_usage(session_id) {
            state.core.total_input_tokens = usage.total.input_tokens as usize;
            state.core.total_output_tokens = usage.total.output_tokens as usize;
            state.core.total_tokens = usage.total.total_tokens as usize;
            state.core.total_cache_creation_tokens =
                usage.total.cache_creation_input_tokens as usize;
            state.core.total_cache_read_tokens = usage.total.cache_read_input_tokens as usize;
            state.summary.usage = usage;
        }
        if let Some(turn) = state.active_turn.as_mut() {
            turn.native.usage = Some(self.usage_ledger.turn_usage(session_id, turn.turn_id()));
        }
    }
}
