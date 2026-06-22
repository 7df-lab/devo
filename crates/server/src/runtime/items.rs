use std::borrow::Cow;

use crate::titles::build_title_generation_request;
use crate::titles::derive_provisional_title;
use crate::titles::normalize_generated_title;

use super::*;

impl ServerRuntime {
    pub(super) async fn maybe_start_title_generation_from_user_input(
        self: &Arc<Self>,
        session_id: SessionId,
        user_input: &str,
    ) {
        self.maybe_assign_provisional_title(session_id, user_input)
            .await;

        let needs_title = {
            let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
                return;
            };
            let mut session = session_arc.lock().await;
            if session.first_user_input.is_none() {
                session.first_user_input = Some(user_input.to_string());
            }
            let first_input = session.first_user_input.clone();
            let needs = matches!(
                session.summary.title_state,
                SessionTitleState::Unset | SessionTitleState::Provisional
            );
            (needs, first_input)
        };
        if needs_title.0
            && let Some(first_input) = needs_title.1
        {
            let runtime = Arc::clone(self);
            tokio::spawn(async move {
                runtime
                    .maybe_generate_final_title(session_id, first_input)
                    .await;
            });
        }
    }

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
            let (model_selection, reasoning_effort_selection, should_skip, runtime_context) = {
                let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
                    return;
                };
                let session = session_arc.lock().await;
                (
                    session_model_selection(&session.summary)
                        .map(str::to_string)
                        .unwrap_or_else(|| session.runtime_context.default_model.clone()),
                    session.summary.reasoning_effort_selection.clone(),
                    matches!(session.summary.title_state, SessionTitleState::Final(_)),
                    Arc::clone(&session.runtime_context),
                )
            };

            if should_skip {
                return;
            }

            let turn_config = runtime_context
                .resolve_turn_config(Some(model_selection.as_str()), reasoning_effort_selection);
            let resolved_request = turn_config.model.resolve_reasoning_effort_selection(
                turn_config.reasoning_effort_selection.as_deref(),
            );
            let request_model = turn_config.provider_request_model(&resolved_request.request_model);

            let response = match runtime_context
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
        if !should_emit_turn_item_events(&turn_item) {
            let item_id = ItemId::new();
            let item_seq = self.allocate_item_sequence(session_id).await;
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
            return;
        }

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
                let turn_kind = session
                    .active_turn
                    .as_ref()
                    .filter(|turn| turn.turn_id == turn_id)
                    .map(|turn| turn.kind.clone())
                    .or_else(|| {
                        session
                            .latest_turn
                            .as_ref()
                            .filter(|turn| turn.turn_id == turn_id)
                            .map(|turn| turn.kind.clone())
                    })
                    .unwrap_or_default();
                session
                    .persisted_turn_items
                    .push(crate::execution::PersistedTurnItem {
                        turn_id,
                        turn_kind,
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
    let mut rendered = String::new();
    for item in input {
        let part = match item {
            crate::InputItem::Text { text } => {
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                Cow::Borrowed(text)
            }
            crate::InputItem::Skill { name, path } => {
                Cow::Owned(format!("[skill:{name} @ {}]", path.display()))
            }
            crate::InputItem::LocalImage { path } => {
                Cow::Owned(format!("[image:{}]", path.display()))
            }
            crate::InputItem::Mention { path, name } => Cow::Owned(format!(
                "[mention:{}]",
                name.as_deref().unwrap_or(path.as_str())
            )),
        };
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str(&part);
    }
    (!rendered.is_empty()).then_some(rendered)
}

fn should_emit_turn_item_events(turn_item: &TurnItem) -> bool {
    !matches!(
        turn_item,
        TurnItem::ResearchArtifact(ResearchArtifactItem {
            artifact_type: ResearchArtifactType::FinalReportMetadata,
            ..
        })
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::InputItem;

    #[test]
    fn render_input_items_trims_text_and_preserves_item_markers() {
        let input = vec![
            InputItem::Text {
                text: "  hello  ".to_string(),
            },
            InputItem::Text {
                text: "   ".to_string(),
            },
            InputItem::Skill {
                name: "writer".to_string(),
                path: PathBuf::from("writer.md"),
            },
            InputItem::LocalImage {
                path: PathBuf::from("photo.png"),
            },
            InputItem::Mention {
                path: "src/lib.rs".to_string(),
                name: None,
            },
            InputItem::Mention {
                path: "src/main.rs".to_string(),
                name: Some("main".to_string()),
            },
        ];

        assert_eq!(
            render_input_items(&input),
            Some(
                "hello\n[skill:writer @ writer.md]\n[image:photo.png]\n[mention:src/lib.rs]\n[mention:main]"
                    .to_string()
            )
        );
    }

    #[test]
    fn render_input_items_returns_none_for_empty_text_only_input() {
        assert_eq!(
            render_input_items(&[InputItem::Text {
                text: " \n\t ".to_string(),
            }]),
            None
        );
    }

    #[test]
    fn final_report_metadata_is_prompt_only() {
        let prompt_only = TurnItem::ResearchArtifact(ResearchArtifactItem {
            artifact_type: ResearchArtifactType::FinalReportMetadata,
            title: "Research Context Reference".to_string(),
            content: "compact reference".to_string(),
        });
        let visible_artifact = TurnItem::ResearchArtifact(ResearchArtifactItem {
            artifact_type: ResearchArtifactType::Finding,
            title: "Research Finding".to_string(),
            content: "finding body".to_string(),
        });

        assert_eq!(
            vec![
                should_emit_turn_item_events(&prompt_only),
                should_emit_turn_item_events(&visible_artifact),
            ],
            vec![false, true]
        );
    }
}
