//! Server-owned autonomous continuation for active thread goals.
//!
//! The TUI persists goal state through `goal/set`; this module decides when an
//! active goal is eligible to become an internal Build turn and launches that
//! turn without adding a synthetic user message to the transcript.

use super::*;
use crate::goal::GoalStatus;
use futures::future::BoxFuture;

struct GoalContinuationCandidate {
    goal_id: GoalId,
    goal: Goal,
}

impl ServerRuntime {
    pub(super) fn maybe_start_goal_continuation_turn(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
                return;
            };
            if self
                .pause_goal_continuation_after_failed_turn(session_id, &session_arc)
                .await
            {
                return;
            }
            if !session_allows_goal_continuation(&session_arc).await {
                return;
            }
            let Some(candidate) = self.goal_continuation_candidate(session_id).await else {
                return;
            };
            if !session_allows_goal_continuation(&session_arc).await {
                return;
            }

            let (turn_config, resolved_request) = {
                let session = session_arc.lock().await;
                let requested_model = session_model_selection(&session.summary);
                let requested_thinking = session.summary.thinking.clone();
                let turn_config = self
                    .deps
                    .resolve_turn_config(requested_model, requested_thinking);
                let resolved_request = turn_config
                    .model
                    .resolve_thinking_selection(turn_config.thinking_selection.as_deref());
                (turn_config, resolved_request)
            };
            let request_model = turn_config.provider_request_model(&resolved_request.request_model);

            let now = Utc::now();
            let turn = {
                let mut session = session_arc.lock().await;
                if !session_allows_goal_continuation_locked(&session) {
                    return;
                }
                let turn = TurnMetadata {
                    turn_id: TurnId::new(),
                    session_id,
                    sequence: session
                        .latest_turn
                        .as_ref()
                        .map_or(1, |turn| turn.sequence + 1),
                    status: TurnStatus::Running,
                    kind: devo_core::TurnKind::Regular,
                    model: turn_config.model.slug.clone(),
                    model_binding_id: turn_config.model_binding_id.clone(),
                    thinking: turn_config.thinking_selection.clone(),
                    reasoning_effort: resolved_request.effective_reasoning_effort,
                    request_model,
                    request_thinking: resolved_request.request_thinking.clone(),
                    started_at: now,
                    completed_at: None,
                    usage: None,
                };
                session.summary.status = SessionRuntimeStatus::ActiveTurn;
                session.summary.updated_at = now;
                apply_turn_config_to_session_summary(&mut session.summary, &turn_config);
                session.active_turn = Some(turn.clone());
                turn
            };
            if !self
                .mark_goal_continuation_turn_started(session_id, &candidate.goal_id, turn.turn_id)
                .await
            {
                self.clear_goal_continuation_turn_reservation(&session_arc, turn.turn_id)
                    .await;
                return;
            }

