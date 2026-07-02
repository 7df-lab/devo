mod event_stream;
mod finalize;
mod followup;
mod item_stream;
mod query;
mod shell;
mod tool_display;
mod tool_results;
mod trace;
mod types;

pub(crate) use event_stream::{QUERY_EVENT_CHANNEL_CAPACITY, spawn_turn_event_stream};
pub(crate) use finalize::FinalizeTurnParams;
pub(crate) use query::TurnModelQueryParams;
pub(crate) use types::ExecuteTurnRequest;

use std::sync::Arc;

use super::*;

impl ServerRuntime {
    /// Execute one turn end-to-end via the session actor.
    pub(in crate::runtime) async fn execute_turn(self: Arc<Self>, request: ExecuteTurnRequest) {
        let Some(handle) = self.session(request.session_id).await else {
            return;
        };
        handle.execute_turn(Arc::clone(&self), request).await;
    }

    pub(super) async fn prepare_turn_execution_for_actor(
        self: &Arc<Self>,
        state: &mut SessionActorState,
        turn: &crate::TurnMetadata,
        display_input: &str,
        emits_user_message: bool,
    ) {
        self.capture_turn_workspace_baseline(
            state.session_id(),
            turn.turn_id,
            state.summary.cwd.clone(),
        )
        .await;
        state.turn_approval_cache = crate::execution::ApprovalGrantCache::default();
        if emits_user_message {
            self.emit_turn_item(
                state.session_id(),
                turn.turn_id,
                crate::ItemKind::UserMessage,
                devo_core::TurnItem::UserMessage(devo_core::TextItem {
                    text: display_input.to_string(),
                }),
                serde_json::json!({ "title": "You", "text": display_input }),
            )
            .await;
        }
    }

    pub(in crate::runtime) fn tool_registry_for_actor_state(
        &self,
        state: &SessionActorState,
    ) -> Arc<devo_core::tools::ToolRegistry> {
        state
            .tool_registry
            .clone()
            .unwrap_or_else(|| Arc::clone(&state.runtime_context.registry))
    }
}

#[cfg(test)]
mod tests;
