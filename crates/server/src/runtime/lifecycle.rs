use super::*;

impl ServerRuntime {
    /// Loads durable sessions from rollout files and installs them into the runtime map.
    /// Also restores token stats and pending queues from SQLite.
    pub async fn load_persisted_sessions(self: &Arc<Self>) -> anyhow::Result<()> {
        let sessions = self.rollout_store.load_sessions(&self.deps).await?;
        tracing::info!(session_count = sessions.len(), "loaded persisted sessions");
        let mut restored_goal_stores = std::collections::HashMap::new();

        // Restore token stats and pending queues from SQLite
        for (session_id, session_arc) in &sessions {
            let mut session = session_arc.lock().await;

            if !session.summary.ephemeral
                && let Err(err) = self.deps.db.upsert_session(&session.summary)
            {
                tracing::warn!(
                    session_id = %session_id,
                    error = %err,
                    "failed to seed restored session metadata to database"
                );
                continue;
            }

            match self.deps.db.get_stats(session_id) {
                Ok(Some(stats)) => {
                    session.summary.total_input_tokens = stats.total_input_tokens;
                    session.summary.total_output_tokens = stats.total_output_tokens;
                    session.summary.total_tokens = stats.total_tokens;
                    session.summary.total_cache_creation_tokens = stats.total_cache_creation_tokens;
                    session.summary.total_cache_read_tokens = stats.total_cache_read_tokens;
                    session.summary.prompt_token_estimate = stats.prompt_token_estimate;
                    if let Ok(mut core) = session.core_session.try_lock() {
                        core.total_input_tokens = stats.total_input_tokens;
                        core.total_output_tokens = stats.total_output_tokens;
                        core.total_tokens = stats.total_tokens;
                        core.total_cache_creation_tokens = stats.total_cache_creation_tokens;
                        core.total_cache_read_tokens = stats.total_cache_read_tokens;
                        core.last_input_tokens = stats.last_input_tokens;
                        core.last_turn_tokens = core.last_turn_tokens.max(stats.last_input_tokens);
                        core.prompt_token_estimate = stats.prompt_token_estimate;
                    }
                    tracing::debug!(
                        session_id = %session_id,
                        "restored token stats from database"
                    );
                }
                Ok(None) => {
                    // No stats in database, persist current stats
                    let stats = crate::db::SessionStats {
                        total_input_tokens: session.summary.total_input_tokens,
                        total_output_tokens: session.summary.total_output_tokens,
                        total_tokens: session.summary.total_tokens,
                        total_cache_creation_tokens: session.summary.total_cache_creation_tokens,
                        total_cache_read_tokens: session.summary.total_cache_read_tokens,
                        last_input_tokens: 0,
                        turn_count: 0,
                        prompt_token_estimate: session.summary.prompt_token_estimate,
                    };
                    if let Err(err) = self.deps.db.update_stats(session_id, &stats) {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %err,
                            "failed to persist initial token stats to database"
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %err,
                        "failed to load token stats from database"
                    );
                }
            }

            // Restore pending turn queue from SQLite
            match self
                .deps
                .db
                .drain_pending(session_id, crate::db::QueueType::Turn)
            {
                Ok(items) => {
                    if !items.is_empty() {
                        let core_session = session.core_session.lock().await;
                        let mut queue = core_session
                            .pending_turn_queue
                            .lock()
                            .expect("pending turn queue mutex should not be poisoned");
                        queue.extend(items);
                        tracing::debug!(
                            session_id = %session_id,
                            pending_count = queue.len(),
                            "restored pending turn queue from database"
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %err,
                        "failed to load pending turn queue from database"
                    );
                }
            }

            // Clear any stale btw inputs from previous session
            if let Err(err) = self
                .deps
                .db
                .clear_pending(session_id, crate::db::QueueType::Btw)
            {
                tracing::warn!(
                    session_id = %session_id,
                    error = %err,
                    "failed to clear stale btw inputs from database"
                );
            }

            drop(session);
            match self.goal_durable_store.replay_goal_store(*session_id).await {
                Ok(Some(mut goal_store)) => {
                    if let Some(goal) = goal_store.get()
                        && goal.status == crate::goal::GoalStatus::Active
                    {
                        let previous_status = goal.status;
                        match goal_store.set_status(devo_protocol::ThreadGoalStatus::Paused) {
                            Ok(paused_goal) => {
                                if let Err(error) = self
                                    .goal_durable_store
                                    .append_status_changed(
                                        &paused_goal,
                                        previous_status,
                                        Some(
                                            "Goal paused because the session was restored without explicit resume."
                                                .to_string(),
                                        ),
                                    )
                                    .await
                                {
                                    tracing::warn!(
                                        session_id = %session_id,
                                        error = %error,
                                        "failed to persist restored goal pause record"
                                    );
                                }
                            }
                            Err(error) => {
                                tracing::warn!(
                                    session_id = %session_id,
                                    error = %error,
                                    "failed to pause restored active goal"
                                );
                            }
                        }
                    }
                    restored_goal_stores.insert(*session_id, goal_store);
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %error,
                        "failed to replay durable goal records"
                    );
                }
            }
        }

        let mut runtime_sessions = self.sessions.lock().await;
        runtime_sessions.extend(sessions);
        drop(runtime_sessions);
        self.goal_stores.lock().await.extend(restored_goal_stores);
        Ok(())
    }

