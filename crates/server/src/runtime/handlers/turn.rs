use super::super::*;
use super::image_input::write_data_uri_image_to_temp;

/// Immutable admission result, captured before releasing the session gate.
pub(crate) enum TurnAdmissionSnapshot {
    Started(Box<devo_protocol::native::turn::Turn>),
    Queued(devo_protocol::native::queue::QueueEntry),
}

fn pending_turn_metadata(
    collaboration_mode: devo_protocol::CollaborationMode,
    model: Option<String>,
    model_binding_id: Option<String>,
    client_user_message_id: Option<&str>,
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
    if let Some(client_user_message_id) = client_user_message_id {
        metadata.insert(
            "clientUserMessageId".to_string(),
            serde_json::Value::String(client_user_message_id.to_string()),
        );
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
        self.handle_native_turn_start(connection_id, request_id, params)
            .await
    }

    /// Native `turn/start` (L2-DES-APP-008 Phase B): lean params (input +
    /// idempotency key; per-turn model/settings moved to settings updates),
    /// busy sessions reject with `TURN_ALREADY_RUNNING` (clients use
    /// `session/queue/push`). Approval checkpoints are not implicitly cancelled
    /// by data input, and the result carries
    /// the canonical turn snapshot. Idempotent replays return the originally
    /// started turn.
    async fn handle_native_turn_start(
        self: &Arc<Self>,
        connection_id: Option<u64>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_turn::TurnStartParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical turn/start params: {error}"),
                    );
                }
            };
        let session_id = params.session_id;
        // Normalize Image data-URIs to LocalImage paths; keep Native UserInput
        // end-to-end (no legacy InputItem conversion).
        let mut input = Vec::with_capacity(params.input.len());
        for item in &params.input {
            use devo_protocol::native::item::UserInput;
            let converted = match item {
                UserInput::Image {
                    uri,
                    mime_type,
                    detail,
                } => {
                    let path = match write_data_uri_image_to_temp(uri, mime_type.as_deref()) {
                        Ok(path) => path,
                        Err(error) => {
                            return self.error_response(
                                request_id,
                                ProtocolErrorCode::InvalidParams,
                                format!("failed to decode image input: {error}"),
                            );
                        }
                    };
                    UserInput::LocalImage {
                        path,
                        detail: *detail,
                    }
                }
                UserInput::Audio { .. } => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        "audio inputs are not served by canonical turn/start yet",
                    );
                }
                other => other.clone(),
            };
            input.push(converted);
        }
        // Idempotent replay: return the originally started turn snapshot.
        let idempotency_key = (session_id, params.idempotency_key.clone());
        if let Some(turn) = self
            .turn_start_idempotency
            .lock()
            .await
            .get(&idempotency_key)
            .cloned()
        {
            return serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_turn::TurnStartResult { turn },
            })
            .expect("serialize canonical turn/start response");
        }

        let collaboration_mode = match self.session(session_id).await {
            Some(handle) => handle.collaboration_mode().await.unwrap_or_default(),
            None => Default::default(),
        };
        if let Err(error) = self.cancel_saved_turn(session_id).await {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                error.to_string(),
            );
        }
        let turn_params = TurnStartParams {
            session_id,
            input,
            model: None,
            model_binding_id: None,
            reasoning_effort_selection: None,
            sandbox: None,
            approval_policy: None,
            cwd: None,
            collaboration_mode,
            execution_mode: Default::default(),
        };
        let mut accepted_turn = None;
        let response = self
            .handle_turn_start_with_queue_policy_and_client_message_id(
                connection_id,
                request_id.clone(),
                turn_params,
                TurnStartQueuePolicy::RejectActive,
                Some(
                    params
                        .client_user_message_id
                        .clone()
                        .unwrap_or_else(|| format!("turn:{}", params.idempotency_key)),
                ),
                &mut accepted_turn,
            )
            .await;
        let Ok(success) =
            serde_json::from_value::<SuccessResponse<TurnStartResult>>(response.clone())
        else {
            return response;
        };
        let TurnStartResult::Started { turn_id, .. } = success.result else {
            // A turn raced in between the reservation check and admission;
            // canonical busy semantics reject instead of queueing.
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "session already has an active prompt turn",
            );
        };
        // Admission owns the response snapshot. The background turn may already
        // have ended (or been replaced) before this wrapper resumes.
        let Some(TurnAdmissionSnapshot::Started(turn)) = accepted_turn.filter(|snapshot| {
            matches!(snapshot, TurnAdmissionSnapshot::Started(turn) if turn.id == turn_id)
        }) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                "started turn has no admission snapshot",
            );
        };
        let turn = *turn;
        self.turn_start_idempotency
            .lock()
            .await
            .insert(idempotency_key, turn.clone());
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_turn::TurnStartResult { turn },
        })
        .expect("serialize canonical turn/start response")
    }

    pub(crate) async fn handle_turn_start_with_queue_policy(
        self: &Arc<Self>,
        connection_id: Option<u64>,
        request_id: serde_json::Value,
        params: TurnStartParams,
        queue_policy: TurnStartQueuePolicy,
    ) -> serde_json::Value {
        let mut accepted_turn = None;
        self.handle_turn_start_with_queue_policy_and_client_message_id(
            connection_id,
            request_id,
            params,
            queue_policy,
            /*client_user_message_id*/ None,
            &mut accepted_turn,
        )
        .await
    }

    /// Return the accepted Native turn or queue snapshot alongside the internal compatibility
    /// response, without consulting mutable active-turn state after execution starts.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle_turn_start_with_queue_policy_and_client_message_id(
        self: &Arc<Self>,
        connection_id: Option<u64>,
        request_id: serde_json::Value,
        params: TurnStartParams,
        queue_policy: TurnStartQueuePolicy,
        client_user_message_id: Option<String>,
        accepted_turn: &mut Option<TurnAdmissionSnapshot>,
    ) -> serde_json::Value {
        *accepted_turn = None;
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
        let Some(reservation) = self
            .session_turn_reservation_snapshot(params.session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let requested_cwd = params.cwd.clone();
        let workspace_root = requested_cwd
            .clone()
            .unwrap_or_else(|| reservation.summary.cwd.clone());
        let runtime_context = if requested_cwd
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
        if let Some(binding_id) = params.model_binding_id.as_deref() {
            let binding_error = {
                let config_store = runtime_context
                    .config_store
                    .lock()
                    .expect("app config store mutex should not be poisoned");
                let provider_config = config_store.effective_config().provider_catalog_config();
                let user_config_dir = config_store.user_config_dir().to_path_buf();
                let effective = devo_core::effective_provider_catalog_with_home(
                    &provider_config,
                    Some(user_config_dir.as_path()),
                )
                .unwrap_or_else(|_| provider_config.clone());
                match effective.resolve_model(Some(binding_id)) {
                    Ok(_) => None,
                    Err(error) => Some(error.to_string()),
                }
            };
            if let Some(error) = binding_error {
                return self.error_response(request_id, ProtocolErrorCode::InvalidParams, error);
            }
        }
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
        // Resolve hooks and input outside admission: hooks may perform I/O.
        // Re-snapshot under the short gate before checking idle/queue order.
        let state_change_guard = session_handle.lock_state_change().await;
        let Some(reservation) = self
            .session_turn_reservation_snapshot(params.session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let now = Utc::now();
        let mut cwd_change = None;
        if reservation.active_turn.is_some()
            || !reservation
                .pending_turn_queue
                .lock()
                .expect("pending queue mutex poisoned")
                .is_empty()
        {
            if queue_policy == TurnStartQueuePolicy::RejectActive {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::TurnAlreadyRunning,
                    "session already has active or queued input",
                );
            }
            let active_turn_id = reservation
                .active_turn
                .as_ref()
                .or(reservation.latest_turn.as_ref())
                .map_or_else(TurnId::new, |turn| turn.turn_id());
            let queued_model = params
                .model
                .or_else(|| reservation.summary.model_name().map(str::to_string));
            let queued_model_binding_id = params
                .model_binding_id
                .or_else(|| reservation.summary.model_binding_id().map(str::to_string));
            let mut item = devo_core::PendingInputItem::new(
                devo_core::PendingInputKind::UserInput {
                    input: params.input.clone(),
                    display_text: display_input.clone(),
                    prompt_text: resolved_input.prompt_text.clone(),
                    prompt_messages: resolved_input.prompt_messages.clone(),
                    prompt_images: resolved_input.images.clone(),
                },
                pending_turn_metadata(
                    params.collaboration_mode,
                    queued_model,
                    queued_model_binding_id,
                    client_user_message_id.as_deref(),
                ),
                now,
            );
            item.assign_client_identity();
            let queued_input_id = item.id;
            if let Err(error) = self.persist_inbox_item(
                params.session_id,
                reservation.ephemeral,
                &reservation.pending_turn_queue,
                &item,
            ) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist inbox input: {error}"),
                );
            }
            let queue_len_after = {
                let queue = reservation
                    .pending_turn_queue
                    .lock()
                    .expect("pending queue mutex poisoned");
                *accepted_turn = super::queue::native_queue_entries(&queue)
                    .into_iter()
                    .find(|entry| entry.queue_item_id == queued_input_id)
                    .map(TurnAdmissionSnapshot::Queued);
                queue.len()
            };
            tracing::info!(
                session_id = %params.session_id,
                queued_input_id = %queued_input_id,
                queue_len_after,
                active_turn_id = %active_turn_id,
                active_kind = ?reservation.active_turn.as_ref().map(|turn| turn.native.kind),
                "queued follow-up input while turn active"
            );
            let sid = params.session_id;
            // The gate-free enqueue can race the post-turn drain: if the
            // active turn ended between the snapshot and this push and the
            // followup chain already found an empty queue, the entry would
            // strand. Kick an idle-only drain; it no-ops otherwise.
            let runtime = Arc::clone(self);
            tokio::spawn(async move {
                runtime.drain_queue_if_idle(sid).await;
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
            .or_else(|| reservation.summary.settings.reasoning_effort.clone());
        let turn_config = match runtime_context.resolve_session_turn_config(
            &reservation.summary,
            requested_model,
            requested_reasoning_effort_selection,
        ) {
            Ok(config) => config,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    error.to_string(),
                );
            }
        };
        let model_binding_id = turn_config
            .model_binding_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let resolved_request = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());
        let request_model = turn_config.provider_request_model(&resolved_request.request_model);
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
                kind: devo_protocol::native::turn::TurnKind::Regular,
                status: devo_protocol::native::turn::TurnStatus::InProgress,
                model: devo_protocol::native::model::ModelBinding {
                    provider: model_binding_id,
                    model: request_model,
                    variant: turn_config.variant.clone(),
                    reasoning_effort: resolved_request.effective_reasoning_effort,
                },
                collaboration_mode: Some(params.collaboration_mode),
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
        let mut inbox_item = devo_protocol::PendingInputItem::new(
            devo_protocol::PendingInputKind::UserInput {
                input: params.input.clone(),
                display_text: display_input.clone(),
                prompt_text: resolved_input.prompt_text.clone(),
                prompt_messages: resolved_input.prompt_messages.clone(),
                prompt_images: resolved_input.images.clone(),
            },
            pending_turn_metadata(
                params.collaboration_mode,
                None,
                None,
                client_user_message_id.as_deref(),
            ),
            now,
        );
        inbox_item.assign_client_identity();
        if let Err(error) = self.persist_inbox_item(
            params.session_id,
            reservation.ephemeral,
            &reservation.pending_turn_queue,
            &inbox_item,
        ) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("inbox admission failed: {error}"),
            );
        }
        {
            let mut queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending queue mutex poisoned");
            if queue.front().is_none_or(|head| head.id != inbox_item.id) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::TurnAlreadyRunning,
                    "input already admitted or waiting behind older input",
                );
            }
            if !reservation.ephemeral {
                match self
                    .deps
                    .db
                    .claim_queue_steer(&params.session_id, &inbox_item, &turn_id)
                {
                    Ok(true) => {}
                    Ok(false) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InternalError,
                            "input already claimed",
                        );
                    }
                    Err(error) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InternalError,
                            format!("inbox claim failed: {error}"),
                        );
                    }
                }
            }
            queue.pop_front();
        }
        session_handle
            .begin_runtime_turn(runtime_turn.clone(), turn_config.clone())
            .await;
        drop(state_change_guard);
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
            && persistence.rollout_path.is_some()
            && let Err(error) = self
                .persist_turn_line_deduped(params.session_id, &runtime_turn)
                .await
        {
            self.restore_claimed_inbox_head(params.session_id, inbox_item.clone(), turn_id)
                .await;
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist turn start: {error}"),
            );
        }

        if let Some(spawn) = session_handle.spawn_snapshot().await {
            self.register_turn_spawn_snapshot(params.session_id, turn_id, Arc::new(spawn))
                .await;
        }

        // First untitled session: record first user input and apply an
        // immediate heuristic title. LLM polish runs after the turn merges via
        // notify_title_polish — never inline here (client turn/start timeouts).
        self.prepare_title_from_user_input(params.session_id, &display_input)
            .await;

        // Project the user bubble before spawn / TurnStarted so IM is not
        // blocked on the turn task or workspace baseline I/O.
        let user_message_text =
            crate::runtime::items::render_input_text_without_images(&params.input);
        let user_message = if let Some(client_user_message_id) = client_user_message_id {
            crate::runtime::items::native_user_message_item_with_client_message_id(
                user_message_text,
                &resolved_input.image_paths,
                devo_protocol::native::item::UserMessageEntry::TurnStart,
                client_user_message_id,
            )
        } else {
            crate::runtime::items::native_user_message_item(
                user_message_text,
                &resolved_input.image_paths,
                devo_protocol::native::item::UserMessageEntry::TurnStart,
            )
        };
        self.emit_turn_native_item(
            runtime_turn.native.session_id,
            runtime_turn.native.id,
            user_message,
        )
        .await;

        let runtime = Arc::clone(self);
        let turn_for_task = runtime_turn.clone();
        let display_input_for_task = display_input.clone();
        let input_for_task = resolved_input.prompt_text.clone();
        let input_messages_for_task = resolved_input.prompt_messages.clone();
        let input_images_for_task = resolved_input.images.clone();
        let input_image_paths_for_task = resolved_input.image_paths.clone();
        let turn_config_for_task = turn_config.clone();
        let collaboration_mode = params.collaboration_mode;
        let session_id = params.session_id;
        self.add_active_turn_user_text(session_id, turn_id, display_input.clone())
            .await;
        self.spawn_active_runtime_turn_task(
            params.session_id,
            runtime_turn.clone(),
            connection_id,
            async move {
                runtime
                    .execute_turn(ExecuteTurnRequest {
                        session_id,
                        turn: turn_for_task,
                        turn_config: turn_config_for_task,
                        display_input: display_input_for_task,
                        client_user_message_id: None,
                        input: input_for_task,
                        input_messages: input_messages_for_task,
                        input_images: input_images_for_task,
                        input_image_paths: input_image_paths_for_task,
                        collaboration_mode,
                        input_mode: TurnInputMode::VisibleUserMessage,
                        user_message_already_emitted: true,
                    })
                    .await;
            },
        )
        .await;

        tracing::info!(
            session_id = %params.session_id,
            turn_id = %turn_id,
            sequence = runtime_turn.native.sequence,
            request_model = %runtime_turn.native.model.model,
            input_chars = resolved_input.prompt_text.len(),
            "started turn"
        );
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::session_status_changed(
                params.session_id,
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

        #[cfg(test)]
        self.pause_turn_admission_response_for_test(params.session_id)
            .await;
        *accepted_turn = Some(TurnAdmissionSnapshot::Started(Box::new(
            runtime_turn.native.clone(),
        )));
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnStartResult::Started {
                turn_id,
                status: TurnStatus::Running,
                accepted_at: now,
            },
        })
        .expect("serialize turn/start response")
    }
}

#[cfg(test)]
mod client_user_message_metadata_tests {
    use pretty_assertions::assert_eq;

    use super::pending_turn_metadata;

    #[test]
    fn queued_turn_metadata_includes_client_user_message_id_atomically() {
        assert_eq!(
            pending_turn_metadata(
                devo_protocol::CollaborationMode::Build,
                None,
                None,
                Some("message-id-1"),
            ),
            Some(serde_json::json!({"clientUserMessageId":"message-id-1"}))
        );
    }
}