            if let Err(error) = self
                .append_goal_continuation_turn_start(session_id, &turn)
                .await
            {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn.turn_id,
                    error = %error,
                    "failed to persist goal continuation turn start"
                );
                self.clear_goal_continuation_turn_reservation(&session_arc, turn.turn_id)
                    .await;
                return;
            }

            self.broadcast_event(ServerEvent::SessionStatusChanged(
                SessionStatusChangedPayload {
                    session_id,
                    status: SessionRuntimeStatus::ActiveTurn,
                },
            ))
            .await;
            self.broadcast_event(ServerEvent::TurnStarted(TurnEventPayload {
                session_id,
                turn: turn.clone(),
            }))
            .await;

            let cancel_token = CancellationToken::new();
            self.active_turn_cancellations
                .lock()
                .await
                .insert(session_id, cancel_token);
            let runtime = Arc::clone(self);
            let task_turn = turn.clone();
            let task_turn_config = turn_config.clone();
            let task = tokio::spawn(async move {
                runtime
                    .execute_turn(ExecuteTurnRequest {
                        session_id,
                        turn: task_turn,
                        turn_config: task_turn_config,
                        display_input: String::new(),
                        input: String::new(),
                        collaboration_mode: devo_protocol::CollaborationMode::Build,
                        input_mode: TurnInputMode::HiddenGoalContinuation {
                            goal: candidate.goal.to_thread_goal(),
                        },
                    })
                    .await;
            });
            self.active_tasks
                .lock()
                .await
                .insert(session_id, task.abort_handle());
        })
    }

    async fn goal_continuation_candidate(
        &self,
        session_id: SessionId,
    ) -> Option<GoalContinuationCandidate> {
        let stores = self.goal_stores.lock().await;
        let goal = stores.get(&session_id)?.get()?.clone();
        if !goal.check_continuation().should_continue {
            return None;
        }
        Some(GoalContinuationCandidate {
            goal_id: goal.goal_id.clone(),
            goal,
        })
    }

    async fn pause_goal_continuation_after_failed_turn(
        &self,
        session_id: SessionId,
        session_arc: &Arc<Mutex<RuntimeSession>>,
    ) -> bool {
        let (latest_turn, failure_message) = {
            let session = session_arc.lock().await;
            let latest_turn = session.latest_turn.clone();
            let failure_message = latest_turn.as_ref().and_then(|turn| {
                latest_failed_turn_error_message(&session.persisted_turn_items, turn)
            });
            (latest_turn, failure_message)
        };

        let mut stores = self.goal_stores.lock().await;
        let Some(goal) = stores.get_mut(&session_id).and_then(GoalStore::get_mut) else {
            return false;
        };
        if goal.status != GoalStatus::Active
            || !failed_turn_should_suppress_goal(goal.updated_at, latest_turn.as_ref())
        {
            return false;
        }

        let previous_status = goal.status;
        goal.status = GoalStatus::Paused;
        goal.blocker_summary = Some(goal_failure_blocker_summary(failure_message.as_deref()));
        goal.updated_at = Utc::now();
        let durable_goal = goal.clone();
        drop(stores);

        if let Err(error) = self
            .goal_durable_store
            .append_status_changed(
                &durable_goal,
                previous_status,
                durable_goal.blocker_summary.clone(),
            )
            .await
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist failed-turn goal pause record");
        }
        self.sync_core_session_goal(session_id, None).await;
        tracing::warn!(
            session_id = %session_id,
            "paused active goal continuation after failed turn"
        );
        true
    }

    async fn append_goal_continuation_turn_start(
        &self,
        session_id: SessionId,
        turn: &TurnMetadata,
    ) -> anyhow::Result<()> {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return Ok(());
        };
        let (record, session_context, turn_context) = {
            let session = session_arc.lock().await;
            let core_session = session.core_session.lock().await;
            (
                session.record.clone(),
                core_session.session_context.clone(),
                core_session.latest_turn_context.clone(),
            )
        };
        if let Some(record) = record {
            self.rollout_store.append_turn(
                &record,
                build_turn_record(turn, session_context, turn_context),
            )?;
        }
        let goal = {
            let stores = self.goal_stores.lock().await;
            stores.get(&session_id).and_then(GoalStore::get).cloned()
        };
        if let Some(goal) = goal
            && let Some(prompt) = goal.continuation_prompt()
            && let Err(error) = self
                .goal_durable_store
                .append_context_snapshot(&goal, format!("turn-{}", turn.turn_id), prompt)
                .await
        {
            tracing::warn!(session_id = %session_id, turn_id = %turn.turn_id, error = %error, "failed to persist goal context snapshot");
        }
        Ok(())
    }

    async fn mark_goal_continuation_turn_started(
        &self,
        session_id: SessionId,
        goal_id: &GoalId,
        turn_id: TurnId,
    ) -> bool {
        let mut stores = self.goal_stores.lock().await;
        let goal = stores
            .get_mut(&session_id)
            .and_then(GoalStore::get_mut)
            .filter(|goal| &goal.goal_id == goal_id)
            .filter(|goal| goal.check_continuation().should_continue)
            .map(|goal| {
                let previous_status = goal.status;
                goal.usage.record_turn();
                if goal.token_budget_exhausted() {
                    goal.status = GoalStatus::BudgetLimited;
                    goal.blocker_summary = Some(
                        "Goal token budget reached; launched a budget-limit wrap-up turn."
                            .to_string(),
                    );
                }
                goal.updated_at = Utc::now();
                (goal.clone(), previous_status)
            });
        drop(stores);

        let Some((goal, previous_status)) = goal else {
            return false;
        };
        if let Err(error) = self
            .goal_durable_store
            .append_budget_accounted(
                &goal, turn_id, /*token_delta*/ 0, /*turn_delta*/ 1,
                /*duration_delta_seconds*/ 0,
            )
            .await
        {
            tracing::warn!(session_id = %session_id, turn_id = %turn_id, error = %error, "failed to persist goal turn accounting record");
        }
        if goal.status != previous_status {
            if let Err(error) = self
                .goal_durable_store
                .append_status_changed(&goal, previous_status, goal.blocker_summary.clone())
                .await
            {
                tracing::warn!(session_id = %session_id, turn_id = %turn_id, error = %error, "failed to persist goal budget-limit status record");
            }
            self.sync_core_session_goal(session_id, None).await;
        }
        true
    }

    async fn clear_goal_continuation_turn_reservation(
        &self,
        session_arc: &Arc<Mutex<RuntimeSession>>,
        turn_id: TurnId,
    ) {
        let mut session = session_arc.lock().await;
        if session
            .active_turn
            .as_ref()
            .is_some_and(|active| active.turn_id == turn_id)
        {
            session.active_turn = None;
            session.summary.status = SessionRuntimeStatus::Idle;
        }
    }
}

