//! `context/usage/read` RPC handler and mid-turn occupancy broadcasts.

use devo_core::RawContextBreakdown;
use devo_protocol::SuccessResponse;
use devo_protocol::native::ids::SessionId;
use devo_protocol::native::item::ContextOccupancy;
use devo_protocol::native::rpc_admin::ContextUsageReadParams;
use devo_protocol::native::rpc_admin::ContextUsageReadResult;

use super::ServerRuntime;
use crate::ProtocolErrorCode;

impl ServerRuntime {
    /// Publish a live context occupancy snapshot during an in-flight turn.
    ///
    /// Call only with a provider-anchored total (from `Usage` / `UsageDelta`).
    /// The window always resolves to the same effective/compaction limit used
    /// at finalize (and by TUI), never the raw hard model context window.
    pub(super) async fn publish_live_context_occupancy(
        &self,
        session_id: SessionId,
        context_window_hint: Option<u64>,
        raw: RawContextBreakdown,
        anchor_total: u64,
    ) {
        // A zero anchor is a leg-start placeholder (several providers send
        // message_start with all-zero usage), not a measurement. Scaled to
        // the `.max(1)` floor below it would broadcast a 1-token occupancy
        // and the client context bar collapses to 0% for the first moments
        // of every leg. Keep the last known occupancy instead.
        if anchor_total == 0 {
            return;
        }
        let window = self
            .live_occupancy_window(session_id, context_window_hint)
            .await
            .max(1);
        let occupancy =
            super::context_occupancy::occupancy_from_raw(window, raw, anchor_total.max(1));
        let mut native_session_id = None;
        if let Some(stream) = self.active_stream_state(session_id).await {
            let mut stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_mut() {
                inline.summary.last_query_total_tokens = occupancy.total_tokens as usize;
                inline.summary.last_context_occupancy = Some(occupancy.clone());
                inline.hook_context.summary = inline.summary.clone();
                native_session_id = Some(inline.summary.native.id);
            }
        }
        let native_session_id = if let Some(native_session_id) = native_session_id {
            native_session_id
        } else if let Some(summary) = self.session_summary_snapshot(session_id).await {
            summary.native.id
        } else {
            // boundary: legacy session id when summary unavailable
            session_id
        };
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::ContextUsageUpdated {
                session_id: native_session_id,
                occupancy,
            },
        )
        .await;
    }

    async fn live_occupancy_window(
        &self,
        session_id: SessionId,
        context_window_hint: Option<u64>,
    ) -> u64 {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                let model = inline
                    .summary
                    .model_name()
                    .and_then(|slug| self.deps.model_catalog.get(slug))
                    .or_else(|| {
                        inline
                            .summary
                            .model_binding_id()
                            .and_then(|binding| self.deps.model_catalog.get(binding))
                    });
                return super::context_occupancy::occupancy_window_tokens(model);
            }
        }

        if let Some(summary) = self.session_summary_snapshot(session_id).await {
            let model = summary
                .model_name()
                .and_then(|slug| self.deps.model_catalog.get(slug))
                .or_else(|| {
                    summary
                        .model_binding_id()
                        .and_then(|binding| self.deps.model_catalog.get(binding))
                });
            if let Some(occupancy) = summary.last_context_occupancy.as_ref()
                && occupancy.context_window_tokens > 0
                && model.is_none()
            {
                return occupancy.context_window_tokens;
            }
            return super::context_occupancy::occupancy_window_tokens(model);
        }

        context_window_hint.unwrap_or(1).max(1)
    }

    pub(super) async fn handle_context_usage_read(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<ContextUsageReadParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid context/usage/read params: {error}"),
                );
            }
        };

        let session_id = params.session_id;

        let Some((occupancy, _usage_known)) = self.context_usage_snapshot(session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                format!("session not found: {session_id}"),
            );
        };

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: ContextUsageReadResult { occupancy },
        })
        .expect("serialize context/usage/read response")
    }

    /// Resolve the current occupancy snapshot for a session.
    ///
    /// Returns `None` when the session summary is unavailable. The bool is
    /// `usage_known`: `true` when the snapshot carries a measured occupancy
    /// (set at turn finalize), `false` before the first usage or right after
    /// a compaction clears it — in that case callers still receive an empty
    /// occupancy sized to the model's compaction limit so they can report
    /// the context window.
    pub(crate) async fn context_usage_snapshot(
        &self,
        session_id: SessionId,
    ) -> Option<(ContextOccupancy, bool)> {
        let summary = self.session_summary_snapshot(session_id).await?;
        if let Some(occupancy) = summary.last_context_occupancy {
            return Some((occupancy, true));
        }
        let model = summary
            .model_name()
            .and_then(|slug| self.deps.model_catalog.get(slug))
            .or_else(|| {
                summary
                    .model_binding_id()
                    .and_then(|binding| self.deps.model_catalog.get(binding))
            });
        let window = model
            .map(crate::runtime::context_occupancy::resolved_compaction_limit)
            .unwrap_or(0);
        Some((ContextOccupancy::empty(window), false))
    }
}
