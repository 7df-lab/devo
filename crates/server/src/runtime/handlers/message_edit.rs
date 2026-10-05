use super::super::*;
use super::message_edit_restore::{
    apply_safe_workspace_restore, candidate_files, core_restore_policy,
    discover_restore_candidates, restore_completed_notification, restore_started_notification,
};

struct MessageEditRequest {
    session_id: SessionId,
    target_message_id: Option<devo_protocol::ItemId>,
    expected_target_message_id: Option<devo_protocol::ItemId>,
    edited_content_parts: Vec<serde_json::Value>,
    edited_mentions: Vec<serde_json::Value>,
    edit_mode: devo_protocol::native::rpc_session::MessageEditMode,
    client_edit_id: Option<String>,
    workspace_restore_policy:
        Option<devo_protocol::native::rpc_session::MessageEditWorkspaceRestore>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MessageEditResult {
    edit_id: String,
    replacement_message_id: devo_protocol::ItemId,
    replacement_turn_id: Option<TurnId>,
    edit_state: String,
}

impl ServerRuntime {
    /// Native `session/message/edit` (ratified #10; L1-REQ-CONV-005):
    /// runs the shared edit machinery and projects the superseding
    /// `UserMessage` revision. `expectedRevision` is enforced
    /// against the rollout-backed canonical history (`0` skips).
    ///
    /// If a turn is active for the target message, the server interrupts
    /// that turn first (without holding `state_change_gate`), then applies
    /// the same completed/interrupted-turn edit path.
    pub(crate) async fn handle_native_session_message_edit(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionMessageEditParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/message/edit params: {error}"),
                    );
                }
            };
        let legacy_session_id = params.session_id;
        let target_item_id = devo_protocol::ItemId::from(params.item_id.as_str());

        if params.expected_revision > 0 {
            match self
                .native_item_revision(legacy_session_id, params.item_id.as_str())
                .await
            {
                Some(revision) if revision == params.expected_revision => {}
                Some(_) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::WorkspaceVersionConflict,
                        "item revision changed; refetch before editing",
                    );
                }
                None => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidContentParts,
                        "item does not exist in the canonical history",
                    );
                }
            }
        }

        let edited_input = match super::queue::normalize_user_inputs(&params.content) {
            Ok(input) => input,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid canonical session/message/edit content: {error}"),
                );
            }
        };
        let edited_content_parts: Vec<serde_json::Value> = edited_input
            .iter()
            .map(|item| serde_json::to_value(item).expect("serialize input item"))
            .collect();

        let edit_params = MessageEditRequest {
            session_id: legacy_session_id,
            target_message_id: Some(target_item_id),
            expected_target_message_id: None,
            edited_content_parts,
            edited_mentions: Vec::new(),
            edit_mode: params.mode,
            client_edit_id: Some(params.idempotency_key.clone()),
            workspace_restore_policy: Some(params.workspace_restore),
        };
        let response = self
            .handle_message_edit(connection_id, request_id.clone(), edit_params)
            .await;
        let Ok(success) =
            serde_json::from_value::<SuccessResponse<MessageEditResult>>(response.clone())
        else {
            return response;
        };
        let result = success.result;

        // Facade envelope for the superseding UserMessage revision (same
        // concession as the other facade items: seq 0, history is the truth).
        let now = Utc::now();
        let item = devo_protocol::native::item::ItemEnvelope {
            id: devo_protocol::native::ids::ItemId::from_string(
                result.replacement_message_id.to_string(),
            ),
            session_id: params.session_id,
            turn_id: result
                .replacement_turn_id
                .expect("message edit must produce a replacement turn"),
            seq: 0,
            revision: params.expected_revision + 1,
            created_at: now,
            updated_at: now,
            state: devo_protocol::native::item::ItemState::Completed,
            item: devo_protocol::native::item::Item::UserMessage {
                client_user_message_id: None,
                content: params.content.clone(),
                entry: match params.mode {
                    devo_protocol::native::rpc_session::MessageEditMode::Normal => {
                        devo_protocol::native::item::UserMessageEntry::TurnStart
                    }
                    devo_protocol::native::rpc_session::MessageEditMode::QueuedOnly => {
                        devo_protocol::native::item::UserMessageEntry::Queue
                    }
                },
            },
            parent_id: None,
        };
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionMessageEditResult {
                item,
                replacement_turn_id: result.replacement_turn_id.map(|turn_id| {
                    devo_protocol::native::ids::TurnId::from_string(turn_id.to_string())
                }),
                edit_state: match result.edit_state.as_str() {
                    "queued" => devo_protocol::native::rpc_session::MessageEditState::Queued,
                    _ => devo_protocol::native::rpc_session::MessageEditState::Accepted,
                },
            },
        })
        .expect("serialize canonical session/message/edit response")
    }

    /// Current revision of one item in the rollout-backed canonical history.
    async fn native_item_revision(&self, session_id: SessionId, item_id: &str) -> Option<u32> {
        let rollout_path = self
            .deps
            .db
            .get_session_index(&session_id)
            .ok()
            .flatten()
            .and_then(|index| index.rollout_path)
            .or_else(|| {
                self.rollout_store
                    .find_rollout_by_session_id(&session_id)
                    .ok()
                    .flatten()
            })?;
        let history = devo_core::read_canonical_history(&rollout_path).ok()?;
        history
            .items
            .iter()
            .find(|item| item.id.as_str() == item_id)
            .map(|item| item.revision)
    }

    /// Interrupts the session's active turn before message edit takes the
    /// state-change gate. Validates the edit target against durable
    /// canonical history first — active-turn user messages live in the turn
    /// working set until MergeTurn, so `export_runtime_session` is not
    /// authoritative mid-turn.
    async fn interrupt_active_turn_before_message_edit(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        _session_handle: &SessionHandle,
        params: &MessageEditRequest,
    ) -> Result<(), serde_json::Value> {
        let Some(active_turn_id) = self.runtime_active_turn_id(params.session_id).await else {
            return Ok(());
        };
        let expected_target_message_id = params
            .expected_target_message_id
            .or(params.target_message_id);
        let Some(expected) = expected_target_message_id else {
            return Err(self.error_response(
                request_id,
                ProtocolErrorCode::InvalidContentParts,
                "session/message/edit requires a target item id",
            ));
        };
        let native_session_id =
            devo_protocol::native::ids::SessionId::from_string(params.session_id.to_string());
        let history = match self
            .load_canonical_history(&request_id, native_session_id)
            .await
        {
            Ok(history) => history,
            Err(response) => return Err(response),
        };
        let Some(latest_user) = history.items.iter().rev().find(|item| {
            matches!(
                &item.item,
                devo_protocol::native::item::Item::UserMessage { .. }
            )
        }) else {
            return Err(self.error_response(
                request_id,
                ProtocolErrorCode::OlderMessageRequiresFork,
                "no immediately previous user message is available to edit",
            ));
        };
        if latest_user.id.as_str() != expected.to_string() {
            return Err(self.error_response(
                request_id,
                ProtocolErrorCode::ExpectedTargetMessageMismatch,
                "expected target message does not match the current editable message",
            ));
        }

        let interrupt_response = self
            .interrupt_turn(
                request_id.clone(),
                serde_json::to_value(TurnInterruptParams {
                    session_id: params.session_id,
                    turn_id: active_turn_id,
                    reason: Some("interrupted by session/message/edit".to_string()),
                })
                .expect("serialize internal turn interruption"),
            )
            .await;
        if interrupt_response.get("error").is_some() {
            return Err(interrupt_response);
        }
        let _ = self
            .command_exec_manager
            .terminate_session(connection_id, params.session_id)
            .await;
        Ok(())
    }

    async fn handle_message_edit(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: MessageEditRequest,
    ) -> serde_json::Value {
        let session_id = params.session_id;
        let edited_input = match params
            .edited_content_parts
            .iter()
            .cloned()
            .map(serde_json::from_value::<devo_protocol::native::item::UserInput>)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(input) => input,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidContentParts,
                    format!("invalid session/message/edit content: {error}"),
                );
            }
        };
        let Some(display_input) = render_input_items(&edited_input) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidContentParts,
                "session/message/edit content is empty",
            );
        };

        let Some(session_handle) = self.session(session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        // Interrupt any active turn before taking `state_change_gate` so the
        // mailbox stays short while we wait for terminalization (up to 5s).
        if let Err(response) = self
            .interrupt_active_turn_before_message_edit(
                connection_id,
                request_id.clone(),
                &session_handle,
                &params,
            )
            .await
        {
            return response;
        }

        let _state_change_guard = session_handle.lock_state_change().await;
        if self.runtime_active_turn_id(session_id).await.is_some() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ActiveTurnEditRejected,
                "cannot edit the previous message while a turn is active",
            );
        }
        let Some(hook_context) = session_handle.hook_context_snapshot().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let workspace_root = hook_context.summary.cwd.clone();
        let runtime_context = hook_context.runtime_context;
        let Some(resolved_input) = (match runtime_context
            .resolve_input_items(&edited_input, Some(workspace_root.as_path()))
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
                    format!("failed to resolve session/message/edit input: {error}"),
                );
            }
        }) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidContentParts,
                "session/message/edit content is empty",
            );
        };
        let prompt_hook_report = self
            .run_session_hook(
                session_id,
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
        let edited_mentions = match params
            .edited_mentions
            .iter()
            .cloned()
            .map(serde_json::from_value::<devo_core::Mention>)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(mentions) => mentions,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidMentions,
                    format!("invalid session/message/edit mentions: {error}"),
                );
            }
        };
        if params.edit_mode != devo_protocol::native::rpc_session::MessageEditMode::Normal {
            return self.error_response(
                request_id,
                ProtocolErrorCode::WorkspaceRestoreFailedToStart,
                "session/message/edit queued-only edits are not implemented",
            );
        }
        let Some(session) = session_handle.export_runtime_session().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        if session.active_turn.is_some() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ActiveTurnEditRejected,
                "cannot edit the previous message while a turn is active",
            );
        }

        let expected_target_message_id = params
            .expected_target_message_id
            .or(params.target_message_id);
        let Some(target) = session
            .persisted_turn_items
            .iter()
            .rev()
            .find(|item| crate::persisted_native_item::is_user_message(&item.item))
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::OlderMessageRequiresFork,
                "no immediately previous user message is available to edit",
            );
        };
        if let Some(expected) = expected_target_message_id
            && target.legacy_item_id() != Some(expected)
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ExpectedTargetMessageMismatch,
                "expected target message does not match the current editable message",
            );
        }
        let requested_restore_policy = params
            .workspace_restore_policy
            .unwrap_or(devo_protocol::native::rpc_session::MessageEditWorkspaceRestore::Safe);
        let workspace_restore_policy = core_restore_policy(requested_restore_policy);
        let Some(rollout_path) = session.rollout_path.clone() else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                "session/message/edit requires a durable session",
            );
        };
        let native_session_id = session.summary.native.id;
        let target_message_id = target.item_id;
        let target_turn_id = target.turn_id;
        let legacy_target_message_id = target
            .legacy_item_id()
            .expect("native target message id must bridge");
        let legacy_target_turn_id = target
            .legacy_turn_id()
            .expect("native target turn id must bridge");
        let target_turn_items = session
            .persisted_turn_items
            .iter()
            .filter(|item| item.turn_id == target_turn_id)
            .cloned()
            .collect::<Vec<_>>();
        let sequence = session
            .latest_turn
            .as_ref()
            .map_or(1, |turn| turn.native.sequence + 1);
        let requested_model =
            requested_model_selection(None, None, &session.summary).map(str::to_string);
        let requested_reasoning_effort_selection =
            session.summary.settings.reasoning_effort.clone();
        let runtime_context = Arc::clone(&session.runtime_context);
        let collaboration_mode = {
            let core_session = session.core_session.lock().await;
            core_session.collaboration_mode
        };
        drop(session);

        let turn_config = runtime_context.resolve_turn_config(
            requested_model.as_deref(),
            requested_reasoning_effort_selection.clone(),
        );
        let resolved_request = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());
        let request_model = turn_config.provider_request_model(&resolved_request.request_model);
        let replacement_native_message_id = devo_protocol::native::ids::ItemId::new();
        let replacement_message_id = replacement_native_message_id;
        let records = devo_core::create_edit_records(
            params.session_id,
            legacy_target_message_id,
            Some(legacy_target_turn_id),
            replacement_message_id,
            vec![devo_core::ContentPart::Text(display_input.clone())],
            edited_mentions,
            workspace_restore_policy,
        );
        let [first_record, second_record]: [devo_core::DurableRecord; 2] = match records.try_into()
        {
            Ok(records) => records,
            Err(_) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    "session/message/edit did not create the expected durable records",
                );
            }
        };
        let (
            devo_core::DurableRecord::MessageEditRecorded(mut edit_record),
            devo_core::DurableRecord::TurnSuperseded(mut superseded_record),
        ) = (first_record, second_record)
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                "session/message/edit created unexpected durable records",
            );
        };
        edit_record.requested_by_client_id = params.client_edit_id.clone();
        let restore_plan = if workspace_restore_policy == devo_core::WorkspaceRestorePolicy::Skip {
            None
        } else {
            let restore_candidates =
                discover_restore_candidates(&target_turn_items, target_turn_id);
            let restore_candidate_files = candidate_files(&restore_candidates);
            let (restore_record, restore_id) = devo_core::plan_workspace_restore(
                params.session_id,
                legacy_target_turn_id,
                restore_candidate_files,
                workspace_restore_policy,
            );
            let devo_core::DurableRecord::TurnWorkspaceRestoreStarted(restore_started_record) =
                restore_record
            else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    "session/message/edit created unexpected workspace restore record",
                );
            };
            superseded_record.restore_id = Some(restore_id);
            Some((restore_started_record, restore_candidates))
        };
        let replacement_turn_id = superseded_record.replacement_turn_id;
        let now = Utc::now();
        let replacement_runtime_turn = crate::turn::RuntimeTurn {
            native: devo_protocol::native::turn::Turn {
                // boundary: core DurableRecord UUID → Native TurnId
                id: replacement_turn_id,
                session_id: native_session_id,
                sequence,
                kind: devo_protocol::native::turn::TurnKind::Regular,
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
                request_thinking: resolved_request.request_thinking.clone(),
                stop_reason: None,
                failure_reason: None,
            },
        };
        if let Err(error) = self
            .rollout_store
            .append_message_edit_recorded_at(&rollout_path, edit_record.clone())
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist message edit record: {error}"),
            );
        }
        if let Err(error) = self
            .rollout_store
            .append_turn_superseded_at(&rollout_path, superseded_record.clone())
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist turn superseded record: {error}"),
            );
        }
        let restore_event_payloads = if let Some((restore_started_record, restore_candidates)) =
            restore_plan
        {
            if let Err(error) = self
                .rollout_store
                .append_workspace_restore_started_at(&rollout_path, restore_started_record.clone())
            {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::WorkspaceRestoreFailedToStart,
                    format!("failed to persist workspace restore start: {error}"),
                );
            }
            let restore_outcomes =
                apply_safe_workspace_restore(&workspace_root, &restore_candidates).await;
            let restore_completed_record = devo_core::complete_workspace_restore(
                params.session_id,
                restore_started_record.restore_id,
                restore_outcomes,
            );
            let devo_core::DurableRecord::TurnWorkspaceRestoreCompleted(restore_completed_record) =
                restore_completed_record
            else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    "session/message/edit created unexpected workspace restore completion",
                );
            };
            if let Err(error) = self.rollout_store.append_workspace_restore_completed_at(
                &rollout_path,
                restore_completed_record.clone(),
            ) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist workspace restore completion: {error}"),
                );
            }
            Some((
                restore_started_notification(
                    &restore_started_record,
                    &edit_record.edit_id.0.to_string(),
                ),
                restore_completed_notification(
                    &restore_completed_record,
                    &edit_record.edit_id.0.to_string(),
                ),
            ))
        } else {
            None
        };
        if let Err(error) = self
            .persist_turn_line_deduped(params.session_id, &replacement_runtime_turn)
            .await
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist replacement turn start: {error}"),
            );
        }
        let replacement_item_seq = self
            .allocate_item_sequence(&replacement_runtime_turn.native.session_id)
            .await;
        let replacement_native = crate::runtime::items::native_user_message_item(
            display_input.clone(),
            &resolved_input.image_paths,
            devo_protocol::native::item::UserMessageEntry::TurnStart,
        );
        let replacement_envelope = {
            let history = match self
                .load_canonical_history(&request_id, params.session_id)
                .await
            {
                Ok(history) => history,
                Err(response) => return response,
            };
            let parents = devo_core::resolve_parent_map(&history);
            let branch_parent = parents
                .get(&target_message_id)
                .cloned()
                .flatten()
                .or_else(|| {
                    // Target may be the last user message without edges yet:
                    // parent is previous tree-visible item, or root.
                    let mut visible: Vec<_> = history
                        .items
                        .iter()
                        .filter(|item| devo_core::is_tree_visible_item(&item.item))
                        .collect();
                    visible.sort_by_key(|item| item.seq);
                    let idx = visible
                        .iter()
                        .position(|item| item.id == target_message_id)?;
                    if idx == 0 {
                        None
                    } else {
                        Some(visible[idx - 1].id)
                    }
                });
            let leaf_epoch = history.leaf_epoch.saturating_add(1);
            let mut envelope = devo_protocol::native::wire_projector::typed_item_envelope(
                replacement_runtime_turn.native.session_id,
                replacement_runtime_turn.native.id,
                replacement_native_message_id,
                replacement_item_seq,
                &replacement_native,
                devo_protocol::native::item::ItemState::Completed,
                Utc::now(),
                None,
            );
            envelope.parent_id = branch_parent;
            if let Err(error) = self.rollout_store.append_tree_edge_at(
                &rollout_path,
                params.session_id,
                replacement_native_message_id,
                branch_parent,
            ) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist edit tree edge: {error}"),
                );
            }
            if let Err(error) = self.rollout_store.append_session_leaf_at(
                &rollout_path,
                params.session_id,
                Some(replacement_native_message_id),
                leaf_epoch,
            ) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist edit session leaf: {error}"),
                );
            }
            envelope
        };
        if let Err(error) = self
            .rollout_store
            .append_canonical_item_at(&rollout_path, replacement_envelope)
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist replacement message item: {error}"),
            );
        }

        let Some(mut session) = session_handle.export_runtime_session().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        session
            .persisted_turn_items
            .retain(|item| item.turn_id != target_turn_id);
        let mut rebuilt_messages = Vec::new();
        let mut rebuilt_history_items = Vec::new();
        let mut tool_names_by_id = HashMap::new();
        for item in &session.persisted_turn_items {
            crate::prompt_from_native_item::apply_native_item(
                &mut rebuilt_messages,
                &mut rebuilt_history_items,
                &mut tool_names_by_id,
                item.item.clone(),
            );
        }
        if let Some(history_item) =
            crate::persisted_native_item::history_entry_from_native_item(&replacement_native)
        {
            rebuilt_history_items.push(history_item);
        }
        session.history_items = rebuilt_history_items;
        session
            .persisted_turn_items
            .push(crate::persisted_native_item::PersistedNativeItem::new(
                replacement_runtime_turn.native.id,
                replacement_runtime_turn.native.kind,
                replacement_native_message_id,
                replacement_native,
            ));
        let branch_turn_count = session
            .persisted_turn_items
            .iter()
            .filter(|item| crate::persisted_native_item::is_user_message(&item.item))
            .count()
            .saturating_sub(1);
        session.latest_compaction_snapshot = None;
        session.summary.set_status(SessionStatus::Active);
        session.summary.native.active_turn_id = Some(replacement_runtime_turn.native.id);
        session.summary.sync_activity();
        session.summary.updated_at = now;
        session.summary.last_activity_at = now;
        session.active_turn = Some(replacement_runtime_turn.clone());
        let status_changed = session.summary.status_changed_notification();
        {
            let mut core_session = session.core_session.lock().await;
            core_session.messages = rebuilt_messages;
            core_session.prompt_messages = None;
            core_session.turn_count = branch_turn_count;
            if resolved_input.prompt_messages.is_empty() {
                core_session.push_message(Message::user(resolved_input.prompt_text.clone()));
            } else {
                for prompt_message in &resolved_input.prompt_messages {
                    core_session.push_message(Message::user(prompt_message.clone()));
                }
            }
        }
        session_handle
            .replace_state(
                crate::runtime::session_actor::SessionActorState::from_runtime_session(session),
            )
            .await;

        let runtime = Arc::clone(self);
        let replacement_turn_for_task = replacement_runtime_turn.clone();
        let turn_config_for_task = turn_config.clone();
        let display_input_for_task = display_input.clone();
        let input_for_task = resolved_input.prompt_text.clone();
        let input_messages_for_task = resolved_input.prompt_messages.clone();
        let input_images_for_task = resolved_input.images.clone();
        let input_image_paths_for_task = resolved_input.image_paths.clone();
        let session_id = params.session_id;
        let execute_session_id = session_id;
        let broadcast_session_id = session_id;
        let replacement_goal = {
            let stores = self.goal_stores.lock().await;
            stores
                .get(&session_id)
                .and_then(GoalStore::get)
                .map(Goal::to_thread_goal)
                .unwrap_or(devo_protocol::ThreadGoal {
                    thread_id: session_id,
                    objective: "message edit replacement".to_string(),
                    status: devo_protocol::ThreadGoalStatus::Complete,
                    token_budget: None,
                    tokens_used: 0,
                    time_used_seconds: 0,
                    created_at: now.timestamp(),
                    updated_at: now.timestamp(),
                })
        };
        self.spawn_active_runtime_turn_task(
            session_id,
            replacement_runtime_turn.clone(),
            None,
            async move {
                runtime
                    .execute_turn(ExecuteTurnRequest {
                        session_id: execute_session_id,
                        turn: replacement_turn_for_task,
                        turn_config: turn_config_for_task,
                        display_input: display_input_for_task,
                        client_user_message_id: None,
                        input: input_for_task,
                        input_messages: input_messages_for_task,
                        input_images: input_images_for_task,
                        input_image_paths: input_image_paths_for_task,
                        collaboration_mode,
                        input_mode: TurnInputMode::HiddenGoalContinuation {
                            goal: replacement_goal,
                        },
                        user_message_already_emitted: false,
                    })
                    .await;
            },
        )
        .await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::MessageEditRecorded {
                session_id: broadcast_session_id,
                edit_id: edit_record.edit_id.0.to_string(),
                target_message_id,
                replacement_message_id: replacement_native_message_id,
                edit_state: "accepted".to_string(),
                content_preview: display_input.clone(),
                mentions: params.edited_mentions.clone(),
                timestamp: edit_record.created_at,
            },
        )
        .await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnSuperseded {
                session_id: native_session_id,
                superseded_turn_id: target_turn_id,
                replacement_turn_id: replacement_runtime_turn.native.id,
                edit_id: superseded_record.edit_id.0.to_string(),
                reason: superseded_record.reason.clone(),
            },
        )
        .await;
        if let Some((started, completed)) = restore_event_payloads {
            self.broadcast_notification(started).await;
            self.broadcast_notification(completed).await;
        }
        self.broadcast_notification(status_changed).await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnStarted {
                turn: Box::new(replacement_runtime_turn.native.clone()),
            },
        )
        .await;
        self.emit_native_item_started(
            replacement_runtime_turn.native.session_id,
            replacement_runtime_turn.native.id,
            replacement_native_message_id,
            // The replacement message id is reused as-is; no new item
            // sequence is allocated on this path.
            None,
            crate::runtime::items::native_user_message_item(
                display_input.clone(),
                &resolved_input.image_paths,
                devo_protocol::native::item::UserMessageEntry::TurnStart,
            ),
        )
        .await;
        self.emit_native_item_completed(
            replacement_runtime_turn.native.session_id,
            replacement_runtime_turn.native.id,
            replacement_native_message_id,
            None,
            crate::runtime::items::native_user_message_item(
                display_input,
                &resolved_input.image_paths,
                devo_protocol::native::item::UserMessageEntry::TurnStart,
            ),
        )
        .await;

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: MessageEditResult {
                edit_id: edit_record.edit_id.0.to_string(),
                replacement_message_id,
                replacement_turn_id: Some(replacement_turn_id),
                edit_state: "accepted".to_string(),
            },
        })
        .expect("serialize session/message/edit response")
    }
}
