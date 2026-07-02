use super::super::*;
use crate::TurnInputDisposition;
use crate::TurnQueueRemoveParams;
use crate::TurnQueueRemoveResult;
use crate::TurnQueueSteerParams;
use crate::TurnQueueSteerResult;

fn pending_turn_metadata(
    collaboration_mode: devo_protocol::CollaborationMode,
    model: Option<String>,
    model_binding_id: Option<String>,
) -> Option<serde_json::Value> {
    let mut metadata = serde_json::Map::new();
    if collaboration_mode != devo_protocol::CollaborationMode::Build {
        metadata.insert(
            "collaboration_mode".to_string(),
            serde_json::json!(collaboration_mode),
        );
    }
    if let Some(model_binding_id) = model_binding_id {
        metadata.insert(
            "model_binding_id".to_string(),
            serde_json::Value::String(model_binding_id),
        );
    }
    if let Some(model) = model {
        metadata.insert("model".to_string(), serde_json::Value::String(model));
    }
    (!metadata.is_empty()).then_some(serde_json::Value::Object(metadata))
}

impl ServerRuntime {
    pub(crate) async fn handle_turn_start_for_connection(
        self: &Arc<Self>,
        connection_id: Option<u64>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: TurnStartParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid turn/start params: {error}"),
                );
            }
        };
        self.handle_turn_start_with_queue_policy(
            connection_id,
            request_id,
            params,
            TurnStartQueuePolicy::Queue,
        )
        .await
    }

    pub(crate) async fn handle_turn_start_with_queue_policy(
        self: &Arc<Self>,
        connection_id: Option<u64>,
        request_id: serde_json::Value,
        params: TurnStartParams,
        queue_policy: TurnStartQueuePolicy,
    ) -> serde_json::Value {
        if params.input.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "turn input is empty",
            );
        }
        let Some(display_input) = render_input_items(&params.input) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "turn input is empty",
            );
        };
        let Some(session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let Some(reservation) = session_handle.turn_reservation_snapshot().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let workspace_root = params
            .cwd
            .clone()
            .unwrap_or_else(|| reservation.summary.cwd.clone());
        let runtime_context = if params
            .cwd
            .as_ref()
            .is_some_and(|cwd| cwd != &reservation.summary.cwd)
        {
            match self.deps.context_for_workspace(&workspace_root).await {
                Ok(runtime_context) => runtime_context,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to initialize session workspace: {error}"),
                    );
                }
            }
        } else {
            reservation.runtime_context
        };
        let Some(resolved_input) = (match runtime_context
            .resolve_input_items(&params.input, Some(workspace_root.as_path()))
        {
            Ok(resolved_input) => resolved_input,
            Err(error) => {
                let code = match error {
                    devo_core::SkillError::SkillNotFound { .. }
                    | devo_core::SkillError::AmbiguousSkillName { .. }
                    | devo_core::SkillError::SkillDisabled { .. } => {
                        ProtocolErrorCode::InvalidParams
                    }
                    devo_core::SkillError::SkillParseFailed { .. }
                    | devo_core::SkillError::SkillRootUnavailable { .. }
                    | devo_core::SkillError::DuplicateSkillId { .. } => {
                        ProtocolErrorCode::InternalError
                    }
                };
                return self.error_response(
                    request_id,
                    code,
                    format!("failed to resolve turn input: {error}"),
                );
            }
        }) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "turn input is empty",
            );
        };
        let prompt_hook_report = self
            .run_session_hook(
                params.session_id,
                devo_core::HookEvent::UserPromptSubmit,
                serde_json::Map::from_iter([(
                    "prompt".to_string(),
                    serde_json::Value::String(resolved_input.prompt_text.clone()),
                )]),
            )
            .await;
        if let Some(reason) = prompt_hook_report.first_blocking_reason() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::PolicyDenied,
                format!("prompt blocked by hook: {reason}"),
            );
        }
        if params.execution_mode == devo_protocol::TurnExecutionMode::Research {
            return self
                .handle_research_turn_start(
                    connection_id,
                    request_id,
                    params,
                    display_input,
                    resolved_input.prompt_text,
                )
                .await;
        }

        let now = Utc::now();
        let mut cwd_change = None;
        if let Some(active_turn) = reservation.active_turn.as_ref() {
            if queue_policy == TurnStartQueuePolicy::RejectActive {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::TurnAlreadyRunning,
                    "session already has an active prompt turn",
                );
            }
            let active_turn_id = active_turn.turn_id;
            let queued_model = params
                .model
                .clone()
                .or_else(|| reservation.summary.model.clone());
            let queued_model_binding_id = params
                .model_binding_id
                .clone()
                .or_else(|| reservation.summary.model_binding_id.clone());
            let item = devo_core::PendingInputItem::new(
                devo_core::PendingInputKind::UserInput {
                    input: params.input.clone(),
                    display_text: display_input.clone(),
                    prompt_text: resolved_input.prompt_text.clone(),
                    prompt_messages: resolved_input.prompt_messages.clone(),
                },
                pending_turn_metadata(
                    params.collaboration_mode,
                    queued_model,
                    queued_model_binding_id,
                ),
                now,
            );
            let queued_input_id = item.id;
            session_handle
                .enqueue_pending_turn_input(item.clone())
                .await;
            if !reservation.ephemeral
                && let Err(err) =
                    self.deps
                        .db
                        .push_pending(&params.session_id, QueueType::Turn, &item)
            {
                tracing::warn!(
                    session_id = %params.session_id,
                    error = %err,
                    "failed to persist pending turn message to database"
                );
            }
            let sid = params.session_id;
            let runtime = Arc::clone(self);
            tokio::spawn(async move {
                runtime.broadcast_updated_queue(sid).await;
            });
            return serde_json::to_value(SuccessResponse {
                id: request_id,
                result: TurnStartResult::Queued {
                    active_turn_id,
                    queued_input_id,
                    status: TurnStatus::Pending,
                    accepted_at: now,
                },
            })
            .expect("serialize queued turn/start response");
        }
        if let Some(cwd) = params.cwd.clone() {
            let old_cwd = reservation.summary.cwd.clone();
            if old_cwd != cwd {
                cwd_change = Some((old_cwd, cwd.clone()));
                session_handle
                    .update_session_workspace(cwd.clone(), Arc::clone(&runtime_context))
                    .await;
            }
        }
        if let Some(permission_mode) = params
            .approval_policy
            .as_deref()
            .and_then(permission_mode_from_approval_policy)
        {
            session_handle
                .update_core_permission_mode(permission_mode)
                .await;
        }
        let requested_model = requested_model_selection(
            params.model_binding_id.as_deref(),
            params.model.as_deref(),
            &reservation.summary,
        );
        let requested_reasoning_effort_selection = params
            .reasoning_effort_selection
            .clone()
            .or_else(|| reservation.summary.reasoning_effort_selection.clone());
        let turn_config = runtime_context
            .resolve_turn_config(requested_model, requested_reasoning_effort_selection);
        let resolved_request = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());
        let request_model = turn_config.provider_request_model(&resolved_request.request_model);
        let turn = TurnMetadata {
            turn_id: TurnId::new(),
            session_id: params.session_id,
            sequence: reservation
                .latest_turn
                .as_ref()
                .map_or(1, |turn| turn.sequence + 1),
            status: TurnStatus::Running,
            kind: devo_core::TurnKind::Regular,
            model: turn_config.model.slug.clone(),
            model_binding_id: turn_config.model_binding_id.clone(),
            reasoning_effort_selection: turn_config.reasoning_effort_selection.clone(),
            reasoning_effort: resolved_request.effective_reasoning_effort,
            request_model,
            request_thinking: resolved_request.request_thinking,
            started_at: now,
            completed_at: None,
            usage: None,
            stop_reason: None,
            failure_reason: None,
        };
        session_handle
            .begin_active_turn(turn.clone(), turn_config.clone())
            .await;
        if let Some((old_cwd, new_cwd)) = cwd_change {
            self.run_session_hook(
                params.session_id,
                devo_core::HookEvent::CwdChanged,
                serde_json::Map::from_iter([
                    (
                        "old_cwd".to_string(),
                        serde_json::Value::String(old_cwd.display().to_string()),
                    ),
                    (
                        "new_cwd".to_string(),
                        serde_json::Value::String(new_cwd.display().to_string()),
                    ),
                ]),
            )
            .await;
        }
        self.maybe_start_title_generation_from_user_input(params.session_id, &display_input)
            .await;
        if let Some(persistence) = session_handle.turn_persistence_snapshot().await
            && let Some(record) = persistence.record
            && let Err(error) = self.rollout_store.append_turn(
                &record,
                build_turn_record(
                    &turn,
                    persistence.session_context,
                    persistence.latest_turn_context,
                ),
            )
        {
            let _ = session_handle
                .clear_active_turn_if_matches(turn.turn_id)
                .await;
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist turn start: {error}"),
            );
        }

        self.broadcast_event(ServerEvent::InputQueueUpdated(
            devo_core::InputQueueUpdatedPayload {
                session_id: params.session_id,
                pending_count: 0,
                pending_texts: vec![],
            },
        ))
        .await;
        self.active_turn_cancellations
            .lock()
            .await
            .insert(params.session_id, CancellationToken::new());
        self.active_turn_ids
            .lock()
            .await
            .insert(params.session_id, turn.turn_id);
        if let Some(connection_id) = connection_id {
            self.active_turn_connections
                .lock()
                .await
                .insert(params.session_id, connection_id);
        }
        let runtime = Arc::clone(self);
        let turn_for_task = turn.clone();
        let display_input_for_task = display_input.clone();
        let input_for_task = resolved_input.prompt_text.clone();
        let input_messages_for_task = resolved_input.prompt_messages.clone();
        let turn_config_for_task = turn_config.clone();
        let collaboration_mode = params.collaboration_mode;
        let session_id = params.session_id;
        let task = tokio::spawn(async move {
            runtime
                .execute_turn(ExecuteTurnRequest {
                    session_id,
                    turn: turn_for_task,
                    turn_config: turn_config_for_task,
                    display_input: display_input_for_task,
                    input: input_for_task,
                    input_messages: input_messages_for_task,
                    collaboration_mode,
                    input_mode: TurnInputMode::VisibleUserMessage,
                })
                .await;
        });
        self.active_tasks
            .lock()
            .await
            .insert(params.session_id, task.abort_handle());

        tracing::info!(
            session_id = %params.session_id,
            turn_id = %turn.turn_id,
            sequence = turn.sequence,
            request_model = %turn.request_model,
            input_chars = resolved_input.prompt_text.len(),
            "started turn"
        );
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id: params.session_id,
                status: SessionRuntimeStatus::ActiveTurn,
            },
        ))
        .await;
        self.broadcast_event(ServerEvent::TurnStarted(TurnEventPayload {
            session_id: params.session_id,
            turn: turn.clone(),
        }))
        .await;

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnStartResult::Started {
                turn_id: turn.turn_id,
                status: turn.status.clone(),
                accepted_at: now,
            },
        })
        .expect("serialize turn/start response")
    }

    pub(crate) async fn handle_turn_shell_command_for_connection(
        self: &Arc<Self>,
        connection_id: Option<u64>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: ShellCommandParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid turn/shell_command params: {error}"),
                );
            }
        };
        let command = params.command.trim().to_string();
        if command.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "shell command is empty",
            );
        }
        let Some(session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        let requested_runtime_context = match params.cwd.as_ref() {
            Some(cwd) => match self.deps.context_for_workspace(cwd).await {
                Ok(runtime_context) => Some(runtime_context),
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to initialize session workspace: {error}"),
                    );
                }
            },
            None => None,
        };
        let Some(reservation) = session_handle.turn_reservation_snapshot().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        if reservation.active_turn.is_some() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "cannot run shell command while a turn is active",
            );
        }
        let now = Utc::now();
        let mut cwd_change = None;
        let cwd = params
            .cwd
            .clone()
            .unwrap_or_else(|| reservation.summary.cwd.clone());
        if let Some(cwd) = params.cwd.clone() {
            let old_cwd = reservation.summary.cwd.clone();
            if old_cwd != cwd {
                cwd_change = Some((old_cwd, cwd.clone()));
                if let Some(runtime_context) = requested_runtime_context.as_ref() {
                    session_handle
                        .update_session_workspace(cwd.clone(), Arc::clone(runtime_context))
                        .await;
                }
            }
        }
        let model = reservation.summary.model.clone().unwrap_or_default();
        let turn = TurnMetadata {
            turn_id: TurnId::new(),
            session_id: params.session_id,
            sequence: reservation
                .latest_turn
                .as_ref()
                .map_or(1, |turn| turn.sequence + 1),
            status: TurnStatus::Running,
            kind: devo_core::TurnKind::Other("shell_command".to_string()),
            model: model.clone(),
            model_binding_id: reservation.summary.model_binding_id.clone(),
            reasoning_effort_selection: reservation.summary.reasoning_effort_selection.clone(),
            reasoning_effort: reservation.summary.reasoning_effort,
            request_model: model,
            request_thinking: reservation.summary.reasoning_effort_selection.clone(),
            started_at: now,
            completed_at: None,
            usage: None,
            stop_reason: None,
            failure_reason: None,
        };
        session_handle
            .begin_active_turn(
                turn.clone(),
                reservation.runtime_context.resolve_turn_config(
                    session_model_selection(&reservation.summary),
                    reservation.summary.reasoning_effort_selection.clone(),
                ),
            )
            .await;
        if let Some((old_cwd, new_cwd)) = cwd_change {
            self.run_session_hook(
                params.session_id,
                devo_core::HookEvent::CwdChanged,
                serde_json::Map::from_iter([
                    (
                        "old_cwd".to_string(),
                        serde_json::Value::String(old_cwd.display().to_string()),
                    ),
                    (
                        "new_cwd".to_string(),
                        serde_json::Value::String(new_cwd.display().to_string()),
                    ),
                ]),
            )
            .await;
        }

        if let Some(persistence) = session_handle.turn_persistence_snapshot().await
            && let Some(record) = persistence.record
            && let Err(error) = self
                .rollout_store
                .append_turn(&record, build_turn_record(&turn, None, None))
        {
            self.clear_active_turn_reservation(&session_handle, turn.turn_id)
                .await;
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist shell command turn start: {error}"),
            );
        }

        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id: params.session_id,
                status: SessionRuntimeStatus::ActiveTurn,
            },
        ))
        .await;
        self.broadcast_event(ServerEvent::TurnStarted(TurnEventPayload {
            session_id: params.session_id,
            turn: turn.clone(),
        }))
        .await;

        let runtime = Arc::clone(self);
        let command_for_task = command.clone();
        let turn_for_task = turn.clone();
        self.active_turn_cancellations
            .lock()
            .await
            .insert(params.session_id, CancellationToken::new());
        self.active_turn_ids
            .lock()
            .await
            .insert(params.session_id, turn.turn_id);
        if let Some(connection_id) = connection_id {
            self.active_turn_connections
                .lock()
                .await
                .insert(params.session_id, connection_id);
        }
        let task = tokio::spawn(async move {
            runtime
                .execute_shell_command_turn(params.session_id, turn_for_task, command_for_task, cwd)
                .await;
        });
        self.active_tasks
            .lock()
            .await
            .insert(params.session_id, task.abort_handle());

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: ShellCommandResult {
                turn_id: turn.turn_id,
                status: turn.status,
                accepted_at: now,
            },
        })
        .expect("serialize turn/shell_command response")
    }

    pub(crate) async fn handle_turn_interrupt(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: TurnInterruptParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid turn/interrupt params: {error}"),
                );
            }
        };
        let Some(session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        // Cancel before any session-actor mailbox round-trip: the actor may be blocked
        // waiting for a permission response and cannot process commands until cancelled.
        if let Some(cancel_token) = self
            .active_turn_cancellations
            .lock()
            .await
            .get(&params.session_id)
            .cloned()
        {
            cancel_token.cancel();
        }
        if let Some(task) = self.active_tasks.lock().await.remove(&params.session_id) {
            task.abort();
        }

        let removed_len = self
            .session_interactive
            .clear_pending_user_inputs_for_turn(params.session_id, params.turn_id)
            .await;
        if removed_len > 0 {
            tracing::info!(
                session_id = %params.session_id,
                turn_id = %params.turn_id,
                removed_len,
                "cleared pending request_user_input requests for interrupted turn"
            );
        }

        // Cancel via a clone rather than `remove`: see the comment in
        // `interrupt_child_runtime_work` for why removing here races with
        // `run_turn_model_query` fetching the same token.
        let close_children_parent = params.session_id;
        Arc::clone(self)
            .interrupt_all_child_agents(close_children_parent)
            .await;
        if self.runtime_active_turn_id(params.session_id).await != Some(params.turn_id) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnNotFound,
                "turn is not active",
            );
        }
        let Some(interrupted_turn) = session_handle.interrupt_active_turn().await.flatten() else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnNotFound,
                "turn is not active",
            );
        };
        if interrupted_turn.turn_id != params.turn_id {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnNotFound,
                "turn does not exist",
            );
        }

        let deferred = session_handle.take_deferred_items().await;
        if let Some((item_id, item_seq, text)) = deferred.assistant {
            self.complete_item(
                params.session_id,
                params.turn_id,
                item_id,
                item_seq,
                ItemKind::AgentMessage,
                TurnItem::AgentMessage(TextItem { text: text.clone() }),
                serde_json::json!({ "title": "Assistant", "text": text }),
            )
            .await;
        }
        if let Some((item_id, item_seq, text)) = deferred.reasoning {
            self.complete_item(
                params.session_id,
                params.turn_id,
                item_id,
                item_seq,
                ItemKind::Reasoning,
                TurnItem::Reasoning(TextItem { text: text.clone() }),
                serde_json::json!({ "title": "Reasoning", "text": text }),
            )
            .await;
        }
        if let Some(persistence) = session_handle.turn_persistence_snapshot().await
            && let Some(record) = persistence.record
            && let Err(error) = self.rollout_store.append_turn(
                &record,
                build_turn_record(
                    &interrupted_turn,
                    persistence.session_context,
                    persistence.latest_turn_context,
                ),
            )
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist interrupted turn: {error}"),
            );
        }

        tracing::info!(
            session_id = %params.session_id,
            turn_id = %interrupted_turn.turn_id,
            status = ?interrupted_turn.status,
            "interrupted turn"
        );
        self.finalize_turn_workspace_changes(params.session_id, &interrupted_turn)
            .await;
        self.broadcast_event(ServerEvent::TurnInterrupted(TurnEventPayload {
            session_id: params.session_id,
            turn: interrupted_turn.clone(),
        }))
        .await;
        self.broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
            session_id: params.session_id,
            turn: interrupted_turn.clone(),
        }))
        .await;
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id: params.session_id,
                status: SessionRuntimeStatus::Idle,
            },
        ))
        .await;
        self.record_terminal_turn_status(
            interrupted_turn.turn_id,
            TerminalTurnSnapshot::from_turn(&interrupted_turn),
        )
        .await;

        let runtime = Arc::clone(self);
        let sid = params.session_id;
        tokio::spawn(async move {
            runtime.spawn_next_turn_from_queue(sid).await;
        });

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnInterruptResult {
                turn_id: interrupted_turn.turn_id,
                status: interrupted_turn.status,
            },
        })
        .expect("serialize turn/interrupt response")
    }

    pub(crate) async fn handle_turn_steer(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: TurnSteerParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid turn/steer params: {error}"),
                );
            }
        };
        if params.input.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "turn steer input is empty",
            );
        }
        let Some(display_input) = render_input_items(&params.input) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "turn steer input is empty",
            );
        };
        let Some(session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let Some(reservation) = session_handle.turn_reservation_snapshot().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let Some(active_turn) = reservation.active_turn.as_ref() else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::NoActiveTurn,
                "no active turn exists",
            );
        };
        let turn_id = active_turn.turn_id;
        if turn_id != params.expected_turn_id {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ExpectedTurnMismatch,
                "active turn did not match expectedTurnId",
            );
        }
        if active_turn.kind != devo_core::TurnKind::Regular {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ActiveTurnNotSteerable,
                "cannot steer a non-regular turn",
            );
        }
        let workspace_root = reservation.summary.cwd.clone();
        let runtime_context = reservation.runtime_context;
        let resolved_input = match runtime_context
            .resolve_input_items(&params.input, Some(workspace_root.as_path()))
        {
            Ok(Some(resolved_input)) => resolved_input,
            Ok(None) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::EmptyInput,
                    "turn steer input is empty",
                );
            }
            Err(error) => {
                let code = match error {
                    devo_core::SkillError::SkillNotFound { .. }
                    | devo_core::SkillError::AmbiguousSkillName { .. }
                    | devo_core::SkillError::SkillDisabled { .. } => {
                        ProtocolErrorCode::InvalidParams
                    }
                    devo_core::SkillError::SkillParseFailed { .. }
                    | devo_core::SkillError::SkillRootUnavailable { .. }
                    | devo_core::SkillError::DuplicateSkillId { .. } => {
                        ProtocolErrorCode::InternalError
                    }
                };
                return self.error_response(
                    request_id,
                    code,
                    format!("failed to resolve turn steer input: {error}"),
                );
            }
        };

        self.emit_turn_item(
            params.session_id,
            turn_id,
            ItemKind::UserMessage,
            TurnItem::SteerInput(TextItem {
                text: display_input.clone(),
            }),
            serde_json::json!({ "title": "You", "text": display_input.clone() }),
        )
        .await;
        let item = devo_core::PendingInputItem::new(
            devo_core::PendingInputKind::UserInput {
                input: params.input.clone(),
                display_text: display_input,
                prompt_text: resolved_input.prompt_text,
                prompt_messages: resolved_input.prompt_messages,
            },
            None,
            chrono::Utc::now(),
        );
        session_handle.enqueue_btw_input(item.clone()).await;

        if !reservation.ephemeral
            && let Err(err) = self
                .deps
                .db
                .push_pending(&params.session_id, QueueType::Btw, &item)
        {
            tracing::warn!(
                session_id = %params.session_id,
                error = %err,
                "failed to persist btw input to database"
            );
        }

        self.emit_to_connection(
            connection_id,
            "serverRequest/resolved",
            ServerEvent::ServerRequestResolved(ServerRequestResolvedPayload {
                session_id: params.session_id,
                request_id: "steer-accepted".into(),
                turn_id: Some(turn_id),
            }),
        )
        .await;
        tracing::info!(
            connection_id,
            session_id = %params.session_id,
            turn_id = %turn_id,
            input_items = params.input.len(),
            "accepted turn steer request"
        );
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnSteerResult {
                turn_id,
                disposition: TurnInputDisposition::Steered,
            },
        })
        .expect("serialize turn/steer response")
    }

    pub(crate) async fn handle_turn_queue_remove(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: TurnQueueRemoveParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid turn/queue/remove params: {error}"),
                );
            }
        };
        let Some(session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let Some(reservation) = session_handle.turn_reservation_snapshot().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let pending_turn_queue = reservation.pending_turn_queue;
        let is_ephemeral = reservation.ephemeral;
        let removed = {
            let mut queue = pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            let before = queue.len();
            queue.retain(|item| item.id != params.queued_input_id);
            queue.len() != before
        };
        if removed
            && !is_ephemeral
            && let Err(error) = self.deps.db.remove_pending_by_id(
                &params.session_id,
                QueueType::Turn,
                &params.queued_input_id,
            )
        {
            tracing::warn!(
                session_id = %params.session_id,
                queued_input_id = %params.queued_input_id,
                error = %error,
                "failed to remove pending turn message from database"
            );
        }
        if removed {
            self.broadcast_updated_queue(params.session_id).await;
        }
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnQueueRemoveResult { removed },
        })
        .expect("serialize turn/queue/remove response")
    }

    pub(crate) async fn handle_turn_queue_steer(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: TurnQueueSteerParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid turn/queue/steer params: {error}"),
                );
            }
        };
        let Some(session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let Some(reservation) = session_handle.turn_reservation_snapshot().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let Some(active_turn) = reservation.active_turn.as_ref() else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::NoActiveTurn,
                "no active turn exists",
            );
        };
        if active_turn.turn_id != params.expected_turn_id {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ExpectedTurnMismatch,
                "active turn did not match expectedTurnId",
            );
        }
        if active_turn.kind != devo_core::TurnKind::Regular {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ActiveTurnNotSteerable,
                "cannot steer a non-regular turn",
            );
        }
        let turn_id = active_turn.turn_id;
        let pending_turn_queue = reservation.pending_turn_queue;
        let is_ephemeral = reservation.ephemeral;
        let (item, display_input) = {
            let mut queue = pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            let Some(index) = queue
                .iter()
                .position(|item| item.id == params.queued_input_id)
            else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "queued input does not exist",
                );
            };
            let display_input = match &queue[index].kind {
                devo_core::PendingInputKind::UserText { text } => text.clone(),
                devo_core::PendingInputKind::UserInput { display_text, .. } => display_text.clone(),
                _ => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        "queued input cannot be steered",
                    );
                }
            };
            let item = queue
                .remove(index)
                .expect("queued item index should remain valid");
            (item, display_input)
        };

        session_handle.enqueue_btw_input(item.clone()).await;

        if !is_ephemeral {
            if let Err(error) = self.deps.db.remove_pending_by_id(
                &params.session_id,
                QueueType::Turn,
                &params.queued_input_id,
            ) {
                tracing::warn!(
                    session_id = %params.session_id,
                    queued_input_id = %params.queued_input_id,
                    error = %error,
                    "failed to remove steered queued message from database"
                );
            }
            if let Err(error) = self
                .deps
                .db
                .push_pending(&params.session_id, QueueType::Btw, &item)
            {
                tracing::warn!(
                    session_id = %params.session_id,
                    queued_input_id = %params.queued_input_id,
                    error = %error,
                    "failed to persist steered queued message to database"
                );
            }
        }

        self.emit_turn_item(
            params.session_id,
            turn_id,
            ItemKind::UserMessage,
            TurnItem::SteerInput(TextItem {
                text: display_input.clone(),
            }),
            serde_json::json!({ "title": "You", "text": display_input }),
        )
        .await;
        self.broadcast_updated_queue(params.session_id).await;
        self.emit_to_connection(
            connection_id,
            "serverRequest/resolved",
            ServerEvent::ServerRequestResolved(ServerRequestResolvedPayload {
                session_id: params.session_id,
                request_id: "queued-steer-accepted".into(),
                turn_id: Some(turn_id),
            }),
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnQueueSteerResult {
                turn_id,
                disposition: TurnInputDisposition::Steered,
            },
        })
        .expect("serialize turn/queue/steer response")
    }

    pub(crate) async fn handle_events_subscribe(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: EventsSubscribeParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid events/subscribe params: {error}"),
                );
            }
        };
        if let Some(connection) = self.connections.lock().await.get_mut(&connection_id) {
            connection.subscriptions.push(SubscriptionFilter {
                session_id: params.session_id,
                event_types: params.event_types.unwrap_or_default().into_iter().collect(),
                include_child_agents: params.include_child_agents,
            });
        }
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: EventsSubscribeResult {
                subscription_id: format!("sub-{connection_id}-1").into(),
            },
        })
        .expect("serialize events/subscribe response")
    }
}
