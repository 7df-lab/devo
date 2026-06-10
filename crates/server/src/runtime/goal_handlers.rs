use super::*;

impl ServerRuntime {
    // ── Goal Handlers ─────────────────────────────────────────────────

    pub(super) async fn handle_goal_create(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalCreateParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/create params: {e}"),
                );
            }
        };
        let session_id = params.session_id;

        let mut stores = self.goal_stores.lock().await;
        let store = stores.entry(session_id).or_insert_with(GoalStore::new);
        match store.create(params) {
            Ok(goal) => {
                let should_continue = goal.status == crate::goal::GoalStatus::Active;
                let thread_goal = goal.to_thread_goal();
                let session_goal = should_continue.then(|| thread_goal.clone());
                let durable_goal = goal.clone();
                let result = serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: devo_protocol::GoalCreateResult { goal: thread_goal },
                })
                .expect("serialize goal create result");
                drop(stores);
                if let Err(error) = self
                    .goal_durable_store
                    .append_goal_created(&durable_goal)
                    .await
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal create record");
                }
                self.sync_core_session_goal(session_id, session_goal).await;
                if should_continue {
                    self.maybe_start_goal_continuation_turn(session_id).await;
                }
                result
            }
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal creation failed: {e}"),
            ),
        }
    }

    pub(super) async fn handle_goal_set(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalSetParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/set params: {e}"),
                );
            }
        };
        let session_id = params.session_id;

        let mut stores = self.goal_stores.lock().await;
        let store = stores.entry(session_id).or_insert_with(GoalStore::new);
        let previous_status = store.get().map(|goal| goal.status);
        match store.set(params) {
            Ok(goal) => {
                let should_continue = goal.status == crate::goal::GoalStatus::Active;
                let thread_goal = goal.to_thread_goal();
                let session_goal = should_continue.then(|| thread_goal.clone());
                let durable_goal = goal.clone();
                let result = serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: devo_protocol::GoalSetResult { goal: thread_goal },
                })
                .expect("serialize goal set result");
                drop(stores);
                if let Err(error) = self
                    .goal_durable_store
                    .append_goal_created(&durable_goal)
                    .await
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal set record");
                }
                let status_record_base = previous_status.unwrap_or(crate::goal::GoalStatus::Active);
                if status_record_base != durable_goal.status
                    && let Err(error) = self
                        .goal_durable_store
                        .append_status_changed(&durable_goal, status_record_base, None)
                        .await
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal status record");
                }
                self.sync_core_session_goal(session_id, session_goal).await;
                if should_continue {
                    self.maybe_start_goal_continuation_turn(session_id).await;
                }
                result
            }
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal set failed: {e}"),
            ),
        }
    }

    #[allow(dead_code)]
    pub(super) async fn handle_goal_pause(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalSetStatusParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/pause params: {e}"),
                );
            }
        };

        let mut stores = self.goal_stores.lock().await;
        let Some(store) = stores.get_mut(&params.session_id) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "no goal store for session",
            );
        };
        let previous_status = store.get().map(|goal| goal.status);
        match store.set_status(devo_protocol::ThreadGoalStatus::Paused) {
            Ok(goal) => {
                let thread_goal = goal.to_thread_goal();
                let durable_goal = goal.clone();
                let result = serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: devo_protocol::GoalSetStatusResult { goal: thread_goal },
                })
                .expect("serialize goal pause result");
                let session_id = params.session_id;
                drop(stores);
                if let Some(previous_status) = previous_status
                    && let Err(error) = self
                        .goal_durable_store
                        .append_status_changed(&durable_goal, previous_status, None)
                        .await
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal pause record");
                }
                self.sync_core_session_goal(session_id, None).await;
                result
            }
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal pause failed: {e}"),
            ),
        }
    }

    pub(super) async fn handle_goal_resume(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalSetStatusParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/resume params: {e}"),
                );
            }
        };
        let session_id = params.session_id;

        let mut stores = self.goal_stores.lock().await;
        let Some(store) = stores.get_mut(&session_id) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "no goal store for session",
            );
        };
        let previous_status = store.get().map(|goal| goal.status);
        match store.set_status(devo_protocol::ThreadGoalStatus::Active) {
            Ok(goal) => {
                let should_continue = goal.status == crate::goal::GoalStatus::Active;
                let thread_goal = goal.to_thread_goal();
                let session_goal = should_continue.then(|| thread_goal.clone());
                let durable_goal = goal.clone();
                let result = serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: devo_protocol::GoalSetStatusResult { goal: thread_goal },
                })
                .expect("serialize goal resume result");
                drop(stores);
                if let Some(previous_status) = previous_status
                    && let Err(error) = self
                        .goal_durable_store
                        .append_status_changed(&durable_goal, previous_status, None)
                        .await
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal resume record");
                }
                self.sync_core_session_goal(session_id, session_goal).await;
                if should_continue {
                    self.maybe_start_goal_continuation_turn(session_id).await;
                }
                result
            }
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal resume failed: {e}"),
            ),
        }
    }

    #[allow(dead_code)]
    pub(super) async fn handle_goal_complete(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalSetStatusParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/complete params: {e}"),
                );
            }
        };

        let mut stores = self.goal_stores.lock().await;
        let Some(store) = stores.get_mut(&params.session_id) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "no goal store for session",
            );
        };
        let previous_status = store.get().map(|goal| goal.status);
        match store.set_status(devo_protocol::ThreadGoalStatus::Complete) {
            Ok(goal) => {
                let thread_goal = goal.to_thread_goal();
                let durable_goal = goal.clone();
                let result = serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: devo_protocol::GoalSetStatusResult { goal: thread_goal },
                })
                .expect("serialize goal complete result");
                let session_id = params.session_id;
                drop(stores);
                if let Some(previous_status) = previous_status
                    && let Err(error) = self
                        .goal_durable_store
                        .append_status_changed(&durable_goal, previous_status, None)
                        .await
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal complete record");
                }
                self.sync_core_session_goal(session_id, None).await;
                result
            }
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal complete failed: {e}"),
            ),
        }
    }

    pub(super) async fn handle_goal_cancel(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: crate::runtime::handlers::goal::GoalCancelParams =
            match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid goal/cancel params: {e}"),
                    );
                }
            };

        let mut stores = self.goal_stores.lock().await;
        let Some(store) = stores.get_mut(&params.session_id) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "no goal store for session",
            );
        };
        let previous_status = store.get().map(|goal| goal.status);
        match store.mutate(GoalMutation {
            goal_id: GoalId(params.goal_id),
            action: GoalAction::Cancel,
        }) {
            Ok(goal) => {
                let thread_goal = goal.to_thread_goal();
                let durable_goal = goal.clone();
                let result = serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: devo_protocol::GoalSetStatusResult { goal: thread_goal },
                })
                .expect("serialize goal cancel result");
                let session_id = params.session_id;
                drop(stores);
                if let Some(previous_status) = previous_status
                    && let Err(error) = self
                        .goal_durable_store
                        .append_status_changed(&durable_goal, previous_status, None)
                        .await
                {
                    tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal cancel record");
                }
                self.sync_core_session_goal(session_id, None).await;
                result
            }
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal cancel failed: {e}"),
            ),
        }
    }

    #[allow(dead_code)]
    pub(super) async fn handle_goal_clear(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalClearParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/clear params: {e}"),
                );
            }
        };

        let mut stores = self.goal_stores.lock().await;
        let cleared_goal_id = stores
            .get(&params.session_id)
            .and_then(GoalStore::get)
            .map(|goal| goal.durable_goal_id);
        let cleared = stores
            .get_mut(&params.session_id)
            .is_some_and(GoalStore::clear);
        drop(stores);
        if cleared {
            if let Some(goal_id) = cleared_goal_id
                && let Err(error) = self
                    .goal_durable_store
                    .append_goal_cleared(params.session_id, goal_id, Some("user clear".to_string()))
                    .await
            {
                tracing::warn!(session_id = %params.session_id, error = %error, "failed to persist goal clear record");
            }
            self.sync_core_session_goal(params.session_id, None).await;
        }

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::GoalClearResult { cleared },
        })
        .expect("serialize goal clear result")
    }

    pub(super) async fn handle_goal_status(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::GoalStatusParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid goal/status params: {e}"),
                );
            }
        };

        let stores = self.goal_stores.lock().await;
        let goal_store: Option<&GoalStore> = stores.get(&params.session_id);
        let projection = goal_store
            .and_then(|store| store.get())
            .map(Goal::to_thread_goal);

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::GoalStatusResult { goal: projection },
        })
        .expect("serialize goal status result")
    }

    pub(super) async fn sync_core_session_goal(
        &self,
        session_id: SessionId,
        goal: Option<devo_protocol::ThreadGoal>,
    ) {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return;
        };
        let core_session = {
            let session = session_arc.lock().await;
            Arc::clone(&session.core_session)
        };
        if let Ok(mut core_session) = core_session.try_lock() {
            if let Some(goal) = goal {
                core_session.set_active_goal(goal);
            } else {
                core_session.clear_active_goal();
            }
        }
    }
}
