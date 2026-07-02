use std::sync::Arc;

use chrono::Utc;
use devo_core::{SessionId, TextItem, TurnItem, TurnStatus};

use super::super::ServerRuntime;
use super::event_stream::turn_failure_reason_from_error;
use super::types::{TurnEventStreamSummary, TurnQueryOutcome};
use crate::db::{QueueType, SessionStats};
use crate::persistence::build_turn_record;
use crate::runtime::session_actor::SessionActorState;
use crate::{ItemKind, SessionRuntimeStatus, SessionStatusChangedPayload, TurnEventPayload};

pub(crate) struct FinalizeTurnParams<'a> {
    pub state: &'a mut SessionActorState,
    pub session_id: SessionId,
    pub turn: crate::TurnMetadata,
    pub query_outcome: TurnQueryOutcome,
    pub event_summary: Option<TurnEventStreamSummary>,
    pub usage_parent_session_id: Option<SessionId>,
}

impl ServerRuntime {
    pub(crate) async fn finalize_executed_turn(self: &Arc<Self>, params: FinalizeTurnParams<'_>) {
        let FinalizeTurnParams {
            state,
            session_id,
            turn,
            query_outcome,
            event_summary,
            usage_parent_session_id,
        } = params;
        let TurnQueryOutcome {
            result,
            mut session_total_input_tokens,
            mut session_total_output_tokens,
            mut session_total_tokens,
            mut session_total_cache_creation_tokens,
            mut session_total_cache_read_tokens,
            session_last_input_tokens,
            session_prompt_token_estimate,
        } = query_outcome;
        let mut latest_usage = event_summary
            .as_ref()
            .and_then(|summary| summary.latest_usage.clone());
        let terminal_stop_reason = event_summary.and_then(|summary| summary.stop_reason);
        if usage_parent_session_id.is_some()
            && let Some(usage) = latest_usage.clone()
        {
            self.publish_subagent_turn_usage(session_id, turn.turn_id, usage)
                .await;
        } else if usage_parent_session_id.is_none()
            && let Some(snapshot) = self.parent_usage_snapshot(session_id, turn.turn_id).await
        {
            latest_usage = Some(snapshot.turn_usage.to_turn_usage());
            session_total_input_tokens = snapshot.session_totals.input_tokens;
            session_total_output_tokens = snapshot.session_totals.output_tokens;
            session_total_tokens = snapshot.session_totals.total_tokens;
            session_total_cache_creation_tokens =
                snapshot.session_totals.cache_creation_input_tokens;
            session_total_cache_read_tokens = snapshot.session_totals.cache_read_input_tokens;
        }
        self.active_tasks.lock().await.remove(&session_id);
        self.active_turn_cancellations
            .lock()
            .await
            .remove(&session_id);
        self.active_turn_ids.lock().await.remove(&session_id);
        self.active_turn_connections
            .lock()
            .await
            .remove(&session_id);
        match &result {
            Ok(()) => {
                self.run_session_hook_for_actor_state(
                    state,
                    session_id,
                    devo_core::HookEvent::Stop,
                    serde_json::Map::from_iter([(
                        "stop_hook_active".to_string(),
                        serde_json::Value::Bool(false),
                    )]),
                )
                .await;
            }
            Err(error) => {
                self.run_session_hook_for_actor_state(
                    state,
                    session_id,
                    devo_core::HookEvent::StopFailure,
                    serde_json::Map::from_iter([
                        (
                            "error".to_string(),
                            serde_json::Value::String(error.to_string()),
                        ),
                        (
                            "error_details".to_string(),
                            serde_json::Value::String(error.to_string()),
                        ),
                    ]),
                )
                .await;
            }
        }

        let final_turn = self
            .persist_terminal_turn_state(
                state,
                session_id,
                &turn,
                &result,
                latest_usage.clone(),
                terminal_stop_reason,
                session_total_input_tokens,
                session_total_output_tokens,
                session_total_tokens,
                session_total_cache_creation_tokens,
                session_total_cache_read_tokens,
                session_last_input_tokens,
                session_prompt_token_estimate,
            )
            .await;
        self.clear_btw_input_queue(state, session_id).await;
        self.append_terminal_turn_record(state, session_id, &final_turn)
            .await;
        self.finalize_turn_workspace_changes(session_id, &final_turn)
            .await;
        self.emit_terminal_turn_events(state, session_id, &final_turn, &result)
            .await;
        self.record_terminal_turn_status(
            final_turn.turn_id,
            super::super::TerminalTurnSnapshot::from_turn(&final_turn),
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn persist_terminal_turn_state(
        self: &Arc<Self>,
        state: &mut SessionActorState,
        session_id: SessionId,
        turn: &crate::TurnMetadata,
        result: &Result<(), devo_core::AgentError>,
        latest_usage: Option<devo_core::TurnUsage>,
        terminal_stop_reason: Option<devo_core::StopReason>,
        session_total_input_tokens: usize,
        session_total_output_tokens: usize,
        session_total_tokens: usize,
        session_total_cache_creation_tokens: usize,
        session_total_cache_read_tokens: usize,
        session_last_input_tokens: usize,
        session_prompt_token_estimate: usize,
    ) -> crate::TurnMetadata {
        let mut final_turn = turn.clone();
        final_turn.completed_at = Some(Utc::now());
        final_turn.status = match result {
            Ok(()) => TurnStatus::Completed,
            Err(devo_core::AgentError::Aborted) => TurnStatus::Interrupted,
            Err(_) => TurnStatus::Failed,
        };
        final_turn.usage = latest_usage.clone();
        final_turn.stop_reason = terminal_stop_reason;
        final_turn.failure_reason = result
            .as_ref()
            .err()
            .and_then(turn_failure_reason_from_error);
        state.latest_turn = Some(final_turn.clone());
        state.active_turn = None;
        state.summary.status = SessionRuntimeStatus::Idle;
        state.summary.updated_at = Utc::now();
        state.summary.last_activity_at = state.summary.updated_at;
        state.summary.total_input_tokens = session_total_input_tokens;
        state.summary.total_output_tokens = session_total_output_tokens;
        state.summary.total_tokens = session_total_tokens;
        state.summary.total_cache_creation_tokens = session_total_cache_creation_tokens;
        state.summary.total_cache_read_tokens = session_total_cache_read_tokens;
        state.summary.prompt_token_estimate = session_prompt_token_estimate;
        if let Some(usage) = &final_turn.usage {
            state.summary.last_query_total_tokens = usage.display_total_tokens();
        }
        state.core.total_input_tokens = session_total_input_tokens;
        state.core.total_output_tokens = session_total_output_tokens;
        state.core.total_tokens = session_total_tokens;
        state.core.total_cache_creation_tokens = session_total_cache_creation_tokens;
        state.core.total_cache_read_tokens = session_total_cache_read_tokens;
        if !state.summary.ephemeral {
            let stats = SessionStats {
                total_input_tokens: session_total_input_tokens,
                total_output_tokens: session_total_output_tokens,
                total_tokens: session_total_tokens,
                total_cache_creation_tokens: session_total_cache_creation_tokens,
                total_cache_read_tokens: session_total_cache_read_tokens,
                last_input_tokens: final_turn
                    .usage
                    .as_ref()
                    .map(|usage| usage.input_tokens as usize)
                    .unwrap_or(session_last_input_tokens),
                turn_count: state.summary.updated_at.timestamp() as usize,
                prompt_token_estimate: session_prompt_token_estimate,
            };
            if let Err(err) = self.deps.db.update_stats(&session_id, &stats) {
                tracing::warn!(
                    session_id = %session_id,
                    error = %err,
                    "failed to persist token stats to database"
                );
            }
        }
        final_turn
    }

    async fn clear_btw_input_queue(
        self: &Arc<Self>,
        state: &SessionActorState,
        session_id: SessionId,
    ) {
        let is_ephemeral = state.summary.ephemeral;
        let btw_input_queue = Arc::clone(&state.btw_input_queue);
        btw_input_queue
            .lock()
            .expect("btw input queue mutex should not be poisoned")
            .clear();
        if !is_ephemeral && let Err(err) = self.deps.db.clear_pending(&session_id, QueueType::Btw) {
            tracing::warn!(
                session_id = %session_id,
                error = %err,
                "failed to clear btw input messages from database"
            );
        }
    }

    async fn append_terminal_turn_record(
        self: &Arc<Self>,
        state: &SessionActorState,
        session_id: SessionId,
        final_turn: &crate::TurnMetadata,
    ) {
        let record = state.record.clone();
        let session_context = state.core.session_context.clone();
        let turn_context = state.core.latest_turn_context.clone();
        if let Some(record) = record
            && let Err(error) = self.rollout_store.append_turn(
                &record,
                build_turn_record(final_turn, session_context, turn_context),
            )
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist terminal turn line");
        }
    }

    async fn emit_terminal_turn_events(
        self: &Arc<Self>,
        state: &SessionActorState,
        session_id: SessionId,
        final_turn: &crate::TurnMetadata,
        result: &Result<(), devo_core::AgentError>,
    ) {
        if let Err(error) = result {
            if matches!(error, devo_core::AgentError::Aborted) {
                tracing::info!(
                    session_id = %session_id,
                    turn_id = %final_turn.turn_id,
                    status = ?final_turn.status,
                    "turn execution interrupted"
                );
                self.broadcast_event(crate::ServerEvent::TurnInterrupted(TurnEventPayload {
                    session_id,
                    turn: final_turn.clone(),
                }))
                .await;
            } else {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %final_turn.turn_id,
                    status = ?final_turn.status,
                    error = %error,
                    "turn execution failed"
                );
                self.emit_turn_item(
                    session_id,
                    final_turn.turn_id,
                    ItemKind::AgentMessage,
                    TurnItem::AgentMessage(TextItem {
                        text: error.to_string(),
                    }),
                    serde_json::json!({ "title": "Error", "text": error.to_string() }),
                )
                .await;
                self.broadcast_event(crate::ServerEvent::TurnFailed(TurnEventPayload {
                    session_id,
                    turn: final_turn.clone(),
                }))
                .await;
            }
        } else {
            tracing::info!(
                session_id = %session_id,
                turn_id = %final_turn.turn_id,
                status = ?final_turn.status,
                total_input_tokens = final_turn.usage.as_ref().map(|usage| usage.input_tokens),
                total_output_tokens = final_turn.usage.as_ref().map(|usage| usage.output_tokens),
                "turn execution completed"
            );
        }
        self.handle_subagent_turn_completed_for_actor_state(state, session_id, final_turn)
            .await;
        self.broadcast_event(crate::ServerEvent::TurnCompleted(TurnEventPayload {
            session_id,
            turn: final_turn.clone(),
        }))
        .await;
        self.broadcast_event(crate::ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id,
                status: SessionRuntimeStatus::Idle,
            },
        ))
        .await;
    }
}
