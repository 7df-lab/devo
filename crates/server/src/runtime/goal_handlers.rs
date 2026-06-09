use super::*;

impl ServerRuntime {
    // ── Goal Handlers ─────────────────────────────────────────────────

    pub(super) async fn handle_goal_create(
        &self,
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

        let mut stores = self.goal_stores.lock().await;
        let store = stores
            .entry(params.session_id)
            .or_insert_with(GoalStore::new);
        match store.create(params) {
            Ok(goal) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::GoalCreateResult {
                    goal: goal.to_thread_goal(),
                },
            })
            .expect("serialize goal create result"),
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal creation failed: {e}"),
            ),
        }
    }

    pub(super) async fn handle_goal_set(
        &self,
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

        let mut stores = self.goal_stores.lock().await;
        let store = stores
            .entry(params.session_id)
            .or_insert_with(GoalStore::new);
        match store.set(params) {
            Ok(goal) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::GoalSetResult {
                    goal: goal.to_thread_goal(),
                },
            })
            .expect("serialize goal set result"),
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
        match store.set_status(devo_protocol::ThreadGoalStatus::Paused) {
            Ok(goal) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::GoalSetStatusResult {
                    goal: goal.to_thread_goal(),
                },
            })
            .expect("serialize goal pause result"),
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal pause failed: {e}"),
            ),
        }
    }

    pub(super) async fn handle_goal_resume(
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
                    format!("invalid goal/resume params: {e}"),
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
        match store.set_status(devo_protocol::ThreadGoalStatus::Active) {
            Ok(goal) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::GoalSetStatusResult {
                    goal: goal.to_thread_goal(),
                },
            })
            .expect("serialize goal resume result"),
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
        match store.set_status(devo_protocol::ThreadGoalStatus::Complete) {
            Ok(goal) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::GoalSetStatusResult {
                    goal: goal.to_thread_goal(),
                },
            })
            .expect("serialize goal complete result"),
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
        match store.mutate(GoalMutation {
            goal_id: GoalId(params.goal_id),
            action: GoalAction::Cancel,
        }) {
            Ok(goal) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::GoalSetStatusResult {
                    goal: goal.to_thread_goal(),
                },
            })
            .expect("serialize goal cancel result"),
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
        let cleared = stores
            .get_mut(&params.session_id)
            .is_some_and(GoalStore::clear);

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
}
