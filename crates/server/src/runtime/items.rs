use super::*;

impl ServerRuntime {
    pub(super) async fn maybe_assign_provisional_title(
        &self,
        session_id: SessionId,
        first_user_input: &str,
    ) {
        let Some(candidate) = derive_provisional_title(first_user_input) else {
            return;
        };
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return;
        };

        let updated_summary = {
            let mut session = session_arc.lock().await;
            if session.summary.title.is_some()
                || !matches!(session.summary.title_state, SessionTitleState::Unset)
            {
                return;
            }

            let previous_title = session.summary.title.clone();
            let updated_at = Utc::now();
            session.summary.title = Some(candidate.clone());
            session.summary.title_state = SessionTitleState::Provisional;
            session.summary.updated_at = updated_at;

            if let Some(record) = session.record.as_mut() {
                record.title = Some(candidate.clone());
                record.title_state = SessionTitleState::Provisional;
                record.updated_at = updated_at;
                if let Err(error) = self.rollout_store.append_title_update(
                    record,
                    candidate.clone(),
                    SessionTitleState::Provisional,
                    previous_title,
                ) {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist provisional title");
                }
            }
            session.summary.clone()
        };

        self.broadcast_event(ServerEvent::SessionTitleUpdated(SessionEventPayload {
            session: updated_summary,
        }))
        .await;
    }

    /// Attempts to generate a final session title by calling the LLM.
    /// Retries up to MAX_TITLE_RETRIES times with exponential backoff.
    /// Exhausting retries leaves the title at `Provisional`; the caller
    /// should re-trigger on the next user message.
    const MAX_TITLE_RETRIES: usize = 5;
    const TITLE_RETRY_BASE_DELAY_SECS: u64 = 1;

    pub(super) async fn maybe_generate_final_title(
        self: Arc<Self>,
        session_id: SessionId,
        first_user_input: String,
    ) {
        for attempt in 1..=Self::MAX_TITLE_RETRIES {
            let (model_selection, thinking, should_skip) = {
                let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
                    return;
                };
                let session = session_arc.lock().await;
                (
                    session_model_selection(&session.summary)
                        .map(str::to_string)
                        .unwrap_or_else(|| self.deps.default_model.clone()),
                    session.summary.thinking.clone(),
                    matches!(session.summary.title_state, SessionTitleState::Final(_)),
                )
            };

            if should_skip {
                return;
            }

            let turn_config = self
                .deps
                .resolve_turn_config(Some(model_selection.as_str()), thinking);
            let resolved_request = turn_config
                .model
                .resolve_thinking_selection(turn_config.thinking_selection.as_deref());
            let request_model = turn_config.provider_request_model(&resolved_request.request_model);

            let response = match self
                .deps
                .provider_router
                .complete(
                    turn_config.provider_route.clone(),
                    build_title_generation_request(request_model.clone(), &first_user_input),
                )
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    tracing::warn!(
                        session_id = %session_id,
                        attempt,
                        model = %turn_config.model.slug,
                        request_model = %request_model,
                        error = %error,
                        "title gen failed"
                    );
                    if attempt < Self::MAX_TITLE_RETRIES {
                        let delay = Self::TITLE_RETRY_BASE_DELAY_SECS * (1u64 << (attempt - 1));
                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                    }
                    continue;
                }
            };

            let generated_title = match normalize_generated_title(&response.content) {
                Ok(title) => title,
                Err(error) => {
                    tracing::warn!(
                        session_id = %session_id,
                        attempt,
                        model = %turn_config.model.slug,
                        request_model = %request_model,
                        response_id = %response.id,
                        content_blocks = response.content.len(),
                        title_error = error.as_str(),
                        "title gen returned no valid title"
                    );
                    if attempt < Self::MAX_TITLE_RETRIES {
                        let delay = Self::TITLE_RETRY_BASE_DELAY_SECS * (1u64 << (attempt - 1));
                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                    }
                    continue;
                }
            };

            let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
                return;
            };
            let updated_summary = {
                let mut session = session_arc.lock().await;
                if matches!(session.summary.title_state, SessionTitleState::Final(_)) {
                    return;
                }

                let previous_title = session.summary.title.clone();
                let updated_at = Utc::now();
                session.summary.title = Some(generated_title.clone());
                session.summary.title_state =
                    SessionTitleState::Final(SessionTitleFinalSource::ModelGenerated);
                session.summary.updated_at = updated_at;

                if let Some(record) = session.record.as_mut() {
                    record.title = Some(generated_title.clone());
                    record.title_state =
                        SessionTitleState::Final(SessionTitleFinalSource::ModelGenerated);
                    record.updated_at = updated_at;
                    if let Err(error) = self.rollout_store.append_title_update(
                        record,
                        generated_title.clone(),
                        record.title_state.clone(),
                        previous_title,
                    ) {
                        tracing::warn!(session_id = %session_id, error = %error, "failed to persist title");
                    }
                }
                session.summary.clone()
            };

            self.broadcast_event(ServerEvent::SessionTitleUpdated(SessionEventPayload {
                session: updated_summary,
            }))
            .await;
            return;
        }
        tracing::warn!(session_id = %session_id, "title generation exhausted all retries");
    }

    pub(super) async fn emit_turn_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_kind: ItemKind,
        turn_item: TurnItem,
        payload: serde_json::Value,
    ) {
        let (item_id, item_seq) = self
            .start_item(session_id, turn_id, item_kind.clone(), payload.clone())
            .await;
        self.complete_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            item_kind.clone(),
            turn_item,
            payload.clone(),
        )
        .await;
    }

    pub(super) async fn start_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_kind: ItemKind,
        payload: serde_json::Value,
    ) -> (ItemId, u64) {
        let item_id = ItemId::new();
        let item_seq = self.allocate_item_sequence(session_id).await;
        self.emit_item_started(session_id, turn_id, item_id, item_kind, payload)
            .await;
        (item_id, item_seq)
    }

    pub(super) async fn emit_item_started(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_id: ItemId,
        item_kind: ItemKind,
        payload: serde_json::Value,
    ) {
        self.broadcast_event(ServerEvent::ItemStarted(ItemEventPayload {
            context: EventContext {
                session_id,
                turn_id: Some(turn_id),
                item_id: Some(item_id),
                seq: 0,
            },
            item: ItemEnvelope {
                item_id,
                item_kind,
                payload,
            },
        }))
        .await;
    }

    pub(super) async fn emit_item_completed(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_id: ItemId,
        item_kind: ItemKind,
        payload: serde_json::Value,
    ) {
        self.broadcast_event(ServerEvent::ItemCompleted(ItemEventPayload {
            context: EventContext {
                session_id,
                turn_id: Some(turn_id),
                item_id: Some(item_id),
                seq: 0,
            },
            item: ItemEnvelope {
                item_id,
                item_kind,
                payload,
            },
        }))
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn complete_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_id: ItemId,
        item_seq: u64,
        item_kind: ItemKind,
        turn_item: TurnItem,
        payload: serde_json::Value,
    ) {
        self.persist_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            turn_item,
            Some(TurnStatus::Running),
            None,
        )
        .await;
        self.emit_item_completed(session_id, turn_id, item_id, item_kind, payload)
            .await;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn persist_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_id: ItemId,
        item_seq: u64,
        turn_item: TurnItem,
        turn_status: Option<TurnStatus>,
        worklog: Option<Worklog>,
    ) {
        if let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() {
            let record = {
                let mut session = session_arc.lock().await;
                if let Some(history_item) = history_item_from_turn_item(&turn_item) {
                    session.history_items.push(history_item);
                }
                session
                    .persisted_turn_items
                    .push(crate::execution::PersistedTurnItem {
                        turn_id,
                        item_id,
                        turn_item: turn_item.clone(),
                    });
                session.record.clone()
            };
            if let Some(record) = record {
                let item = build_item_record(
                    session_id,
                    turn_id,
                    item_id,
                    item_seq,
                    turn_item,
                    turn_status,
                    worklog,
                );
                if let Err(error) = self.rollout_store.append_item(&record, item) {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist item line");
                }
            }
        }
    }

    pub(super) async fn allocate_item_sequence(&self, session_id: SessionId) -> u64 {
        if let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() {
            let mut session = session_arc.lock().await;
            let item_seq = session.next_item_seq;
            session.loaded_item_count += 1;
            session.next_item_seq += 1;
            return item_seq;
        }
        1
    }
}

pub(crate) fn render_input_items(input: &[crate::InputItem]) -> Option<String> {
    let parts = input
        .iter()
        .map(|item| match item {
            crate::InputItem::Text { text } => text.trim().to_string(),
            crate::InputItem::Skill { name, path } => {
                format!("[skill:{name} @ {}]", path.display())
            }
            crate::InputItem::LocalImage { path } => format!("[image:{}]", path.display()),
            crate::InputItem::Mention { path, name } => {
                format!("[mention:{}]", name.as_deref().unwrap_or(path.as_str()))
            }
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join("\n"))
}
