use super::*;

impl ServerRuntime {
    // ── Native session/goal/* (live Goal + Native GoalStatus; no ThreadGoal) ─

    /// Native `session/goal/set` (L2-DES-APP-008 Phase B): creates the
    /// session goal with `ifExists` semantics and idempotency-key replay.
    pub(super) async fn handle_native_session_goal_set(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionGoalSetParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/goal/set params: {error}"),
                    );
                }
            };
        let session_id = params.session_id;
        if let Some(goal) = self
            .goal_set_idempotency
            .lock()
            .await
            .get(&(session_id, params.idempotency_key.clone()))
            .cloned()
        {
            return serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_session::SessionGoalSetResult { goal },
            })
            .expect("serialize canonical session/goal/set response");
        }
        if !self.sessions.lock().await.contains_key(&session_id) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        }
        let replace_existing = matches!(
            params.if_exists,
            devo_protocol::native::rpc_session::GoalIfExists::Replace
        );
        let title_input = params.objective.trim().to_string();
        let create_params = devo_protocol::GoalCreateParams {
            session_id,
            objective: params.objective.clone(),
            token_budget: params
                .token_budget
                .and_then(|budget| i64::try_from(budget).ok()),
            replace_existing,
        };
        let mut stores = self.goal_stores.lock().await;
        let store = stores.entry(session_id).or_insert_with(GoalStore::new);
        let goal = match store.create(create_params) {
            Ok(goal) => goal,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("goal creation failed: {error}"),
                );
            }
        };
        let should_continue = goal.status == crate::goal::GoalStatus::Active;
        let durable_goal = goal.clone();
        let native_goal = goal.to_native_goal();
        drop(stores);
        if let Err(error) = self
            .goal_durable_store
            .append_goal_created(&durable_goal)
            .await
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal create record");
        }
        if replace_existing {
            self.interrupt_active_goal_continuation_turn(session_id, "goal replaced")
                .await;
        }
        self.sync_core_session_goal(session_id, Some(&durable_goal))
            .await;
        self.schedule_goal_followup_work(session_id, Some(title_input), should_continue)
            .await;
        self.goal_set_idempotency
            .lock()
            .await
            .insert((session_id, params.idempotency_key), native_goal.clone());
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::GoalCreated {
                goal: native_goal.clone(),
            },
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionGoalSetResult { goal: native_goal },
        })
        .expect("serialize canonical session/goal/set response")
    }

    /// Native `session/goal/update` (ratified #3): in-place edit of the
    /// current goal preserving id, usage stats, and continuation linkage.
    pub(super) async fn handle_native_session_goal_update(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionGoalUpdateParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/goal/update params: {error}"),
                    );
                }
            };
        let session_id = params.session_id;
        let current = self
            .goal_stores
            .lock()
            .await
            .get(&session_id)
            .and_then(|store| store.get().map(crate::goal::Goal::to_native_goal));
        let Some(current) = current else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::GoalNotFound,
                "no active goal to update",
            );
        };
        if let Some(expected) = params.expected_goal_id.as_ref()
            && *expected != current.id
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::GoalNotFound,
                "goal was replaced; refetch before editing",
            );
        }
        if let Some(goal) = self
            .goal_update_idempotency
            .lock()
            .await
            .get(&(session_id, params.idempotency_key.clone()))
            .cloned()
        {
            return serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_session::SessionGoalUpdateResult { goal },
            })
            .expect("serialize canonical session/goal/update replay response");
        }

        let status = match params.patch.status {
            None => None,
            Some(devo_protocol::native::goal::GoalStatus::Active) => {
                Some(crate::goal::GoalStatus::Active)
            }
            Some(devo_protocol::native::goal::GoalStatus::Paused) => {
                Some(crate::goal::GoalStatus::Paused)
            }
            Some(devo_protocol::native::goal::GoalStatus::Completed) => {
                Some(crate::goal::GoalStatus::Completed)
            }
            Some(system_controlled) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("goal status {system_controlled:?} is system-computed, not editable"),
                );
            }
        };
        let token_budget = match params.patch.token_budget {
            devo_protocol::native::patch::PatchField::Missing => None,
            devo_protocol::native::patch::PatchField::Null => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "clearing the token budget is not in the vocabulary",
                );
            }
            devo_protocol::native::patch::PatchField::Value(budget) => Some(budget),
        };
        if !self.sessions.lock().await.contains_key(&session_id) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        }
        let title_input = params
            .patch
            .objective
            .as_deref()
            .map(str::trim)
            .filter(|objective| !objective.is_empty())
            .map(str::to_string);
        let only_pause_budget_limited = status == Some(crate::goal::GoalStatus::Paused)
            && params.patch.objective.is_none()
            && token_budget.is_none();

        let mut stores = self.goal_stores.lock().await;
        let store = stores.entry(session_id).or_insert_with(GoalStore::new);
        let previous_status = store.get().map(|goal| goal.status);
        if previous_status == Some(crate::goal::GoalStatus::BudgetLimited)
            && only_pause_budget_limited
            && let Some(goal) = store.get().cloned()
        {
            let native_goal = goal.to_native_goal();
            drop(stores);
            self.interrupt_active_goal_continuation_turn(
                session_id,
                "budget-limited goal wrap-up stopped",
            )
            .await;
            self.sync_core_session_goal(session_id, None).await;
            self.goal_update_idempotency
                .lock()
                .await
                .insert((session_id, params.idempotency_key), native_goal.clone());
            return serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_session::SessionGoalUpdateResult {
                    goal: native_goal,
                },
            })
            .expect("serialize canonical session/goal/update response");
        }

        let legacy_session_id = session_id;
        let goal = match store.patch(
            params.patch.objective.clone(),
            status,
            token_budget,
            /*allow_create*/ false,
            legacy_session_id,
        ) {
            Ok(goal) => goal,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("goal set failed: {error}"),
                );
            }
        };
        let should_continue = goal.status == crate::goal::GoalStatus::Active;
        let should_interrupt_continuation = previous_status.is_some_and(|status| {
            matches!(
                status,
                crate::goal::GoalStatus::Active | crate::goal::GoalStatus::BudgetLimited
            )
        }) && !should_continue;
        let durable_goal = goal.clone();
        let native_goal = goal.to_native_goal();
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
        if should_interrupt_continuation {
            self.interrupt_active_goal_continuation_turn(
                session_id,
                "goal status changed from active",
            )
            .await;
        }
        self.sync_core_session_goal(session_id, Some(&durable_goal))
            .await;
        self.schedule_goal_followup_work(session_id, title_input, should_continue)
            .await;
        self.goal_update_idempotency
            .lock()
            .await
            .insert((session_id, params.idempotency_key), native_goal.clone());
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionGoalUpdateResult {
                goal: native_goal,
            },
        })
        .expect("serialize canonical session/goal/update response")
    }

    /// Native `session/goal/read`: the session's current goal, or `null`
    /// when none (including cleared goals).
    pub(super) async fn handle_native_session_goal_read(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionGoalReadParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/goal/read params: {error}"),
                    );
                }
            };
        let goal = self
            .goal_stores
            .lock()
            .await
            .get(&params.session_id)
            .and_then(|store| store.get())
            .filter(|goal| goal.status != crate::goal::GoalStatus::Cleared)
            .map(crate::goal::Goal::to_native_goal);
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionGoalReadResult { goal },
        })
        .expect("serialize canonical session/goal/read response")
    }

    /// Native goal lifecycle transitions (`session/goal/pause|resume|
    /// complete|cancel|clear`). `expectedGoalId` is a precondition against
    /// acting on a concurrently replaced goal.
    pub(super) async fn handle_native_session_goal_transition(
        self: &Arc<Self>,
        method: &str,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionGoalTransitionParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical {method} params: {error}"),
                    );
                }
            };
        let session_id = params.session_id;
        let current_goal = self
            .goal_stores
            .lock()
            .await
            .get(&session_id)
            .and_then(|store| store.get().cloned());
        let Some(current_goal) = current_goal else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::GoalNotFound,
                "session has no goal",
            );
        };
        if params.expected_goal_id != current_goal.goal_id {
            return self.error_response(
                request_id,
                ProtocolErrorCode::GoalNotFound,
                "expected goal id does not match the session's current goal",
            );
        }

        if method == "session/goal/clear" {
            let mut stores = self.goal_stores.lock().await;
            let cleared_goal_id = stores
                .get(&session_id)
                .and_then(GoalStore::get)
                .map(|goal| goal.goal_id);
            let cleared = stores.get_mut(&session_id).is_some_and(GoalStore::clear);
            drop(stores);
            if cleared {
                if let Some(goal_id) = cleared_goal_id {
                    let legacy_session_id = session_id;
                    if let Err(error) = self
                        .goal_durable_store
                        .append_goal_cleared(
                            legacy_session_id,
                            goal_id,
                            Some("user clear".to_string()),
                        )
                        .await
                    {
                        tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal clear record");
                    }
                    self.broadcast_notification(
                        devo_protocol::native::event::ServerNotification::GoalCleared {
                            session_id,
                            goal_id,
                        },
                    )
                    .await;
                }
                self.interrupt_active_goal_continuation_turn(session_id, "goal cleared")
                    .await;
                self.sync_core_session_goal(session_id, None).await;
            }
            return serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_session::SessionGoalClearResult {},
            })
            .expect("serialize canonical session/goal/clear response");
        }

        let mut stores = self.goal_stores.lock().await;
        let Some(store) = stores.get_mut(&session_id) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "no goal store for session",
            );
        };
        let previous_status = store.get().map(|goal| goal.status);

        let goal = match method {
            "session/goal/pause" => {
                let should_interrupt_continuation = previous_status.is_some_and(|status| {
                    matches!(
                        status,
                        crate::goal::GoalStatus::Active | crate::goal::GoalStatus::BudgetLimited
                    )
                });
                if previous_status == Some(crate::goal::GoalStatus::BudgetLimited)
                    && let Some(goal) = store.get().cloned()
                {
                    let native_goal = goal.to_native_goal();
                    drop(stores);
                    self.interrupt_active_goal_continuation_turn(
                        session_id,
                        "budget-limited goal wrap-up stopped",
                    )
                    .await;
                    self.sync_core_session_goal(session_id, None).await;
                    return serde_json::to_value(SuccessResponse {
                        id: request_id,
                        result: devo_protocol::native::rpc_session::SessionGoalTransitionResult {
                            goal: native_goal,
                        },
                    })
                    .expect("serialize canonical goal transition response");
                }
                match store.set_status(crate::goal::GoalStatus::Paused) {
                    Ok(goal) => {
                        let durable_goal = goal.clone();
                        let native_goal = goal.to_native_goal();
                        drop(stores);
                        if let Some(previous_status) = previous_status
                            && let Err(error) = self
                                .goal_durable_store
                                .append_status_changed(&durable_goal, previous_status, None)
                                .await
                        {
                            tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal pause record");
                        }
                        if should_interrupt_continuation {
                            self.interrupt_active_goal_continuation_turn(session_id, "goal paused")
                                .await;
                        }
                        self.sync_core_session_goal(session_id, None).await;
                        return serde_json::to_value(SuccessResponse {
                            id: request_id,
                            result:
                                devo_protocol::native::rpc_session::SessionGoalTransitionResult {
                                    goal: native_goal,
                                },
                        })
                        .expect("serialize canonical goal transition response");
                    }
                    Err(error) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InvalidParams,
                            format!("goal pause failed: {error}"),
                        );
                    }
                }
            }
            "session/goal/resume" => match store.set_status(crate::goal::GoalStatus::Active) {
                Ok(goal) => goal,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("goal resume failed: {error}"),
                    );
                }
            },
            "session/goal/complete" => match store.set_status(crate::goal::GoalStatus::Completed) {
                Ok(goal) => goal,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("goal complete failed: {error}"),
                    );
                }
            },
            "session/goal/cancel" => {
                match store.mutate(GoalMutation {
                    goal_id: current_goal.goal_id,
                    action: GoalAction::Cancel,
                }) {
                    Ok(goal) => goal,
                    Err(error) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InvalidParams,
                            format!("goal cancel failed: {error}"),
                        );
                    }
                }
            }
            _ => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("unknown goal transition method '{method}'"),
                );
            }
        };

        let should_continue = goal.status == crate::goal::GoalStatus::Active;
        let durable_goal = goal.clone();
        let native_goal = goal.to_native_goal();
        drop(stores);
        if let Some(previous_status) = previous_status
            && let Err(error) = self
                .goal_durable_store
                .append_status_changed(&durable_goal, previous_status, None)
                .await
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist goal status record");
        }
        match method {
            "session/goal/resume" => {
                self.sync_core_session_goal(session_id, Some(&durable_goal))
                    .await;
                self.schedule_goal_followup_work(
                    session_id,
                    /*title_input*/ None,
                    should_continue,
                )
                .await;
            }
            "session/goal/complete" => {
                self.interrupt_active_goal_continuation_turn(session_id, "goal completed")
                    .await;
                self.sync_core_session_goal(session_id, None).await;
            }
            "session/goal/cancel" => {
                self.interrupt_active_goal_continuation_turn(session_id, "goal canceled")
                    .await;
                self.sync_core_session_goal(session_id, None).await;
            }
            _ => {}
        }
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::GoalStatusChanged {
                session_id,
                goal_id: native_goal.id,
                status: native_goal.status,
            },
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionGoalTransitionResult {
                goal: native_goal,
            },
        })
        .expect("serialize canonical goal transition response")
    }

    // ── Legacy / ACP Goal Handlers (ThreadGoal wire) ──────────────────

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
        let replace_existing = params.replace_existing;
        let title_input = params.objective.trim().to_string();
        if !self.sessions.lock().await.contains_key(&session_id) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        }

        let mut stores = self.goal_stores.lock().await;
        let store = stores.entry(session_id).or_insert_with(GoalStore::new);
        match store.create(params) {
            Ok(goal) => {
                let should_continue = goal.status == crate::goal::GoalStatus::Active;
                let thread_goal = goal.to_thread_goal();
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
                // Interrupt before any session-actor mailbox round-trip: the actor
                // may be blocked inside an in-flight continuation turn.
                if replace_existing {
                    self.interrupt_active_goal_continuation_turn(session_id, "goal replaced")
                        .await;
                }
                self.sync_core_session_goal(session_id, Some(&durable_goal))
                    .await;
                self.schedule_goal_followup_work(session_id, Some(title_input), should_continue)
                    .await;
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
        let requested_status = params.status;
        let title_input = params
            .objective
            .as_deref()
            .map(str::trim)
            .filter(|objective| !objective.is_empty())
            .map(str::to_string);
        let only_pause_budget_limited = requested_status
            == Some(devo_protocol::ThreadGoalStatus::Paused)
            && params.objective.is_none()
            && params.token_budget.is_none();
        if !self.sessions.lock().await.contains_key(&session_id) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        }

        let mut stores = self.goal_stores.lock().await;
        let store = stores.entry(session_id).or_insert_with(GoalStore::new);
        let previous_status = store.get().map(|goal| goal.status);
        if previous_status == Some(crate::goal::GoalStatus::BudgetLimited)
            && only_pause_budget_limited
            && let Some(goal) = store.get().cloned()
        {
            let thread_goal = goal.to_thread_goal();
            let result = serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::GoalSetResult { goal: thread_goal },
            })
            .expect("serialize budget-limited goal pause result");
            drop(stores);
            self.interrupt_active_goal_continuation_turn(
                session_id,
                "budget-limited goal wrap-up stopped",
            )
            .await;
            self.sync_core_session_goal(session_id, None).await;
            return result;
        }
        match store.set(params) {
            Ok(goal) => {
                let should_continue = goal.status == crate::goal::GoalStatus::Active;
                let should_interrupt_continuation = previous_status.is_some_and(|status| {
                    matches!(
                        status,
                        crate::goal::GoalStatus::Active | crate::goal::GoalStatus::BudgetLimited
                    )
                }) && !should_continue;
                let thread_goal = goal.to_thread_goal();
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
                if should_interrupt_continuation {
                    self.interrupt_active_goal_continuation_turn(
                        session_id,
                        "goal status changed from active",
                    )
                    .await;
                }
                self.sync_core_session_goal(session_id, Some(&durable_goal))
                    .await;
                self.schedule_goal_followup_work(session_id, title_input, should_continue)
                    .await;
                result
            }
            Err(e) => self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("goal set failed: {e}"),
            ),
        }
    }

    pub(super) async fn handle_goal_clear(
        self: &Arc<Self>,
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
            .map(|goal| goal.goal_id);
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
            self.interrupt_active_goal_continuation_turn(params.session_id, "goal cleared")
                .await;
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

    /// Mirror active goal into the core session actor. Converts to ThreadGoal
    /// only at this core-session boundary (not Native wire).
    pub(super) async fn sync_core_session_goal(&self, session_id: SessionId, goal: Option<&Goal>) {
        let thread_goal = goal
            .filter(|goal| goal.status == crate::goal::GoalStatus::Active)
            .map(Goal::to_thread_goal);
        let Some(session_handle) = self.session(session_id).await else {
            return;
        };
        if self.runtime_active_turn_id(session_id).await.is_some() {
            // Queue without blocking the goal handler; the actor applies this once
            // the in-flight turn releases the mailbox.
            let _ = session_handle.try_set_active_goal(thread_goal);
            return;
        }
        session_handle.set_active_goal(thread_goal).await;
    }

    /// Title work must not block the session actor. When a turn is already
    /// active, defer heuristic prepare to a task (post-turn notify polishes).
    /// When idle, await the fast heuristic apply so continuation sees a title,
    /// then wake polish asynchronously.
    async fn schedule_goal_followup_work(
        self: &Arc<Self>,
        session_id: SessionId,
        title_input: Option<String>,
        should_continue: bool,
    ) {
        let turn_active = self.runtime_active_turn_id(session_id).await.is_some();
        if let Some(title_input) = title_input {
            if turn_active {
                let runtime = Arc::clone(self);
                tokio::spawn(async move {
                    runtime
                        .prepare_title_from_user_input(session_id, &title_input)
                        .await;
                });
            } else {
                // Heuristic apply is local/fast; await it so continuation sees a title.
                // LLM polish stays async via notify.
                self.prepare_title_from_user_input(session_id, &title_input)
                    .await;
                let runtime = Arc::clone(self);
                tokio::spawn(async move {
                    runtime.notify_title_polish(session_id).await;
                });
            }
        }
        if !should_continue {
            return;
        }
        if turn_active {
            return;
        }
        self.maybe_start_goal_continuation_turn(session_id).await;
    }

    /// Kernel `host_request("goal.get")` — same semantics as `session/goal/read`.
    pub(crate) async fn host_goal_get(&self, session_id: &str) -> serde_json::Value {
        let sid = SessionId::from_string(session_id.to_owned());
        let goal = self
            .goal_stores
            .lock()
            .await
            .get(&sid)
            .and_then(|store| store.get())
            .filter(|goal| goal.status != crate::goal::GoalStatus::Cleared)
            .map(crate::goal::Goal::to_native_goal);
        serde_json::json!({ "status": "ok", "goal": goal })
    }

    /// Kernel `host_request("goal.create")`.
    pub(crate) async fn host_goal_create(
        self: &Arc<Self>,
        session_id: &str,
        params: &serde_json::Value,
    ) -> serde_json::Value {
        let sid = SessionId::from_string(session_id.to_owned());
        let Some(objective) = params
            .get("objective")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
        else {
            return serde_json::json!({ "status": "error", "error": "goal.create requires objective" });
        };
        let token_budget = params
            .get("token_budget")
            .and_then(|v| v.as_i64())
            .or_else(|| {
                params
                    .get("token_budget")
                    .and_then(|v| v.as_u64())
                    .and_then(|n| i64::try_from(n).ok())
            });
        // Kernel contract (goal SKILL.md): create FAILS while a goal is still
        // pending (active/paused/blocked/usage- or budget-limited); a
        // terminal goal (completed/failed/canceled/cleared) is replaced.
        // Passing `replace_existing: true` unconditionally used to silently
        // discard a live goal mid-flight.
        let pending = {
            let stores = self.goal_stores.lock().await;
            stores
                .get(&sid)
                .and_then(|store| store.get())
                .is_some_and(|goal| {
                    !matches!(
                        goal.status,
                        devo_core::GoalStatus::Completed
                            | devo_core::GoalStatus::Failed
                            | devo_core::GoalStatus::Canceled
                            | devo_core::GoalStatus::Cleared
                    )
                })
        };
        if pending {
            return serde_json::json!({
                "status": "error",
                "error": "a goal is already pending for this session; complete, cancel, or \
                          clear it before creating a new one"
            });
        }
        let legacy = devo_protocol::GoalCreateParams {
            session_id: sid,
            objective,
            token_budget,
            replace_existing: true,
        };
        let response = self
            .handle_goal_create(
                serde_json::Value::Null,
                serde_json::to_value(&legacy).expect("serialize goal create"),
            )
            .await;
        if let Some(err) = response.get("error") {
            return serde_json::json!({
                "status": "error",
                "error": err.get("message").cloned().unwrap_or_else(|| err.clone()),
            });
        }
        let goal = self
            .goal_stores
            .lock()
            .await
            .get(&sid)
            .and_then(|store| store.get())
            .map(crate::goal::Goal::to_native_goal);
        serde_json::json!({ "status": "ok", "goal": goal })
    }

    /// Kernel `host_request("goal.complete")`.
    pub(crate) async fn host_goal_complete(
        self: &Arc<Self>,
        session_id: &str,
    ) -> serde_json::Value {
        let sid = SessionId::from_string(session_id.to_owned());
        let mut stores = self.goal_stores.lock().await;
        let Some(store) = stores.get_mut(&sid) else {
            return serde_json::json!({
                "status": "error",
                "error": "no active goal exists for this session"
            });
        };
        let previous_status = store.get().map(|goal| goal.status);
        match store.set_status(crate::goal::GoalStatus::Completed) {
            Ok(goal) => {
                let thread_goal = goal.to_thread_goal();
                let durable_goal = goal.clone();
                let native_goal = goal.to_native_goal();
                drop(stores);
                if let Some(previous_status) = previous_status
                    && let Err(error) = self
                        .goal_durable_store
                        .append_status_changed(&durable_goal, previous_status, None)
                        .await
                {
                    tracing::warn!(session_id = %sid, error = %error, "failed to persist host goal.complete");
                }
                // Match Native `session/goal/complete`: clear session goal and
                // notify clients before cancelling work so the TUI can drop
                // "Pursuing goal". Defer turn interrupt so this host_reply can
                // finish (`await goal.complete()` must return ok, not abort).
                // Clear continuation registration without cancelling this turn —
                // we are inside the continuation's own host_request.
                self.clear_goal_continuation_registration(sid).await;
                self.sync_core_session_goal(sid, None).await;
                // Prefer GoalUpdated (full goal) so TUI projection keeps objective
                // while flipping status to complete; StatusChanged alone is a stub.
                self.broadcast_notification(
                    devo_protocol::native::event::ServerNotification::GoalUpdated {
                        goal: native_goal.clone(),
                    },
                )
                .await;
                self.broadcast_notification(
                    devo_protocol::native::event::ServerNotification::GoalStatusChanged {
                        session_id: sid,
                        goal_id: native_goal.id,
                        status: native_goal.status,
                    },
                )
                .await;
                serde_json::json!({
                    "status": "ok",
                    "result": {
                        "status": "complete",
                        "tokens_used": thread_goal.tokens_used,
                        "time_used_seconds": thread_goal.time_used_seconds,
                    }
                })
            }
            Err(err) => serde_json::json!({
                "status": "error",
                "error": err.to_string(),
            }),
        }
    }
}
