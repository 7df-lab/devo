use std::sync::Arc;

use chrono::Utc;
use devo_core::{SessionId, TurnError};
use devo_protocol::native::usage::TurnUsage;

use super::super::ServerRuntime;
use super::failure::{turn_agent_error_from_error, turn_failure_reason_from_error};
use super::types::{TurnEventStreamSummary, TurnQueryOutcome};
use crate::db::SessionStats;
use crate::runtime::session_actor::SessionActorState;
use devo_protocol::native::session::SessionStatus;

pub(crate) struct FinalizeTurnParams<'a> {
    pub state: &'a mut SessionActorState,
    pub session_id: SessionId,
    pub turn: crate::turn::RuntimeTurn,
    pub query_outcome: TurnQueryOutcome,
    pub event_summary: Option<TurnEventStreamSummary>,
}

impl ServerRuntime {
    pub(crate) async fn finalize_executed_turn(self: &Arc<Self>, params: FinalizeTurnParams<'_>) {
        let FinalizeTurnParams {
            state,
            session_id,
            turn,
            query_outcome,
            event_summary,
        } = params;
        // Admission must close before terminal I/O: the actor still advertises
        // this turn until MergeTurn, so its active-turn snapshot is too late.
        if let Some(handle) = self.session(session_id).await {
            let _guard = handle.lock_state_change().await;
            handle.close_queue_steer(turn.turn_id());
        }
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
        let terminal_stop_reason = event_summary
            .as_ref()
            .and_then(|summary| summary.stop_reason.clone());
        let usage_snapshot = self.usage_ledger.session_usage(session_id);
        let turn_usage = usage_snapshot
            .as_ref()
            .map(|_| self.usage_ledger.turn_usage(session_id, turn.turn_id()));
        let latest_query_usage = event_summary
            .as_ref()
            .and_then(|summary| summary.latest_query_usage.clone())
            .or_else(|| self.usage_ledger.latest_query(session_id));
        let terminal_error = result
            .as_ref()
            .err()
            .filter(|error| !matches!(error, devo_core::AgentError::Aborted))
            .map(turn_agent_error_from_error)
            .map(|error| TurnError {
                code: error.error_code,
                message: error.message,
                recovery_hint: error.details.as_ref().and_then(|details| {
                    details
                        .get("recoveryHint")
                        .and_then(|value| value.as_str())
                        .map(str::to_owned)
                }),
            });
        if let Some(snapshot) = usage_snapshot {
            session_total_input_tokens = snapshot.total.input_tokens as usize;
            session_total_output_tokens = snapshot.total.output_tokens as usize;
            session_total_tokens = snapshot.total.total_tokens as usize;
            session_total_cache_creation_tokens =
                snapshot.total.cache_creation_input_tokens as usize;
            session_total_cache_read_tokens = snapshot.total.cache_read_input_tokens as usize;
        }
        // Leave ActiveTurnRegistry turn metadata until after MergeTurn so
        // admission cannot race a free registry against an unmerged working set.
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
                turn_usage,
                latest_query_usage.clone(),
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
        if final_turn.native.status == devo_protocol::native::turn::TurnStatus::Failed
            && final_turn.extras.failure_reason.is_none()
            && let Err(error) = self
                .persist_recovery_disposition(
                    session_id,
                    final_turn.native.id,
                    devo_core::durable_execution::RecoveryDisposition::Available,
                    "This turn ended with a program or provider error.",
                )
                .await
        {
            tracing::warn!(%session_id, %error, "failed to save recovery state");
        }
        if final_turn.native.status == devo_protocol::native::turn::TurnStatus::Interrupted {
            state.core.mark_last_turn_interrupted();
        } else {
            state.core.last_turn_interrupted = false;
        }
        self.clear_steer_input_queue(state, session_id).await;
        self.append_terminal_turn_record(
            state,
            session_id,
            &final_turn,
            latest_query_usage,
            terminal_error.clone(),
        )
        .await;
        append_terminal_history_items(state, terminal_error.as_ref());
        self.finalize_turn_workspace_changes(session_id, &final_turn)
            .await;
        self.emit_terminal_turn_events(
            state,
            session_id,
            &final_turn,
            &result,
            terminal_error.as_ref(),
        )
        .await;
        self.record_terminal_turn_status(
            final_turn.turn_id(),
            super::super::TerminalTurnSnapshot::from_runtime_turn(&final_turn),
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn persist_terminal_turn_state(
        self: &Arc<Self>,
        state: &mut SessionActorState,
        session_id: SessionId,
        turn: &crate::turn::RuntimeTurn,
        result: &Result<(), devo_core::AgentError>,
        turn_usage: Option<TurnUsage>,
        latest_query_usage: Option<TurnUsage>,
        terminal_stop_reason: Option<devo_core::StopReason>,
        session_total_input_tokens: usize,
        session_total_output_tokens: usize,
        session_total_tokens: usize,
        session_total_cache_creation_tokens: usize,
        session_total_cache_read_tokens: usize,
        session_last_input_tokens: usize,
        session_prompt_token_estimate: usize,
    ) -> crate::turn::RuntimeTurn {
        let mut final_turn = turn.clone();
        final_turn.native.completed_at = Some(Utc::now());
        final_turn.native.status = match result {
            Ok(()) => devo_protocol::native::turn::TurnStatus::Completed,
            Err(devo_core::AgentError::Aborted) => {
                devo_protocol::native::turn::TurnStatus::Interrupted
            }
            Err(_) => devo_protocol::native::turn::TurnStatus::Failed,
        };
        final_turn.native.usage = turn_usage;
        final_turn.extras.stop_reason = terminal_stop_reason;
        final_turn.extras.failure_reason = result
            .as_ref()
            .err()
            .and_then(turn_failure_reason_from_error);
        state.latest_turn = Some(final_turn.clone());
        state.active_turn = None;
        state.summary.set_status(SessionStatus::Idle);
        state.summary.updated_at = Utc::now();
        state.summary.last_activity_at = state.summary.updated_at;
        state.summary.set_cumulative_usage(
            session_total_input_tokens,
            session_total_output_tokens,
            session_total_tokens,
            session_total_cache_creation_tokens,
            session_total_cache_read_tokens,
        );
        state.summary.prompt_token_estimate = session_prompt_token_estimate;
        let last_input_for_stats = latest_query_usage
            .as_ref()
            .filter(|usage| {
                !usage
                    .query
                    .unreported_fields
                    .as_ref()
                    .is_some_and(|fields| {
                        fields.contains(&devo_protocol::native::usage::UsageField::InputTokens)
                    })
            })
            .map(|usage| usage.query.input_tokens as usize)
            .unwrap_or(session_last_input_tokens);
        if let Some(usage) = latest_query_usage {
            // Context length uses latest-query display total, not session
            // cumulative total_input/output/tokens.
            state.summary.last_query_usage = Some(usage.clone());
            state.summary.last_query_total_tokens = usage.display_total_tokens() as usize;
            state.core.last_input_tokens = last_input_for_stats;
            state.core.last_turn_tokens = usage.display_total_tokens() as usize;
            if let Some(raw) = state.core.raw_context_breakdown {
                let window = effective_context_window_tokens(state, self);
                let occupancy = super::super::context_occupancy::occupancy_from_raw(
                    window,
                    raw,
                    usage.display_total_tokens(),
                );
                state.core.last_turn_tokens = occupancy.total_tokens as usize;
                state.summary.last_context_occupancy = Some(occupancy);
            }
        }
        state.core.total_input_tokens = session_total_input_tokens;
        state.core.total_output_tokens = session_total_output_tokens;
        state.core.total_tokens = session_total_tokens;
        state.core.total_cache_creation_tokens = session_total_cache_creation_tokens;
        state.core.total_cache_read_tokens = session_total_cache_read_tokens;
        self.refresh_actor_usage(state);
        if !state.summary.ephemeral {
            let stats = SessionStats {
                total_input_tokens: session_total_input_tokens,
                total_output_tokens: session_total_output_tokens,
                total_tokens: session_total_tokens,
                total_cache_creation_tokens: session_total_cache_creation_tokens,
                total_cache_read_tokens: session_total_cache_read_tokens,
                // Persist latest-query input only — turn-aggregate usage would
                // inflate auto-compact after resume via hydrate.
                last_input_tokens: last_input_for_stats,
                turn_count: state.core.turn_count,
                prompt_token_estimate: session_prompt_token_estimate,
                last_context_occupancy: state.summary.last_context_occupancy.clone(),
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

    async fn clear_steer_input_queue(
        self: &Arc<Self>,
        state: &SessionActorState,
        session_id: SessionId,
    ) {
        let is_ephemeral = state.summary.ephemeral;
        let steer_input_queue = Arc::clone(&state.steer_input_queue);
        let leftover: Vec<devo_core::PendingInputItem> = {
            let mut queue = steer_input_queue
                .lock()
                .expect("steer input queue mutex should not be poisoned");
            queue.drain(..).collect()
        };
        if !leftover.is_empty() {
            // Any unconsumed steer degrades into the session turn queue
            // (01 §4.3); normally `SessionState::end_turn` already moved
            // them, this covers terminal paths that skip it.
            let mut queue = state
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            for item in &leftover {
                queue.push_back(item.clone());
            }
        }
        let mut held_ids = Vec::new();
        if !is_ephemeral {
            // Settlement is one transaction: a crash cannot fall between
            // deleting a steer and preserving the unconsumed queued original.
            let queue = state
                .pending_turn_queue
                .lock()
                .expect("pending queue mutex poisoned");
            let ordered: Vec<_> = queue.iter().cloned().collect();
            if let Err(error) = self.deps.db.settle_queue_steers(&session_id, &ordered) {
                tracing::error!(%session_id, %error, "failed to settle queue steers; recovery remains held");
                // Fail closed: a failed settlement must not auto-execute input
                // while its durable receipt is still uncertain.
                drop(queue);
                let mut queue = state
                    .pending_turn_queue
                    .lock()
                    .expect("pending queue mutex poisoned");
                for item in queue.iter_mut() {
                    if !matches!(
                        self.deps.db.queue_steer_receipt(&session_id, &item.id),
                        Ok(None)
                    ) {
                        crate::runtime::handlers::queue_steer::hold_for_review(item);
                        held_ids.push(item.id);
                    }
                }
            }
        }
        for id in held_ids {
            self.broadcast_queue_updated(
                session_id,
                devo_protocol::native::queue::QueueChange::Updated,
                id,
                None,
            )
            .await;
        }
        for item in &leftover {
            self.broadcast_queue_updated(
                session_id,
                devo_protocol::native::queue::QueueChange::Added,
                item.id,
                None,
            )
            .await;
        }
    }

    async fn append_terminal_turn_record(
        self: &Arc<Self>,
        state: &mut SessionActorState,
        session_id: SessionId,
        final_turn: &crate::turn::RuntimeTurn,
        latest_query_usage: Option<TurnUsage>,
        terminal_error: Option<TurnError>,
    ) {
        let rollout_path = state.rollout_path.clone();
        let turn_context = state.core.latest_turn_context.clone();
        let session_context = state.core.session_context.clone();
        let extras = crate::persistence::turn_persistence_extras_from_runtime(
            final_turn,
            /*session_context*/ None,
            turn_context,
            latest_query_usage,
            state.summary.last_context_occupancy.clone(),
        );
        let mut native_turn = final_turn.native.clone();
        native_turn.error = terminal_error.as_ref().map(|error| {
            let mut agent_error = devo_protocol::native::error::AgentError::new(
                error.code.clone(),
                error.message.clone(),
            );
            if let Some(hint) = &error.recovery_hint {
                agent_error.details = Some(serde_json::json!({ "recoveryHint": hint }));
            }
            agent_error
        });
        state.turns_by_id.insert(
            final_turn.turn_id(),
            crate::replay_hydrate::ReplayedTurn::from_native(
                native_turn.clone(),
                Some(extras.clone()),
            ),
        );
        if let Some(rollout_path) = rollout_path
            && let Err(error) = self.rollout_store.append_turn_deduped_at(
                &rollout_path,
                session_id,
                &mut state.session_context_recorded,
                &native_turn,
                Some(extras),
                session_context,
            )
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist terminal turn line");
        }
    }

    async fn emit_terminal_turn_events(
        self: &Arc<Self>,
        state: &SessionActorState,
        session_id: SessionId,
        final_turn: &crate::turn::RuntimeTurn,
        result: &Result<(), devo_core::AgentError>,
        terminal_error: Option<&TurnError>,
    ) {
        if let Err(error) = result {
            if matches!(error, devo_core::AgentError::Aborted) {
                tracing::info!(
                    session_id = %session_id,
                    turn_id = %final_turn.turn_id(),
                    status = ?final_turn.native.status,
                    "turn execution interrupted"
                );
            } else {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %final_turn.turn_id(),
                    status = ?final_turn.native.status,
                    error = %error,
                    "turn execution failed"
                );
            }
        } else {
            tracing::info!(
                session_id = %session_id,
                turn_id = %final_turn.turn_id(),
                status = ?final_turn.native.status,
                total_input_tokens = final_turn.native.usage.as_ref().map(|usage| usage.query.input_tokens),
                total_output_tokens = final_turn.native.usage.as_ref().map(|usage| usage.query.output_tokens),
                "turn execution completed"
            );
        }
        self.handle_subagent_turn_completed_for_actor_state(state, session_id, final_turn)
            .await;
        let mut completed_turn = final_turn.native.clone();
        if let Err(error) = result
            && !matches!(error, devo_core::AgentError::Aborted)
        {
            completed_turn.error = terminal_error.as_ref().map(|error| {
                let mut agent_error = devo_protocol::native::error::AgentError::new(
                    error.code.clone(),
                    error.message.clone(),
                );
                if let Some(hint) = &error.recovery_hint {
                    agent_error.details = Some(serde_json::json!({ "recoveryHint": hint }));
                }
                agent_error
            });
        }
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnCompleted {
                turn: Box::new(completed_turn),
            },
        )
        .await;
        self.broadcast_notification(state.summary.status_changed_notification())
            .await;
        if let Some(occupancy) = state.summary.last_context_occupancy.clone() {
            self.broadcast_notification(
                devo_protocol::native::event::ServerNotification::ContextUsageUpdated {
                    session_id: state.summary.native.id,
                    occupancy,
                },
            )
            .await;
        }
    }
}

fn effective_context_window_tokens(state: &SessionActorState, runtime: &ServerRuntime) -> u64 {
    let model = state
        .core
        .latest_turn_context
        .as_ref()
        .map(|context| &context.model)
        .or_else(|| {
            state
                .summary
                .model_name()
                .and_then(|slug| runtime.deps.model_catalog.get(slug))
        });
    super::super::context_occupancy::occupancy_window_tokens(model)
}

fn append_terminal_history_items(
    state: &mut SessionActorState,
    terminal_error: Option<&TurnError>,
) {
    // Native Turn status/timing is the turn-summary source for first-party
    // clients. Only terminal failures leave a history row, as Item::Warning.
    if let Some(error) = terminal_error {
        let code = error
            .recovery_hint
            .clone()
            .unwrap_or_else(|| error.code.clone());
        state
            .history_items
            .push(crate::persisted_native_item::turn_failure_history_entry(
                code,
                error.message.clone(),
            ));
    }
}