    /// Completes deferred (in-progress) items for all active turns and
    /// persists interrupted turn records. Called on graceful shutdown.
    pub async fn shutdown(self: &Arc<Self>) {
        self.command_exec_manager.terminate_all().await;
        let session_ids: Vec<SessionId> = {
            let sessions = self.sessions.lock().await;
            sessions.keys().cloned().collect()
        };

        for session_id in session_ids {
            let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
                continue;
            };

            self.run_session_hook(
                session_id,
                devo_core::HookEvent::SessionEnd,
                serde_json::Map::from_iter([("reason".to_string(), serde_json::json!("other"))]),
            )
            .await;

            let (deferred_assistant, deferred_reasoning, turn_id, record) = {
                let mut session = session_arc.lock().await;
                let turn_id = session.active_turn.as_ref().map(|t| t.turn_id);
                (
                    session.deferred_assistant.take(),
                    session.deferred_reasoning.take(),
                    turn_id,
                    session.record.clone(),
                )
            };

            let Some(turn_id) = turn_id else {
                continue;
            };

            // Complete deferred items before shutting down
            if let Some((item_id, item_seq, text)) = deferred_assistant {
                self.complete_item(
                    session_id,
                    turn_id,
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
                    session_id,
                    turn_id,
                    item_id,
                    item_seq,
                    ItemKind::Reasoning,
                    TurnItem::Reasoning(TextItem { text: text.clone() }),
                    serde_json::json!({ "title": "Reasoning", "text": text }),
                )
                .await;
            }

            // Mark turn as interrupted
            let interrupted_turn = {
                let mut session = session_arc.lock().await;
                let Some(mut turn) = session.active_turn.take() else {
                    continue;
                };
                if turn.turn_id != turn_id {
                    session.active_turn = Some(turn);
                    continue;
                }
                turn.status = TurnStatus::Interrupted;
                turn.completed_at = Some(Utc::now());
                session.latest_turn = Some(turn.clone());
                session.summary.status = SessionRuntimeStatus::Idle;
                session.summary.updated_at = Utc::now();
                let token_totals = session.core_session.try_lock().ok().map(|core| {
                    (
                        core.total_input_tokens,
                        core.total_output_tokens,
                        core.total_tokens,
                    )
                });
                if let Some((input, output, total)) = token_totals {
                    session.summary.total_input_tokens = input;
                    session.summary.total_output_tokens = output;
                    session.summary.total_tokens = total;
                }
                turn
            };

            // Persist interrupted turn record
            if let Some(record) = record {
                let (session_context, turn_context) = {
                    let session = session_arc.lock().await;
                    let core = session.core_session.lock().await;
                    (
                        core.session_context.clone(),
                        core.latest_turn_context.clone(),
                    )
                };
                if let Err(error) = self.rollout_store.append_turn(
                    &record,
                    build_turn_record(&interrupted_turn, session_context, turn_context),
                ) {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %error,
                        "failed to persist interrupted turn on shutdown"
                    );
                }
            }

            tracing::info!(
                session_id = %session_id,
                turn_id = %interrupted_turn.turn_id,
                "completed deferred items and interrupted turn on shutdown"
            );
        }
    }
}
