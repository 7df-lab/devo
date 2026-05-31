use super::*;

impl ServerRuntime {
    // ── Goal Handlers ─────────────────────────────────────────────────

    pub(super) async fn handle_goal_create(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: CreateGoalParams = match serde_json::from_value(params) {
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
                result: serde_json::json!({
                    "goal_id": goal.goal_id.0,
                    "status": format!("{:?}", goal.status).to_lowercase(),
                }),
            })
            .expect("serialize goal create result"),
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal creation failed: {e}"),
            ),
        }
    }

    #[allow(dead_code)]
    pub(super) async fn handle_goal_pause(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: crate::runtime::handlers::goal::GoalPauseParams =
            match serde_json::from_value(params) {
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
        match store.mutate(GoalMutation {
            goal_id: GoalId(params.goal_id),
            action: GoalAction::Pause,
        }) {
            Ok(goal) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: serde_json::json!({
                    "goal_id": goal.goal_id.0,
                    "status": format!("{:?}", goal.status).to_lowercase(),
                }),
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
        let params: crate::runtime::handlers::goal::GoalResumeParams =
            match serde_json::from_value(params) {
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
        match store.mutate(GoalMutation {
            goal_id: GoalId(params.goal_id),
            action: GoalAction::Resume,
        }) {
            Ok(goal) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: serde_json::json!({
                    "goal_id": goal.goal_id.0,
                    "status": format!("{:?}", goal.status).to_lowercase(),
                }),
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
        let params: crate::runtime::handlers::goal::GoalCompleteParams =
            match serde_json::from_value(params) {
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
        match store.mutate(GoalMutation {
            goal_id: GoalId(params.goal_id),
            action: GoalAction::Complete {
                summary: params.verification_summary,
            },
        }) {
            Ok(goal) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: serde_json::json!({
                    "goal_id": goal.goal_id.0,
                    "status": format!("{:?}", goal.status).to_lowercase(),
                }),
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
                result: serde_json::json!({
                    "goal_id": goal.goal_id.0,
                    "status": format!("{:?}", goal.status).to_lowercase(),
                }),
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
        let params: crate::runtime::handlers::goal::GoalClearParams =
            match serde_json::from_value(params) {
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
        if let Some(store) = stores.get_mut(&params.session_id)
            && let Some(goal) = store.get().cloned()
            && goal.status.is_terminal()
        {
            store
                .mutate(GoalMutation {
                    goal_id: goal.goal_id.clone(),
                    action: GoalAction::Clear,
                })
                .ok();
        }

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: serde_json::json!({ "cleared": true }),
        })
        .expect("serialize goal clear result")
    }

    pub(super) async fn handle_goal_status(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        #[derive(serde::Deserialize)]
        struct GoalStatusParams {
            session_id: SessionId,
        }

        let params: GoalStatusParams = match serde_json::from_value(params) {
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
            .map(GoalProjection::from);

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: serde_json::json!({ "goal": projection }),
        })
        .expect("serialize goal status result")
    }
}
