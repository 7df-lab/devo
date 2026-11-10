use super::*;

impl ServerRuntime {
    /// Current revision of one item in the rollout-backed canonical history.
    pub(super) async fn native_item_revision(
        &self,
        session_id: SessionId,
        item_id: &str,
    ) -> Result<Option<u32>, String> {
        let indexed_path = self
            .deps
            .db
            .get_session_index(&session_id)
            .ok()
            .flatten()
            .and_then(|index| index.rollout_path);
        let Some(rollout_path) = self
            .rollout_store
            .resolve_session_journal(
                &session_id,
                indexed_path.as_deref(),
                crate::persistence::JournalVisibility::Live,
            )
            .map_err(|error| format!("resolve session journal: {error}"))?
        else {
            return Ok(None);
        };
        let history = devo_core::read_canonical_history_for_session(&rollout_path, &session_id)
            .map_err(|error| format!("read session history: {error}"))?;
        Ok(history
            .items
            .iter()
            .find(|item| item.id.as_str() == item_id)
            .map(|item| item.revision))
    }

    /// Interrupts the session's active turn before message edit takes the
    /// state-change gate. Validates the edit target against durable
    /// canonical history first — active-turn user messages live in the turn
    /// working set until MergeTurn, so `export_runtime_session` is not
    /// authoritative mid-turn.
    pub(super) async fn interrupt_active_turn_before_message_edit(
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
        // A later host turn is a real boundary, not something human editing
        // may skip. Reject before interrupting that host's active work.
        let Some(latest_user) = history
            .items
            .iter()
            .rev()
            .find(|item| crate::persisted_native_item::is_turn_input(&item.item))
            .filter(|item| crate::persisted_native_item::is_human_turn_input(&item.item))
        else {
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
            .terminate_session(
                params.session_id,
                crate::runtime::command_exec::SessionCommandOwner::Connection(connection_id),
            )
            .await;
        // Terminal events/status are published before MergeTurn. Editing must
        // wait for that turn's durable state to rejoin the actor and for its
        // admission to be released, without holding state_change_gate. The
        // guarded check in handle_message_edit still rejects any newer turn.
        if tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.active_turns
                .wait_for_turn_release(params.session_id, active_turn_id),
        )
        .await
        .is_err()
        {
            return Err(self.error_response(
                request_id,
                ProtocolErrorCode::ActiveTurnEditRejected,
                "the interrupted turn is still finalizing; retry when it has finished",
            ));
        }
        Ok(())
    }
}
