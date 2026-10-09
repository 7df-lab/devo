use super::super::*;
use std::panic::AssertUnwindSafe;

use devo_protocol::approx_tokens_from_byte_count;
use devo_protocol::native::item::CompactionTrigger;
use futures::FutureExt;

enum CompactionTurnOutcome {
    Skipped,
    Failed { message: String },
    Canceled,
}

struct SessionCompactRequest {
    session_id: SessionId,
}

/// Options for [`ServerRuntime::run_session_compaction`].
#[derive(Debug, Clone)]
pub(crate) struct CompactionRunOptions {
    pub trigger: CompactionTrigger,
    pub custom_instructions: Option<String>,
}

impl Default for CompactionRunOptions {
    fn default() -> Self {
        Self {
            trigger: CompactionTrigger::Manual,
            custom_instructions: None,
        }
    }
}

fn compaction_trigger_hook_label(trigger: CompactionTrigger) -> &'static str {
    match trigger {
        CompactionTrigger::Manual => "manual",
        CompactionTrigger::AgentRequested => "agentRequested",
        CompactionTrigger::AutoThreshold => "autoThreshold",
        CompactionTrigger::ProviderRetry => "providerRetry",
    }
}

impl ServerRuntime {
    /// Native `session/compact/start` (L2-DES-APP-008 Phase B): lean
    /// params and a canonical turn snapshot result, produced by translating
    /// into the legacy flow and projecting the admitted compaction turn.
    pub(crate) async fn handle_native_session_compact_start(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionCompactStartParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/compact/start params: {error}"),
                    );
                }
            };
        let legacy_session_id = params.session_id;
        let response = self
            .handle_session_compact_translated(
                request_id.clone(),
                SessionCompactRequest {
                    session_id: legacy_session_id,
                },
            )
            .await;
        let Ok(success) =
            serde_json::from_value::<SuccessResponse<TurnStartResult>>(response.clone())
        else {
            return response;
        };
        let TurnStartResult::Started { turn_id, .. } = success.result else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "cannot compact while a turn is active or queued",
            );
        };
        // `spawn_active_runtime_turn_task` has already registered runtime metadata.
        // Compaction may not yet have a stream/spawn snapshot, so the mailbox
        // reservation can miss the active turn. Read the registry the same
        // way native `turn/start` does.
        let Some(turn) = self
            .active_turns
            .active_turn(legacy_session_id)
            .await
            .filter(|turn| turn.id.as_str() == turn_id.to_string())
        else {
            return response;
        };
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_turn::TurnStartResult { turn },
        })
        .expect("serialize canonical session/compact/start response")
    }

    async fn handle_session_compact_translated(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: SessionCompactRequest,
    ) -> serde_json::Value {
        let session_id = params.session_id;
        let Some(session_handle) = self.session(session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        // Busy rejection must not wait on the session actor: turns execute
        // inline on the actor, so a mailbox round-trip here would deadlock
        // while a turn is running. `runtime_active_turn_id` reads the runtime
        // turn cache only; the mailbox-based `try_begin_runtime_turn` below
        // stays the authoritative admission check once the session is idle.
        if self.runtime_active_turn_id(session_id).await.is_some() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "cannot compact while a turn is active or queued",
            );
        }

        let _state_change_guard = session_handle.lock_state_change().await;
        let Some(reservation) = session_handle.turn_reservation_snapshot().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        let requested_model = session_model_selection(&reservation.summary);
        let requested_reasoning_effort_selection =
            reservation.summary.settings.reasoning_effort.clone();
        let turn_config = reservation
            .runtime_context
            .resolve_turn_config(requested_model, requested_reasoning_effort_selection);
        let resolved_request = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());
        let request_model = turn_config.provider_request_model(&resolved_request.request_model);
        let now = Utc::now();
        let native_turn_id = devo_protocol::native::ids::TurnId::new();
        let turn_id = native_turn_id;
        let native_session_id = reservation.summary.native.id;
        let runtime_turn = crate::turn::RuntimeTurn {
            native: devo_protocol::native::turn::Turn {
                id: native_turn_id,
                session_id: native_session_id,
                sequence: reservation
                    .latest_turn
                    .as_ref()
                    .map_or(1, |turn| turn.native.sequence + 1),
                kind: devo_protocol::native::turn::TurnKind::Compaction,
                status: devo_protocol::native::turn::TurnStatus::InProgress,
                model: devo_protocol::native::model::ModelBinding {
                    provider: turn_config
                        .model_binding_id
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    model: request_model,
                    variant: None,
                    reasoning_effort: resolved_request.effective_reasoning_effort,
                },
                collaboration_mode: None,
                started_at: now,
                completed_at: None,
                error: None,
                usage: None,
            },
            extras: crate::turn::RuntimeTurnExtras {
                request_thinking: resolved_request.request_thinking,
                stop_reason: None,
                failure_reason: None,
            },
        };

        if !session_handle
            .try_begin_runtime_turn(runtime_turn.clone(), turn_config)
            .await
            .unwrap_or(false)
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "cannot compact while a turn is active or queued",
            );
        }

        if let Some(persistence) = session_handle.turn_persistence_snapshot().await
            && persistence.rollout_path.is_some()
            && let Err(error) = self
                .persist_turn_line_deduped(session_id, &runtime_turn)
                .await
        {
            let _ = session_handle.clear_active_turn_if_matches(turn_id).await;
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist compaction turn start: {error}"),
            );
        }

        let runtime = Arc::clone(self);
        let turn_for_task = runtime_turn.clone();
        let session_handle_for_task = session_handle.clone();
        let post_spawn_session_id = session_id;
        if let Some(spawn) = session_handle.spawn_snapshot().await {
            self.register_turn_spawn_snapshot(session_id, turn_id, Arc::new(spawn))
                .await;
        }
        self.spawn_active_runtime_turn_task(
            session_id,
            runtime_turn.clone(),
            /*connection_id*/ None,
            async move {
                let compaction_session_id = session_id;
                let runtime_for_panic = Arc::clone(&runtime);
                let session_handle_for_panic = session_handle_for_task.clone();
                let turn_for_panic = turn_for_task.clone();
                if let Err(panic) = AssertUnwindSafe(runtime.run_session_compaction(
                    compaction_session_id,
                    session_handle_for_task,
                    turn_for_task,
                    CompactionRunOptions::default(),
                ))
                .catch_unwind()
                .await
                {
                    tracing::error!(
                        session_id = %compaction_session_id,
                        turn_id = %turn_for_panic.turn_id(),
                        panic = ?panic,
                        "session compaction task panicked"
                    );
                    runtime_for_panic
                        .finalize_manual_compaction_turn(
                            &session_handle_for_panic,
                            compaction_session_id,
                            turn_for_panic.clone(),
                            CompactionTurnOutcome::Failed {
                                message: "compaction failed: panicked".to_string(),
                            },
                            /*compaction_item_id*/ None,
                        )
                        .await;
                    // If the panic happened after claim, finalize is a no-op — still
                    // recover so the session is not left without terminal events.
                    if runtime_for_panic
                        .recent_terminal_turn_status(turn_for_panic.turn_id())
                        .await
                        .is_none()
                    {
                        let _ = runtime_for_panic
                            .recover_orphaned_manual_compaction_interrupt(
                                &session_handle_for_panic,
                                compaction_session_id,
                                turn_for_panic.turn_id(),
                            )
                            .await;
                        if runtime_for_panic
                            .recent_terminal_turn_status(turn_for_panic.turn_id())
                            .await
                            .is_none()
                        {
                            // Actor claim may have cleared runtime metadata too; force
                            // a Failed terminal so admission reopens.
                            let mut failed = turn_for_panic;
                            failed.native.status = devo_protocol::native::turn::TurnStatus::Failed;
                            failed.native.completed_at = Some(Utc::now());
                            failed.native.error =
                                Some(devo_protocol::native::error::AgentError::new(
                                    "COMPACTION_FAILED",
                                    "compaction failed: panicked",
                                ));
                            let failed_runtime = failed.clone();
                            session_handle_for_panic
                                .set_runtime_session_idle(Some(failed_runtime.clone()))
                                .await;
                            runtime_for_panic
                                .clear_active_turn_runtime_handles(compaction_session_id)
                                .await;
                            runtime_for_panic
                                .broadcast_notification(
                                    devo_protocol::native::event::ServerNotification::ContextCompactionFailed {
                                        session_id: failed_runtime.native.session_id,
                                        message: "compaction failed: panicked".to_string(),
                                    },
                                )
                                .await;
                            runtime_for_panic
                                .broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnCompleted {
                turn: Box::new(failed_runtime.native.clone()),
            },
        )
                                .await;
                            runtime_for_panic
                                .broadcast_notification(devo_protocol::native::event::ServerNotification::session_status_changed(compaction_session_id, SessionStatus::Idle, /*active_turn_id*/ None))
                                .await;
                            runtime_for_panic
                                .record_terminal_turn_status(
                                    failed.turn_id(),
                                    TerminalTurnSnapshot::from_runtime_turn(&failed),
                                )
                                .await;
                        }
                    }
                }
            },
        )
        .await;

        tracing::info!(
            session_id = %post_spawn_session_id,
            turn_id = %runtime_turn.turn_id(),
            sequence = runtime_turn.native.sequence,
            "started manual compaction turn"
        );
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::session_status_changed(
                post_spawn_session_id,
                SessionStatus::Active,
                /*active_turn_id*/ None,
            ),
        )
        .await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnStarted {
                turn: Box::new(runtime_turn.native.clone()),
            },
        )
        .await;

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnStartResult::Started {
                turn_id,
                status: TurnStatus::Running,
                accepted_at: now,
            },
        })
        .expect("serialize session/compact response")
    }

    /// Admit a ManualCompaction turn and **await** compaction for an
    /// agent-requested (`compact.run`) schedule. Used at turn end before refine.
    pub(crate) async fn execute_agent_requested_compaction(
        self: &Arc<Self>,
        session_id: SessionId,
        instructions: Option<String>,
    ) {
        let Some(session_handle) = self.session(session_id).await else {
            tracing::warn!(
                %session_id,
                "agent-requested compaction skipped: session unavailable"
            );
            return;
        };
        if self.runtime_active_turn_id(session_id).await.is_some() {
            tracing::warn!(
                %session_id,
                "agent-requested compaction skipped: turn still active"
            );
            return;
        }

        let turn = {
            let _state_change_guard = session_handle.lock_state_change().await;
            let Some(reservation) = session_handle.turn_reservation_snapshot().await else {
                tracing::warn!(
                    %session_id,
                    "agent-requested compaction skipped: session unavailable"
                );
                return;
            };

            let requested_model = session_model_selection(&reservation.summary);
            let requested_reasoning_effort_selection =
                reservation.summary.settings.reasoning_effort.clone();
            let turn_config = reservation
                .runtime_context
                .resolve_turn_config(requested_model, requested_reasoning_effort_selection);
            let resolved_request = turn_config.model.resolve_reasoning_effort_selection(
                turn_config.reasoning_effort_selection.as_deref(),
            );
            let request_model = turn_config.provider_request_model(&resolved_request.request_model);
            let now = Utc::now();
            let native_turn_id = devo_protocol::native::ids::TurnId::new();
            let turn_id = native_turn_id;
            let native_session_id = reservation.summary.native.id;
            let runtime_turn = crate::turn::RuntimeTurn {
                native: devo_protocol::native::turn::Turn {
                    id: native_turn_id,
                    session_id: native_session_id,
                    sequence: reservation
                        .latest_turn
                        .as_ref()
                        .map_or(1, |turn| turn.native.sequence + 1),
                    kind: devo_protocol::native::turn::TurnKind::Compaction,
                    status: devo_protocol::native::turn::TurnStatus::InProgress,
                    model: devo_protocol::native::model::ModelBinding {
                        provider: turn_config
                            .model_binding_id
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string()),
                        model: request_model,
                        variant: None,
                        reasoning_effort: resolved_request.effective_reasoning_effort,
                    },
                    collaboration_mode: None,
                    started_at: now,
                    completed_at: None,
                    error: None,
                    usage: None,
                },
                extras: crate::turn::RuntimeTurnExtras {
                    request_thinking: resolved_request.request_thinking,
                    stop_reason: None,
                    failure_reason: None,
                },
            };

            if !session_handle
                .try_begin_runtime_turn(runtime_turn.clone(), turn_config)
                .await
                .unwrap_or(false)
            {
                tracing::warn!(
                    %session_id,
                    "agent-requested compaction skipped: could not admit turn"
                );
                return;
            }

            if let Some(persistence) = session_handle.turn_persistence_snapshot().await
                && persistence.rollout_path.is_some()
                && let Err(error) = self
                    .persist_turn_line_deduped(session_id, &runtime_turn)
                    .await
            {
                let _ = session_handle.clear_active_turn_if_matches(turn_id).await;
                tracing::warn!(
                    %session_id,
                    %error,
                    "agent-requested compaction failed to persist turn start"
                );
                return;
            }

            if let Some(spawn) = session_handle.spawn_snapshot().await {
                self.register_turn_spawn_snapshot(session_id, turn_id, Arc::new(spawn))
                    .await;
            }
            runtime_turn
        };

        let runtime_turn = turn.clone();
        self.register_active_runtime_turn_execution(
            session_id,
            runtime_turn.clone(),
            /*connection_id*/ None,
        )
        .await;
        tracing::info!(
            session_id = %session_id,
            turn_id = %runtime_turn.turn_id(),
            sequence = runtime_turn.native.sequence,
            "started agent-requested compaction turn"
        );
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::session_status_changed(
                session_id,
                SessionStatus::Active,
                /*active_turn_id*/ None,
            ),
        )
        .await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnStarted {
                turn: Box::new(runtime_turn.native.clone()),
            },
        )
        .await;

        Arc::clone(self)
            .run_session_compaction(
                session_id,
                session_handle,
                turn,
                CompactionRunOptions {
                    trigger: CompactionTrigger::AgentRequested,
                    custom_instructions: instructions,
                },
            )
            .await;
    }

    pub(crate) async fn run_session_compaction(
        self: Arc<Self>,
        session_id: SessionId,
        session_handle: crate::runtime::session_actor::SessionHandle,
        turn: crate::turn::RuntimeTurn,
        options: CompactionRunOptions,
    ) {
        tracing::info!(
            session_id = %session_id,
            turn_id = %turn.turn_id(),
            trigger = ?options.trigger,
            "session compaction task started"
        );
        let Some(started_session) = session_handle.native_session().await else {
            self.finalize_manual_compaction_turn(
                &session_handle,
                session_id,
                turn,
                CompactionTurnOutcome::Failed {
                    message: "compaction failed: session unavailable".to_string(),
                },
                /*compaction_item_id*/ None,
            )
            .await;
            return;
        };
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::ContextCompactionStarted {
                session_id: started_session.id,
                turn_id: turn.native.id,
                trigger: options.trigger,
            },
        )
        .await;
        // Surface "Compacting context" in the Desktop transcript as soon as
        // manual compaction begins — not only after summarization finishes.
        let compaction_item_id = devo_protocol::native::ids::ItemId::new();
        self.broadcast_notification(super::super::turn_exec::manual_compaction_started_event(
            turn.native.session_id,
            turn.native.id,
            compaction_item_id,
            /*item_seq*/ None,
        ))
        .await;
        let trigger_label = compaction_trigger_hook_label(options.trigger);
        let custom_instructions = options
            .custom_instructions
            .as_ref()
            .map(|s| serde_json::Value::String(s.clone()))
            .unwrap_or(serde_json::Value::Null);
        self.run_session_hook(
            session_id,
            devo_core::HookEvent::PreCompact,
            serde_json::Map::from_iter([
                ("trigger".to_string(), serde_json::json!(trigger_label)),
                ("custom_instructions".to_string(), custom_instructions),
            ]),
        )
        .await;

        let cancel_token = self
            .active_turns
            .cancel_token(session_id)
            .await
            .unwrap_or_else(CancellationToken::new);

        // Snapshot under the gate, then release it before the model call so
        // admission / queue / metadata RPCs stay responsive (L2-DES-SERVER-002).
        let (items, token_info, model_slug, request_model, max_tokens, provider_route, budget) = {
            let _state_change_guard = session_handle.lock_state_change().await;
            let Some(runtime_session) = session_handle.export_runtime_session().await else {
                tracing::warn!(session_id = %session_id, "session compaction failed: session unavailable");
                self.finalize_manual_compaction_turn(
                    &session_handle,
                    session_id,
                    turn,
                    CompactionTurnOutcome::Failed {
                        message: "compaction failed: session unavailable".to_string(),
                    },
                    Some(compaction_item_id),
                )
                .await;
                return;
            };
            let core_session = runtime_session.core_session.lock().await;

            let items: Vec<ResponseItem> = core_session
                .messages
                .iter()
                .flat_map(|msg| message_to_response_items(msg.clone()))
                .collect();

            let token_info = TokenInfo {
                input_tokens: core_session.total_input_tokens,
                cached_input_tokens: core_session.total_cache_read_tokens,
                output_tokens: core_session.total_output_tokens,
            };

            let model_selection = session_model_selection(&runtime_session.summary)
                .unwrap_or(&runtime_session.runtime_context.default_model);
            let turn_config = runtime_session.runtime_context.resolve_turn_config(
                Some(model_selection),
                /*reasoning_effort_selection*/ None,
            );
            let resolved_request = turn_config.model.resolve_reasoning_effort_selection(None);
            let model_slug = resolved_request.request_model;
            let request_model = turn_config.provider_request_model(&model_slug);
            let max_tokens = runtime_session
                .runtime_context
                .model_catalog
                .get(&model_slug)
                .and_then(|m| m.max_tokens.map(|t| t as usize))
                .unwrap_or(4096);
            let budget = core_session.config.token_budget.clone();
            let provider_route = turn_config.provider_route.clone();
            drop(core_session);
            drop(runtime_session);
            (
                items,
                token_info,
                model_slug,
                request_model,
                max_tokens,
                provider_route,
                budget,
            )
        };

        tracing::debug!(
            session_id = %session_id,
            turn_id = %turn.turn_id(),
            model = %model_slug,
            request_model = %request_model,
            item_count = items.len(),
            input_tokens = token_info.input_tokens,
            cached_input_tokens = token_info.cached_input_tokens,
            output_tokens = token_info.output_tokens,
            "starting compaction summarization"
        );
        let provider = self.usage_ledger.instrumented_provider(
            {
                // Resolve provider without holding the session gate.
                let Some(runtime_session) = session_handle.export_runtime_session().await else {
                    self.finalize_manual_compaction_turn(
                        &session_handle,
                        session_id,
                        turn,
                        CompactionTurnOutcome::Failed {
                            message: "compaction failed: session unavailable".to_string(),
                        },
                        Some(compaction_item_id),
                    )
                    .await;
                    return;
                };
                runtime_session
                    .runtime_context
                    .provider_for_route(provider_route)
            },
            session_id,
            Some(turn.turn_id()),
            devo_protocol::native::usage::UsagePurpose::Compaction,
        );
        let summarizer =
            DefaultHistorySummarizer::with_models(provider, model_slug, request_model, max_tokens);

        let config = CompactionConfig {
            budget,
            // Proactive: user-requested /compact; preserve latest user suffix.
            kind: CompactionKind::Proactive,
        };

        let result = compact_history(
            &items,
            &token_info,
            &summarizer,
            &config,
            Some(&cancel_token),
        )
        .await;

        // Summarize is done: detach abort so interrupt cannot kill mid-terminalize.
        // Cancel token still works for any remaining cooperative checks.
        self.detach_active_turn_abort(session_id).await;

        // Apply under the gate so replace_state cannot race admission/edit.
        let state_change_guard = session_handle.lock_state_change().await;

        match result {
            Err(devo_core::history::compaction::CompactionError::Canceled) => {
                drop(state_change_guard);
                tracing::info!(
                    session_id = %session_id,
                    turn_id = %turn.turn_id(),
                    "session compaction canceled"
                );
                self.finalize_manual_compaction_turn(
                    &session_handle,
                    session_id,
                    turn,
                    CompactionTurnOutcome::Canceled,
                    Some(compaction_item_id),
                )
                .await;
            }
            Ok(CompactAction::Replaced(compacted_items)) => {
                if cancel_token.is_cancelled() {
                    drop(state_change_guard);
                    self.finalize_manual_compaction_turn(
                        &session_handle,
                        session_id,
                        turn,
                        CompactionTurnOutcome::Canceled,
                        Some(compaction_item_id),
                    )
                    .await;
                    return;
                }
                let Some(mut runtime_session) = session_handle.export_runtime_session().await
                else {
                    drop(state_change_guard);
                    self.finalize_manual_compaction_turn(
                        &session_handle,
                        session_id,
                        turn,
                        CompactionTurnOutcome::Failed {
                            message: "compaction failed: session unavailable".to_string(),
                        },
                        Some(compaction_item_id),
                    )
                    .await;
                    return;
                };
                let mut summary_item = summary_item_from_compacted(
                    &compacted_items,
                    devo_protocol::native::item::CompactionTrigger::Manual,
                );
                // Manual compaction has no post-compaction occupancy yet;
                // record what the context held before it.
                if let devo_protocol::native::item::Item::ContextCompaction { before, .. } =
                    &mut summary_item
                {
                    *before = crate::persisted_native_item::context_usage_from_occupancy(
                        runtime_session.summary.last_context_occupancy.as_ref(),
                    );
                }
                // A failed write must leave the previous prompt installed.
                if let Some(rollout_path) = runtime_session.rollout_path.clone() {
                    // Chain the summary item into the transcript tree: write
                    // its parent edge and the advanced session-leaf pointer so
                    // active-path scoping keeps the compaction row. The actor
                    // re-derives its tip from the rollout's leaf line when
                    // `replace_state` runs below.
                    let prep = session_handle.prepare_persist_item(turn.native.id).await;
                    let parent_id = prep.as_ref().and_then(|prep| prep.transcript_leaf_id);
                    let leaf_epoch = prep
                        .as_ref()
                        .map_or(1, |prep| prep.leaf_epoch.saturating_add(1));
                    let persist = CompactionSummaryPersist {
                        session_id,
                        turn_id: turn.native.id,
                        summary_item_id: compaction_item_id,
                        item_seq: runtime_session.next_item_seq,
                        summary_item: summary_item.clone(),
                        snapshot: build_compaction_snapshot_line(
                            &session_id,
                            &turn.native.id,
                            &compaction_item_id,
                            preserved_item_ids_from_compacted(
                                &runtime_session.persisted_turn_items,
                                &compacted_items,
                            ),
                            runtime_session.summary.last_context_occupancy.clone(),
                        ),
                        parent_id,
                        leaf_epoch,
                    };
                    let runtime = Arc::clone(&self);
                    let committed = tokio::task::spawn_blocking(move || {
                        append_compaction_summary_and_snapshot(
                            &runtime.rollout_store,
                            &rollout_path,
                            persist,
                        )
                    })
                    .await;
                    if let Err(error) = committed.unwrap_or_else(|error| Err(error.into())) {
                        drop(state_change_guard);
                        tracing::warn!(
                            session_id = %session_id,
                            turn_id = %turn.turn_id(),
                            error = %error,
                            "manual compaction persistence failed"
                        );
                        self.finalize_manual_compaction_turn(
                            &session_handle,
                            session_id,
                            turn,
                            CompactionTurnOutcome::Failed {
                                message: format!("compaction persistence failed: {error}"),
                            },
                            Some(compaction_item_id),
                        )
                        .await;
                        return;
                    }
                }
                // Claim terminalization before mutating history so an interrupt that
                // already took `active_turn` cannot race with replace_state.
                if session_handle
                    .clear_active_turn_if_matches(turn.turn_id())
                    .await
                    != Some(true)
                {
                    drop(state_change_guard);
                    return;
                }
                let preserved_item_ids = preserved_item_ids_from_compacted(
                    &runtime_session.persisted_turn_items,
                    &compacted_items,
                );
                let new_messages = devo_core::history::response_items_to_messages(&compacted_items);
                {
                    let (
                        compacted_total_input_tokens,
                        compacted_total_output_tokens,
                        compacted_total_tokens,
                        compacted_total_cache_creation_tokens,
                        compacted_total_cache_read_tokens,
                        compacted_prompt_token_estimate,
                        compacted_occupancy,
                    ) = {
                        let previous_occupancy =
                            runtime_session.summary.last_context_occupancy.clone();
                        let mut core_session = runtime_session.core_session.lock().await;
                        core_session.set_prompt_messages(new_messages);
                        let prompt_bytes = core_session
                            .prompt_source_messages()
                            .iter()
                            .map(|message| {
                                serde_json::to_string(message).map_or(0, |json| json.len())
                            })
                            .sum::<usize>();
                        let conversation_tokens = approx_tokens_from_byte_count(prompt_bytes);
                        let compacted_prompt_token_estimate =
                            conversation_tokens.try_into().unwrap_or(usize::MAX);
                        core_session.prompt_token_estimate = compacted_prompt_token_estimate;
                        let model = runtime_session
                            .summary
                            .model_name()
                            .and_then(|slug| {
                                runtime_session
                                    .runtime_context
                                    .model_catalog
                                    .get(slug)
                                    .or_else(|| self.deps.model_catalog.get(slug))
                            })
                            .or_else(|| {
                                runtime_session
                                    .summary
                                    .model_binding_id()
                                    .and_then(|binding| {
                                        runtime_session
                                            .runtime_context
                                            .model_catalog
                                            .get(binding)
                                            .or_else(|| self.deps.model_catalog.get(binding))
                                    })
                            });
                        let window = runtime_session
                            .summary
                            .settings
                            .effective_context_window
                            .or_else(|| {
                                model
                                    .map(super::super::context_occupancy::resolved_compaction_limit)
                            })
                            .unwrap_or(0);
                        let occupancy = super::super::context_occupancy::occupancy_after_compaction(
                            window,
                            previous_occupancy.as_ref(),
                            conversation_tokens,
                            core_session.raw_context_breakdown,
                        );
                        // Keep auto-compact pressure on the post-compact tip so
                        // resume / next query do not re-trigger from pre-compact
                        // latest-query totals.
                        core_session.last_turn_tokens = occupancy.total_tokens as usize;
                        core_session.last_input_tokens = compacted_prompt_token_estimate;
                        (
                            core_session.total_input_tokens,
                            core_session.total_output_tokens,
                            core_session.total_tokens,
                            core_session.total_cache_creation_tokens,
                            core_session.total_cache_read_tokens,
                            compacted_prompt_token_estimate,
                            occupancy,
                        )
                    };
                    runtime_session.summary.set_cumulative_usage(
                        compacted_total_input_tokens,
                        compacted_total_output_tokens,
                        compacted_total_tokens,
                        compacted_total_cache_creation_tokens,
                        compacted_total_cache_read_tokens,
                    );
                    runtime_session.summary.prompt_token_estimate = compacted_prompt_token_estimate;
                    runtime_session.summary.last_query_total_tokens =
                        compacted_occupancy.total_tokens as usize;
                    runtime_session.summary.last_context_occupancy =
                        Some(compacted_occupancy.clone());
                }

                if !runtime_session.summary.ephemeral {
                    let stats = crate::db::SessionStats {
                        total_input_tokens: runtime_session.summary.total_input_tokens(),
                        total_output_tokens: runtime_session.summary.total_output_tokens(),
                        total_tokens: runtime_session.summary.total_tokens(),
                        total_cache_creation_tokens: runtime_session
                            .summary
                            .total_cache_creation_tokens(),
                        total_cache_read_tokens: runtime_session.summary.total_cache_read_tokens(),
                        last_input_tokens: runtime_session.summary.prompt_token_estimate,
                        turn_count: runtime_session.core_session.lock().await.turn_count,
                        prompt_token_estimate: runtime_session.summary.prompt_token_estimate,
                        last_context_occupancy: runtime_session
                            .summary
                            .last_context_occupancy
                            .clone(),
                    };
                    if let Err(err) = self.deps.db.update_stats(&session_id, &stats) {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %err,
                            "failed to persist compaction token stats to database"
                        );
                    }
                }

                let turn_id = turn.turn_id();
                let item_id = compaction_item_id;
                let item_seq = runtime_session.next_item_seq;
                runtime_session.loaded_item_count += 1;
                runtime_session.next_item_seq += 1;

                self.broadcast_notification(
                    super::super::turn_exec::manual_compaction_completed_event(
                        turn.native.session_id,
                        turn.native.id,
                        item_id,
                        item_seq,
                    ),
                )
                .await;

                let compact_summary = match &summary_item {
                    devo_protocol::native::item::Item::ContextCompaction { summary, .. } => {
                        summary.clone().unwrap_or_default()
                    }
                    _ => String::new(),
                };
                if runtime_session.rollout_path.is_some() {
                    let snapshot = build_compaction_snapshot_line(
                        &session_id,
                        &turn_id,
                        &item_id,
                        preserved_item_ids.clone(),
                        runtime_session.summary.last_context_occupancy.clone(),
                    );
                    runtime_session.latest_compaction_snapshot = Some(snapshot.clone());
                    runtime_session
                        .persisted_turn_items
                        .push(compaction_persisted_turn_item(
                            turn.native.id,
                            devo_protocol::native::turn::TurnKind::Compaction,
                            item_id,
                            summary_item.clone(),
                        ));
                    if let Some(history_item) =
                        crate::persisted_native_item::history_entry_from_native_item(&summary_item)
                    {
                        runtime_session.history_items.push(history_item);
                    }
                }

                let mut completed_turn = turn.clone();
                completed_turn.native.status = devo_protocol::native::turn::TurnStatus::Completed;
                completed_turn.native.completed_at = Some(Utc::now());
                let completed_runtime_turn = completed_turn.clone();
                runtime_session.active_turn = None;
                runtime_session.latest_turn = Some(completed_runtime_turn.clone());
                runtime_session.summary.set_status(SessionStatus::Idle);
                let summary = runtime_session.summary.clone();
                session_handle
                    .replace_state(
                        crate::runtime::session_actor::SessionActorState::from_runtime_session(
                            runtime_session,
                        ),
                    )
                    .await;
                drop(state_change_guard);
                self.clear_active_turn_runtime_handles(session_id).await;
                // Follow-ups queued while compacting must run next: a manual
                // compaction turn never runs the turn_exec post-turn pipeline,
                // so drain here once the session is idle again — otherwise the
                // queued message strands and makes TryBeginActiveTurn reject
                // every later turn ("active or queued"). This mirrors the drain
                // in finalize_manual_compaction_turn for the non-replaced
                // outcomes.
                let drain_runtime = Arc::clone(&self);
                let drain_session_id = session_id;
                tokio::spawn(async move {
                    drain_runtime
                        .spawn_next_turn_from_queue(drain_session_id)
                        .await;
                });
                if let Some(persistence) = session_handle.turn_persistence_snapshot().await
                    && persistence.rollout_path.is_some()
                    && let Err(error) = self
                        .persist_turn_line_deduped(session_id, &completed_turn)
                        .await
                {
                    tracing::warn!(
                        session_id = %session_id,
                        turn_id = %completed_turn.turn_id(),
                        error = %error,
                        "failed to persist compaction turn completion"
                    );
                }
                self.run_session_hook(
                    session_id,
                    devo_core::HookEvent::PostCompact,
                    serde_json::Map::from_iter([
                        ("trigger".to_string(), serde_json::json!(trigger_label)),
                        (
                            "compact_summary".to_string(),
                            serde_json::Value::String(compact_summary),
                        ),
                    ]),
                )
                .await;
                tracing::info!(
                    session_id = %session_id,
                    turn_id = %completed_turn.turn_id(),
                    "session compaction completed with replacement"
                );
                if let Some(occupancy) = summary.last_context_occupancy.clone() {
                    self.broadcast_notification(
                        devo_protocol::native::event::ServerNotification::ContextUsageUpdated {
                            session_id: summary.native.id,
                            occupancy,
                        },
                    )
                    .await;
                }
                let session = session_handle
                    .native_session()
                    .await
                    .unwrap_or_else(|| summary.native.clone());
                self.broadcast_notification(
                    devo_protocol::native::event::ServerNotification::ContextCompactionCompleted {
                        session_id: session.id,
                        turn_id: completed_turn.native.id,
                        item_id,
                    },
                )
                .await;
                self.broadcast_notification(
                    devo_protocol::native::event::ServerNotification::TurnCompleted {
                        turn: Box::new(completed_runtime_turn.native.clone()),
                    },
                )
                .await;
                self.broadcast_notification(summary.status_changed_notification())
                    .await;
                self.record_terminal_turn_status(
                    completed_turn.turn_id(),
                    TerminalTurnSnapshot::from_runtime_turn(&completed_turn),
                )
                .await;
            }
            Ok(CompactAction::Skipped) => {
                drop(state_change_guard);
                tracing::info!(
                    session_id = %session_id,
                    turn_id = %turn.turn_id(),
                    "session compaction completed without replacement"
                );
                self.finalize_manual_compaction_turn(
                    &session_handle,
                    session_id,
                    turn,
                    CompactionTurnOutcome::Skipped,
                    Some(compaction_item_id),
                )
                .await;
            }
            Err(error) => {
                drop(state_change_guard);
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn.turn_id(),
                    error = %error,
                    "session compaction failed"
                );
                self.finalize_manual_compaction_turn(
                    &session_handle,
                    session_id,
                    turn,
                    CompactionTurnOutcome::Failed {
                        message: format!("compaction failed: {error}"),
                    },
                    Some(compaction_item_id),
                )
                .await;
            }
        }
    }

    /// Terminalize a manual compaction turn when the task still owns `active_turn`.
    ///
    /// If interrupt already claimed the turn via `interrupt_active_turn`, this is a
    /// no-op so we do not double-emit terminal events.
    async fn finalize_manual_compaction_turn(
        self: &Arc<Self>,
        session_handle: &crate::runtime::session_actor::SessionHandle,
        session_id: SessionId,
        mut turn: crate::turn::RuntimeTurn,
        outcome: CompactionTurnOutcome,
        compaction_item_id: Option<devo_protocol::native::ids::ItemId>,
    ) {
        // Ensure interrupt abort cannot drop us between claim and event emit.
        self.detach_active_turn_abort(session_id).await;

        let now = Utc::now();
        turn.native.completed_at = Some(now);
        turn.native.status = match &outcome {
            CompactionTurnOutcome::Skipped => devo_protocol::native::turn::TurnStatus::Completed,
            CompactionTurnOutcome::Failed { .. } => devo_protocol::native::turn::TurnStatus::Failed,
            CompactionTurnOutcome::Canceled => devo_protocol::native::turn::TurnStatus::Interrupted,
        };
        // Surface why the compaction turn failed on the turn itself — clients
        // render `turn.error`, and without it a failed compaction shows as a
        // bare "Error" with no explanation.
        if let CompactionTurnOutcome::Failed { message } = &outcome {
            turn.native.error = Some(devo_protocol::native::error::AgentError::new(
                "COMPACTION_FAILED",
                message.clone(),
            ));
        }
        let runtime_turn = turn.clone();

        // Atomic claim: interrupt may have already taken `active_turn`.
        if session_handle
            .clear_active_turn_if_matches(turn.turn_id())
            .await
            != Some(true)
        {
            return;
        }
        session_handle
            .set_runtime_session_idle(Some(runtime_turn.clone()))
            .await;
        self.clear_active_turn_runtime_handles(session_id).await;

        // A manual compaction turn never runs the turn_exec post-turn
        // pipeline, so follow-ups queued while compacting would strand here
        // forever — and their presence makes TryBeginActiveTurn reject every
        // later turn or compaction ("active or queued"). Drain once the
        // session is idle again.
        let drain_runtime = Arc::clone(self);
        let drain_session_id = session_id;
        tokio::spawn(async move {
            drain_runtime
                .spawn_next_turn_from_queue(drain_session_id)
                .await;
        });

        if let Some(persistence) = session_handle.turn_persistence_snapshot().await
            && persistence.rollout_path.is_some()
            && let Err(error) = self
                .persist_turn_line_deduped(session_id, &runtime_turn)
                .await
        {
            tracing::warn!(
                session_id = %session_id,
                turn_id = %turn.turn_id(),
                error = %error,
                "failed to persist compaction turn terminal line"
            );
        }

        // Close the early-emitted started item so Desktop does not leave a
        // dangling "Compacting context" divider when compact does not replace.
        if let Some(item_id) = compaction_item_id {
            match &outcome {
                CompactionTurnOutcome::Skipped => {
                    // Match the TUI contract: short sessions warn instead of
                    // claiming "Context compacted" with no history change.
                    self.broadcast_notification(
                        super::super::turn_exec::manual_compaction_item_failed_event(
                            turn.native.session_id,
                            turn.native.id,
                            item_id,
                            "Session is too short to compact — try again once it grows".to_string(),
                        ),
                    )
                    .await;
                }
                CompactionTurnOutcome::Failed { message } => {
                    self.broadcast_notification(
                        super::super::turn_exec::manual_compaction_item_failed_event(
                            turn.native.session_id,
                            turn.native.id,
                            item_id,
                            message.clone(),
                        ),
                    )
                    .await;
                }
                CompactionTurnOutcome::Canceled => {
                    self.broadcast_notification(
                        super::super::turn_exec::manual_compaction_item_failed_event(
                            turn.native.session_id,
                            turn.native.id,
                            item_id,
                            "compaction canceled".to_string(),
                        ),
                    )
                    .await;
                }
            }
        }

        match outcome {
            CompactionTurnOutcome::Skipped => {
                let Some(summary) = session_handle.summary().await else {
                    tracing::warn!(
                        session_id = %session_id,
                        turn_id = %turn.turn_id(),
                        "compaction skipped but session summary unavailable"
                    );
                    self.broadcast_notification(
                        devo_protocol::native::event::ServerNotification::TurnCompleted {
                            turn: Box::new(runtime_turn.native.clone()),
                        },
                    )
                    .await;
                    self.broadcast_notification(
                        devo_protocol::native::event::ServerNotification::session_status_changed(
                            session_id,
                            SessionStatus::Idle,
                            /*active_turn_id*/ None,
                        ),
                    )
                    .await;
                    self.record_terminal_turn_status(
                        turn.turn_id(),
                        TerminalTurnSnapshot::from_runtime_turn(&turn),
                    )
                    .await;
                    return;
                };
                if let Some(occupancy) = summary.last_context_occupancy.clone() {
                    self.broadcast_notification(
                        devo_protocol::native::event::ServerNotification::ContextUsageUpdated {
                            session_id: summary.native.id,
                            occupancy,
                        },
                    )
                    .await;
                }
                let session = session_handle
                    .native_session()
                    .await
                    .unwrap_or_else(|| summary.native.clone());
                if let Some(item_id) = compaction_item_id {
                    self.broadcast_notification(
                        devo_protocol::native::event::ServerNotification::ContextCompactionCompleted {
                            session_id: session.id,
                            turn_id: turn.native.id,
                            item_id,
                        },
                    )
                    .await;
                }
                self.broadcast_notification(
                    devo_protocol::native::event::ServerNotification::TurnCompleted {
                        turn: Box::new(runtime_turn.native.clone()),
                    },
                )
                .await;
            }
            CompactionTurnOutcome::Failed { message } => {
                self.broadcast_notification(
                    devo_protocol::native::event::ServerNotification::ContextCompactionFailed {
                        session_id: turn.native.session_id,
                        message,
                    },
                )
                .await;
                self.broadcast_notification(
                    devo_protocol::native::event::ServerNotification::TurnCompleted {
                        turn: Box::new(runtime_turn.native.clone()),
                    },
                )
                .await;
            }
            CompactionTurnOutcome::Canceled => {
                self.broadcast_notification(
                    devo_protocol::native::event::ServerNotification::ContextCompactionFailed {
                        session_id: turn.native.session_id,
                        message: "compaction canceled".to_string(),
                    },
                )
                .await;
                self.broadcast_notification(
                    devo_protocol::native::event::ServerNotification::TurnCompleted {
                        turn: Box::new(runtime_turn.native.clone()),
                    },
                )
                .await;
            }
        }

        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::session_status_changed(
                session_id,
                SessionStatus::Idle,
                /*active_turn_id*/ None,
            ),
        )
        .await;
        self.record_terminal_turn_status(
            turn.turn_id(),
            TerminalTurnSnapshot::from_runtime_turn(&turn),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use devo_protocol::native::ids::ItemId;

    #[test]
    fn preserved_item_ids_match_complete_command_execution_pair() {
        let command_item_id = ItemId::new();
        let command_input = serde_json::json!({ "cmd": "printf ok" });
        let command_output = serde_json::Value::String("ok".to_string());
        let persisted_turn_items = vec![crate::persisted_native_item::PersistedNativeItem::new(
            TurnId::new(),
            devo_protocol::native::turn::TurnKind::Regular,
            command_item_id,
            devo_protocol::native::item::Item::CommandExecution {
                call_id: "call-1".to_string(),
                command: "printf ok".to_string(),
                argv: None,
                cwd: Default::default(),
                input: Some(command_input.clone()),
                output: Some(command_output.clone()),
                exit_code: None,
                execution_handle: None,
                is_error: false,
                execution_mode: devo_protocol::native::item::ExecutionMode::Foreground,
                origin: devo_protocol::native::item::ExecOrigin::AgentTool,
                sandbox: None,
            },
        )];
        let compacted_items = vec![
            ResponseItem::Message(Message::assistant_text("summary")),
            ResponseItem::ToolCall {
                id: "call-1".to_string(),
                name: "exec_command".to_string(),
                input: command_input,
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "call-1".to_string(),
                content: "ok".to_string(),
                is_error: false,
            },
        ];

        // The journal's single CommandExecution item covers both the call and
        // its output; it must be listed once, not duplicated.
        assert_eq!(
            preserved_item_ids_from_compacted(&persisted_turn_items, &compacted_items),
            vec![command_item_id]
        );
    }
}
