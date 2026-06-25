use super::super::*;
use crate::TurnInputDisposition;

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
        let Some(session_arc) = self.sessions.lock().await.get(&params.session_id).cloned() else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let (workspace_root, runtime_context) = {
            let session = session_arc.lock().await;
            let workspace_root = params
                .cwd
                .clone()
                .unwrap_or_else(|| session.summary.cwd.clone());
            let runtime_context = if params
                .cwd
                .as_ref()
                .is_some_and(|cwd| cwd != &session.summary.cwd)
            {
                None
            } else {
                Some(Arc::clone(&session.runtime_context))
            };
            (workspace_root, runtime_context)
        };
        let runtime_context = match runtime_context {
            Some(runtime_context) => runtime_context,
            None => match self.deps.context_for_workspace(&workspace_root).await {
                Ok(runtime_context) => runtime_context,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to initialize session workspace: {error}"),
                    );
                }
            },
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
        let (turn, turn_config) = {
            let mut session = session_arc.lock().await;
            if let Some(active_turn) = session.active_turn.as_ref() {
                if queue_policy == TurnStartQueuePolicy::RejectActive {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::TurnAlreadyRunning,
                        "session already has an active prompt turn",
                    );
                }
                let pending_turn_queue = Arc::clone(&session.pending_turn_queue);
                let active_turn_id = active_turn.turn_id;
                let is_ephemeral = session.summary.ephemeral;
                let queued_model = params
                    .model
                    .clone()
                    .or_else(|| session.summary.model.clone());
                let queued_model_binding_id = params
                    .model_binding_id
                    .clone()
                    .or_else(|| session.summary.model_binding_id.clone());
                drop(session);

                let queued_input_id = {
                    let collaboration_mode = params.collaboration_mode;
                    let mut guard = pending_turn_queue
                        .lock()
                        .expect("pending turn queue mutex should not be poisoned");
                    let item = devo_core::PendingInputItem::new(
                        devo_core::PendingInputKind::UserInput {
                            input: params.input.clone(),
                            display_text: display_input.clone(),
                            prompt_text: resolved_input.prompt_text.clone(),
                            prompt_messages: resolved_input.prompt_messages.clone(),
                        },
                        pending_turn_metadata(
                            collaboration_mode,
                            queued_model.clone(),
                            queued_model_binding_id.clone(),
                        ),
                        now,
                    );
                    let queued_input_id = item.id;
                    guard.push_back(item.clone());

                    if !is_ephemeral
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
                    queued_input_id
                };
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
                let old_cwd = session.summary.cwd.clone();
                if old_cwd != cwd {
                    cwd_change = Some((old_cwd, cwd.clone()));
                    session.runtime_context = Arc::clone(&runtime_context);
                }
                session.summary.cwd = cwd.clone();
                session.core_session.lock().await.cwd = cwd;
            }
            if let Some(permission_mode) = params
                .approval_policy
                .as_deref()
                .and_then(permission_mode_from_approval_policy)
            {
                session.core_session.lock().await.config.permission_mode = permission_mode;
                session.config.permission_mode = permission_mode;
            }
            let requested_model = requested_model_selection(
                params.model_binding_id.as_deref(),
                params.model.as_deref(),
                &session.summary,
            );
            let requested_reasoning_effort_selection = params
                .reasoning_effort_selection
                .clone()
                .or_else(|| session.summary.reasoning_effort_selection.clone());
            let turn_config = session.runtime_context.resolve_turn_config(
                requested_model,
                requested_reasoning_effort_selection.clone(),
            );
            let resolved_request = turn_config.model.resolve_reasoning_effort_selection(
                turn_config.reasoning_effort_selection.as_deref(),
            );
            let request_model = turn_config.provider_request_model(&resolved_request.request_model);
            apply_turn_config_to_session_summary(&mut session.summary, &turn_config);
            let turn = TurnMetadata {
                turn_id: TurnId::new(),
                session_id: params.session_id,
                sequence: session
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
            session.summary.status = SessionRuntimeStatus::ActiveTurn;
            session.summary.updated_at = now;
            session.summary.last_activity_at = now;
            session.active_turn = Some(turn.clone());
            (turn, turn_config)
        };
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
        let (record, session_context, turn_context) = {
            let session = session_arc.lock().await;
            let core_session = session.core_session.lock().await;
            (
                session.record.clone(),
                core_session.session_context.clone(),
                core_session.latest_turn_context.clone(),
            )
        };
        if let Some(record) = record
            && let Err(error) = self.rollout_store.append_turn(
                &record,
                build_turn_record(&turn, session_context, turn_context),
            )
        {
            {
                let mut session = session_arc.lock().await;
                if session
                    .active_turn
                    .as_ref()
                    .is_some_and(|active| active.turn_id == turn.turn_id)
                {
                    session.active_turn = None;
                    session.summary.status = SessionRuntimeStatus::Idle;
                    session.summary.updated_at = Utc::now();
                    session.summary.last_activity_at = session.summary.updated_at;
                }
            }
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
        let Some(session_arc) = self.sessions.lock().await.get(&params.session_id).cloned() else {
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
        let now = Utc::now();
        let mut cwd_change = None;
        let (turn, cwd) = {
            let mut session = session_arc.lock().await;
            if session.active_turn.is_some() {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::TurnAlreadyRunning,
                    "cannot run shell command while a turn is active",
                );
            }
            let cwd = params
                .cwd
                .clone()
                .unwrap_or_else(|| session.summary.cwd.clone());
            if let Some(cwd) = params.cwd.clone() {
                let old_cwd = session.summary.cwd.clone();
                if old_cwd != cwd {
                    cwd_change = Some((old_cwd, cwd.clone()));
                    if let Some(runtime_context) = requested_runtime_context.as_ref() {
                        session.runtime_context = Arc::clone(runtime_context);
                    }
                }
                session.summary.cwd = cwd.clone();
                session.core_session.lock().await.cwd = cwd;
            }
            let model = session.summary.model.clone().unwrap_or_default();
            let turn = TurnMetadata {
                turn_id: TurnId::new(),
                session_id: params.session_id,
                sequence: session
                    .latest_turn
                    .as_ref()
                    .map_or(1, |turn| turn.sequence + 1),
                status: TurnStatus::Running,
                kind: devo_core::TurnKind::Other("shell_command".to_string()),
                model: model.clone(),
                model_binding_id: session.summary.model_binding_id.clone(),
                reasoning_effort_selection: session.summary.reasoning_effort_selection.clone(),
                reasoning_effort: session.summary.reasoning_effort,
                request_model: model,
                request_thinking: session.summary.reasoning_effort_selection.clone(),
                started_at: now,
                completed_at: None,
                usage: None,
                stop_reason: None,
                failure_reason: None,
            };
            session.summary.status = SessionRuntimeStatus::ActiveTurn;
            session.summary.updated_at = now;
            session.summary.last_activity_at = now;
            session.active_turn = Some(turn.clone());
            (turn, cwd)
        };
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

        {
            let session = session_arc.lock().await;
            if let Some(record) = session.record.clone()
                && let Err(error) = self
                    .rollout_store
                    .append_turn(&record, build_turn_record(&turn, None, None))
            {
                drop(session);
                self.clear_active_turn_reservation(&session_arc, turn.turn_id)
                    .await;
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist shell command turn start: {error}"),
                );
            }
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
        let Some(session_arc) = self.sessions.lock().await.get(&params.session_id).cloned() else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        let deferred_assistant = {
            let mut session = session_arc.lock().await;
            session.deferred_assistant.take()
        };
        let deferred_reasoning = {
            let mut session = session_arc.lock().await;
            session.deferred_reasoning.take()
        };
        if let Some((item_id, item_seq, text)) = deferred_assistant {
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
        if let Some((item_id, item_seq, text)) = deferred_reasoning {
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

        {
            let mut session = session_arc.lock().await;
            let previous_len = session.pending_user_inputs.len();
            session
                .pending_user_inputs
                .retain(|_, pending| pending.turn_id != params.turn_id);
            let removed_len = previous_len.saturating_sub(session.pending_user_inputs.len());
            if removed_len > 0 {
                tracing::info!(
                    session_id = %params.session_id,
                    turn_id = %params.turn_id,
                    removed_len,
                    "cleared pending request_user_input requests for interrupted turn"
                );
            }
        }

        if let Some(cancel_token) = self
            .active_turn_cancellations
            .lock()
            .await
            .remove(&params.session_id)
        {
            cancel_token.cancel();
        }
        if let Some(task) = self.active_tasks.lock().await.remove(&params.session_id) {
            task.abort();
        }
        Arc::clone(self)
            .close_research_child_agents(params.session_id)
            .await;
        let interrupted_turn = {
            let mut session = session_arc.lock().await;
            let Some(mut turn) = session.active_turn.take() else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::TurnNotFound,
                    "turn is not active",
                );
            };
            if turn.turn_id != params.turn_id {
                session.active_turn = Some(turn);
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::TurnNotFound,
                    "turn does not exist",
                );
            }
            turn.status = TurnStatus::Interrupted;
            turn.completed_at = Some(Utc::now());
            session.latest_turn = Some(turn.clone());
            session.summary.status = SessionRuntimeStatus::Idle;
            session.summary.updated_at = Utc::now();
            session.summary.last_activity_at = session.summary.updated_at;
            let totals = session.core_session.try_lock().ok().map(|core_session| {
                (
                    core_session.total_input_tokens,
                    core_session.total_output_tokens,
                    core_session.total_tokens,
                    core_session.total_cache_creation_tokens,
                    core_session.total_cache_read_tokens,
                    core_session.prompt_token_estimate,
                )
            });
            if let Some((
                total_input_tokens,
                total_output_tokens,
                total_tokens,
                total_cache_creation_tokens,
                total_cache_read_tokens,
                prompt_token_estimate,
            )) = totals
            {
                session.summary.total_input_tokens = total_input_tokens;
                session.summary.total_output_tokens = total_output_tokens;
                session.summary.total_tokens = total_tokens;
                session.summary.total_cache_creation_tokens = total_cache_creation_tokens;
                session.summary.total_cache_read_tokens = total_cache_read_tokens;
                session.summary.prompt_token_estimate = prompt_token_estimate;
            }
            turn
        };
        let (record, session_context, turn_context) = {
            let session = session_arc.lock().await;
            let core_session_lock = session.core_session.try_lock();
            if let Ok(core_session) = core_session_lock {
                (
                    session.record.clone(),
                    core_session.session_context.clone(),
                    core_session.latest_turn_context.clone(),
                )
            } else {
                (session.record.clone(), None, None)
            }
        };
        if let Some(record) = record
            && let Err(error) = self.rollout_store.append_turn(
                &record,
                build_turn_record(&interrupted_turn, session_context, turn_context),
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
        let Some(session_arc) = self.sessions.lock().await.get(&params.session_id).cloned() else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let (turn_id, workspace_root, btw_input_queue, runtime_context) = {
            let session = session_arc.lock().await;
            let Some(turn_id) = session.active_turn.as_ref().map(|turn| turn.turn_id) else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::NoActiveTurn,
                    "no active turn exists",
                );
            };
            if turn_id != params.expected_turn_id {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::ExpectedTurnMismatch,
                    "active turn did not match expectedTurnId",
                );
            }
            let active_turn = session.active_turn.as_ref().expect("active turn exists");
            if active_turn.kind != devo_core::TurnKind::Regular {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::ActiveTurnNotSteerable,
                    "cannot steer a non-regular turn",
                );
            }
            (
                turn_id,
                session.summary.cwd.clone(),
                Arc::clone(&session.btw_input_queue),
                Arc::clone(&session.runtime_context),
            )
        };
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
        btw_input_queue
            .lock()
            .expect("btw input queue mutex should not be poisoned")
            .push_back(item.clone());

        {
            let session = session_arc.lock().await;
            if !session.summary.ephemeral
                && let Err(err) =
                    self.deps
                        .db
                        .push_pending(&params.session_id, QueueType::Btw, &item)
            {
                tracing::warn!(
                    session_id = %params.session_id,
                    error = %err,
                    "failed to persist btw input to database"
                );
            }
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
