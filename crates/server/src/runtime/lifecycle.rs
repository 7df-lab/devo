use super::*;

impl ServerRuntime {
    /// Loads durable sessions from rollout files and installs them into the runtime map.
    /// Also restores token stats and pending queues from SQLite.
    pub async fn load_persisted_sessions(self: &Arc<Self>) -> anyhow::Result<()> {
        let mut sessions = self.rollout_store.load_sessions(&self.deps).await?;
        tracing::info!(session_count = sessions.len(), "loaded persisted sessions");
        let mut restored_goal_stores = std::collections::HashMap::new();

        for (session_id, runtime_session) in sessions.iter_mut() {
            if !runtime_session.summary.ephemeral
                && let Err(err) = self.deps.db.upsert_session(&runtime_session.summary)
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
                    runtime_session.summary.total_input_tokens = stats.total_input_tokens;
                    runtime_session.summary.total_output_tokens = stats.total_output_tokens;
                    runtime_session.summary.total_tokens = stats.total_tokens;
                    runtime_session.summary.total_cache_creation_tokens =
                        stats.total_cache_creation_tokens;
                    runtime_session.summary.total_cache_read_tokens = stats.total_cache_read_tokens;
                    runtime_session.summary.prompt_token_estimate = stats.prompt_token_estimate;
                    if let Ok(mut core) = runtime_session.core_session.try_lock() {
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
                    let stats = crate::db::SessionStats {
                        total_input_tokens: runtime_session.summary.total_input_tokens,
                        total_output_tokens: runtime_session.summary.total_output_tokens,
                        total_tokens: runtime_session.summary.total_tokens,
                        total_cache_creation_tokens: runtime_session
                            .summary
                            .total_cache_creation_tokens,
                        total_cache_read_tokens: runtime_session.summary.total_cache_read_tokens,
                        last_input_tokens: 0,
                        turn_count: 0,
                        prompt_token_estimate: runtime_session.summary.prompt_token_estimate,
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

            match self
                .deps
                .db
                .drain_pending(session_id, crate::db::QueueType::Turn)
            {
                Ok(items) => {
                    if !items.is_empty() {
                        let core_session = runtime_session.core_session.lock().await;
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

        for (_, runtime_session) in sessions {
            self.insert_session_actor(SessionActorState::from_runtime_session(runtime_session))
                .await;
        }
        self.goal_stores.lock().await.extend(restored_goal_stores);
        Ok(())
    }

    /// Completes deferred (in-progress) items for all active turns and
    /// persists interrupted turn records. Called on graceful shutdown.
    pub async fn shutdown(self: &Arc<Self>) {
        self.command_exec_manager.terminate_all().await;
        let session_handles = self.list_session_handles().await;

        for session_handle in session_handles {
            let session_id = session_handle.id();

            self.run_session_hook(
                session_id,
                devo_core::HookEvent::SessionEnd,
                serde_json::Map::from_iter([("reason".to_string(), serde_json::json!("other"))]),
            )
            .await;

            let Some(snapshot) = session_handle.take_shutdown_deferred_snapshot().await else {
                continue;
            };
            let Some(turn_id) = snapshot.active_turn_id else {
                continue;
            };

            if let Some((item_id, item_seq, text)) = snapshot.deferred_assistant {
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
            if let Some((item_id, item_seq, text)) = snapshot.deferred_reasoning {
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

            let Some(interrupted_turn) = session_handle.interrupt_active_turn().await.flatten()
            else {
                continue;
            };
            if interrupted_turn.turn_id != turn_id {
                continue;
            }

            if let Some(record) = snapshot.record
                && let Some(persistence) = session_handle.turn_persistence_snapshot().await
                && let Err(error) = self.rollout_store.append_turn(
                    &record,
                    build_turn_record(
                        &interrupted_turn,
                        persistence.session_context,
                        persistence.latest_turn_context,
                    ),
                )
            {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "failed to persist interrupted turn on shutdown"
                );
            }

            tracing::info!(
                session_id = %session_id,
                turn_id = %interrupted_turn.turn_id,
                "completed deferred items and interrupted turn on shutdown"
            );
        }

        let session_ids: Vec<SessionId> = self
            .list_session_handles()
            .await
            .into_iter()
            .map(|handle| handle.id())
            .collect();
        for session_id in session_ids {
            if let Some(handle) = self.sessions.lock().await.get(&session_id).cloned() {
                handle.shutdown().await;
            }
            self.remove_session_actor(session_id).await;
        }
    }
}