async fn session_allows_goal_continuation(session_arc: &Arc<Mutex<RuntimeSession>>) -> bool {
    let session = session_arc.lock().await;
    session_allows_goal_continuation_locked(&session)
}

fn session_allows_goal_continuation_locked(session: &RuntimeSession) -> bool {
    if session.active_turn.is_some()
        || !session.pending_approvals.is_empty()
        || !session.pending_user_inputs.is_empty()
    {
        return false;
    }
    let Ok(core_session) = session.core_session.try_lock() else {
        return false;
    };
    if core_session.collaboration_mode == devo_protocol::CollaborationMode::Plan {
        return false;
    }
    session
        .pending_turn_queue
        .lock()
        .expect("pending turn queue mutex should not be poisoned")
        .is_empty()
}

fn failed_turn_should_suppress_goal(
    goal_updated_at: chrono::DateTime<Utc>,
    latest_turn: Option<&TurnMetadata>,
) -> bool {
    let Some(turn) = latest_turn else {
        return false;
    };
    if turn.status != TurnStatus::Failed {
        return false;
    }
    turn.completed_at.unwrap_or(turn.started_at) > goal_updated_at
}

fn latest_failed_turn_error_message(
    items: &[crate::execution::PersistedTurnItem],
    latest_turn: &TurnMetadata,
) -> Option<String> {
    if latest_turn.status != TurnStatus::Failed {
        return None;
    }
    items.iter().rev().find_map(|item| {
        match (item.turn_id == latest_turn.turn_id, &item.turn_item) {
            (true, TurnItem::AgentMessage(TextItem { text })) if !text.trim().is_empty() => {
                Some(text.clone())
            }
            _ => None,
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GoalFailureClass {
    ToolCallAdjacency,
    ProviderParameter,
    Authentication,
    Permission,
    Other,
}

fn classify_goal_failure(message: Option<&str>) -> GoalFailureClass {
    let Some(message) = message else {
        return GoalFailureClass::Other;
    };
    let normalized = message.to_ascii_lowercase();
    if (normalized.contains("tool_calls") || normalized.contains("tool calls"))
        && (normalized.contains("tool_call_id")
            || normalized.contains("tool messages")
            || normalized.contains("insufficient tool messages"))
    {
        return GoalFailureClass::ToolCallAdjacency;
    }
    if normalized.contains("401")
        || normalized.contains("unauthorized")
        || normalized.contains("authentication")
        || normalized.contains("api key")
        || normalized.contains("token timeout")
    {
        return GoalFailureClass::Authentication;
    }
    if normalized.contains("403")
        || normalized.contains("434")
        || normalized.contains("forbidden")
        || normalized.contains("permission")
        || normalized.contains("no api permission")
    {
        return GoalFailureClass::Permission;
    }
    if normalized.contains("400")
        || normalized.contains("bad request")
        || normalized.contains("invalid request")
        || normalized.contains("invalid_request_error")
        || normalized.contains("invalid parameter")
        || normalized.contains("parameter error")
    {
        return GoalFailureClass::ProviderParameter;
    }
    GoalFailureClass::Other
}

fn goal_failure_blocker_summary(message: Option<&str>) -> String {
    match classify_goal_failure(message) {
        GoalFailureClass::ToolCallAdjacency => {
            "Goal continuation paused after an unrecoverable provider/protocol error: the provider rejected a tool-call transcript because assistant tool_calls were not followed by matching tool messages."
        }
        GoalFailureClass::ProviderParameter => {
            "Goal continuation paused after an unrecoverable provider parameter error, such as a 400 Bad Request or invalid request payload."
        }
        GoalFailureClass::Authentication => {
            "Goal continuation paused after an authentication error. Fix the provider credentials before resuming the goal."
        }
        GoalFailureClass::Permission => {
            "Goal continuation paused after a provider permission error. Fix model or account permissions before resuming the goal."
        }
        GoalFailureClass::Other => {
            "Goal continuation paused because the previous turn failed before the goal could continue."
        }
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(status: TurnStatus, completed_at: chrono::DateTime<Utc>) -> TurnMetadata {
        TurnMetadata {
            turn_id: TurnId::new(),
            session_id: SessionId::new(),
            sequence: 1,
            status,
            kind: devo_core::TurnKind::Regular,
            model: "model-a".into(),
            model_binding_id: None,
            thinking: None,
            reasoning_effort: None,
            request_model: "provider/model-a".into(),
            request_thinking: None,
            started_at: completed_at - chrono::Duration::seconds(1),
            completed_at: Some(completed_at),
            usage: None,
        }
    }

    #[test]
    fn failed_turn_after_goal_update_suppresses_continuation() {
        // Trace: L2-DES-GOAL-001
        let goal_updated_at = Utc::now();
        let latest_turn = turn(
            TurnStatus::Failed,
            goal_updated_at + chrono::Duration::seconds(1),
        );

        assert!(failed_turn_should_suppress_goal(
            goal_updated_at,
            Some(&latest_turn)
        ));
    }

    #[test]
    fn failed_turn_before_goal_update_allows_manual_resume() {
        // Trace: L2-DES-GOAL-001
        let latest_turn_completed_at = Utc::now();
        let goal_updated_at = latest_turn_completed_at + chrono::Duration::seconds(1);
        let latest_turn = turn(TurnStatus::Failed, latest_turn_completed_at);

        assert!(!failed_turn_should_suppress_goal(
            goal_updated_at,
            Some(&latest_turn)
        ));
    }

    #[test]
    fn completed_turn_does_not_suppress_continuation() {
        // Trace: L2-DES-GOAL-001
        let goal_updated_at = Utc::now();
        let latest_turn = turn(
            TurnStatus::Completed,
            goal_updated_at + chrono::Duration::seconds(1),
        );

        assert!(!failed_turn_should_suppress_goal(
            goal_updated_at,
            Some(&latest_turn)
        ));
    }

    #[test]
    fn classifies_tool_call_adjacency_failure() {
        // Trace: L2-DES-GOAL-001
        let message = "Invalid status code: 400 Bad Request; response body: assistant message with 'tool_calls' must be followed by tool messages responding to each 'tool_call_id'";

        assert_eq!(
            classify_goal_failure(Some(message)),
            GoalFailureClass::ToolCallAdjacency
        );
        assert!(goal_failure_blocker_summary(Some(message)).contains("assistant tool_calls"),);
    }

    #[test]
    fn classifies_provider_parameter_failure() {
        // Trace: L2-DES-GOAL-001
        assert_eq!(
            classify_goal_failure(Some("400 Bad Request invalid_request_error")),
            GoalFailureClass::ProviderParameter
        );
    }

    #[test]
    fn classifies_authentication_and_permission_failures() {
        // Trace: L2-DES-GOAL-001
        assert_eq!(
            classify_goal_failure(Some("401 unauthorized invalid api key")),
            GoalFailureClass::Authentication
        );
        assert_eq!(
            classify_goal_failure(Some("403 forbidden no api permission")),
            GoalFailureClass::Permission
        );
    }
}
