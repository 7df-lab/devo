use super::*;
use crate::goal::GoalStatus;

impl ServerRuntime {
    pub(super) async fn account_goal_turn_completed(&self, turn: &TurnMetadata) {
        let continuation_goal_id = self
            .goal_continuation_turn_goals
            .lock()
            .await
            .remove(&turn.turn_id);
        let Some(usage) = turn.usage.as_ref() else {
            return;
        };
        if turn.status == TurnStatus::Pending || turn.status == TurnStatus::Running {
            return;
        }
        if self.session_is_plan_mode(turn.session_id).await {
            return;
        }

        let token_delta = goal_token_delta(usage);
        let duration_delta_seconds = turn_duration_seconds(turn);
        if token_delta == 0 && duration_delta_seconds == 0 {
            return;
        }

        let mut stores = self.goal_stores.lock().await;
        let Some(goal) = stores
            .get_mut(&turn.session_id)
            .and_then(GoalStore::get_mut)
        else {
            return;
        };
        if let Some(goal_id) = continuation_goal_id.as_ref()
            && (&goal.goal_id != goal_id
                || !matches!(goal.status, GoalStatus::Active | GoalStatus::BudgetLimited))
        {
            return;
        }
        let previous_status = apply_goal_usage_delta(goal, token_delta, duration_delta_seconds);
        let durable_goal = goal.clone();
        drop(stores);

        if let Err(error) = self
            .goal_durable_store
            .append_budget_accounted(
                &durable_goal,
                turn.turn_id,
                token_delta,
                /*turn_delta*/ 0,
                duration_delta_seconds,
            )
            .await
        {
            tracing::warn!(session_id = %turn.session_id, turn_id = %turn.turn_id, error = %error, "failed to persist goal token accounting record");
        }

        if previous_status != durable_goal.status {
            if let Err(error) = self
                .goal_durable_store
                .append_status_changed(&durable_goal, previous_status, None)
                .await
            {
                tracing::warn!(session_id = %turn.session_id, turn_id = %turn.turn_id, error = %error, "failed to persist goal budget status record");
            }
            self.sync_core_session_goal(turn.session_id, None).await;
        } else if durable_goal.status == GoalStatus::Active {
            self.sync_core_session_goal(turn.session_id, Some(durable_goal.to_thread_goal()))
                .await;
        } else {
            self.sync_core_session_goal(turn.session_id, None).await;
        }
    }

    async fn session_is_plan_mode(&self, session_id: SessionId) -> bool {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return false;
        };
        let core_session = {
            let session = session_arc.lock().await;
            Arc::clone(&session.core_session)
        };
        core_session.lock().await.collaboration_mode == devo_protocol::CollaborationMode::Plan
    }
}

fn goal_token_delta(usage: &TurnUsage) -> i64 {
    let cached_input = usage.cache_read_input_tokens.unwrap_or_default();
    let non_cached_input = usage.input_tokens.saturating_sub(cached_input);
    i64::from(non_cached_input + usage.output_tokens)
}

fn turn_duration_seconds(turn: &TurnMetadata) -> u64 {
    let Some(completed_at) = turn.completed_at else {
        return 0;
    };
    let seconds = (completed_at - turn.started_at).num_seconds();
    u64::try_from(seconds).unwrap_or_default()
}

fn apply_goal_usage_delta(
    goal: &mut crate::goal::Goal,
    token_delta: i64,
    duration_delta_seconds: u64,
) -> GoalStatus {
    let previous_status = goal.status;
    if token_delta > 0 {
        goal.usage.record_tokens(token_delta);
    }
    goal.usage.duration_seconds = goal
        .usage
        .duration_seconds
        .saturating_add(duration_delta_seconds);
    goal.updated_at = chrono::Utc::now();
    previous_status
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn goal_with_budget(max_tokens: i64) -> crate::goal::Goal {
        crate::goal::Goal::from_create_params(devo_protocol::GoalCreateParams {
            session_id: SessionId::new(),
            objective: "finish accounting".to_string(),
            token_budget: Some(max_tokens),
            replace_existing: false,
        })
        .expect("goal")
    }

    #[test]
    fn goal_token_delta_counts_non_cached_input_and_output() {
        // Trace: L2-DES-GOAL-001
        let usage = TurnUsage {
            input_tokens: 120,
            output_tokens: 30,
            cache_creation_input_tokens: Some(40),
            cache_read_input_tokens: Some(70),
            reasoning_output_tokens: None,
            total_tokens: None,
        };

        assert_eq!(goal_token_delta(&usage), 80);
    }

    #[test]
    fn apply_goal_usage_delta_leaves_budget_limit_for_wrap_up_turn() {
        // Trace: L2-DES-GOAL-001
        let mut goal = goal_with_budget(100);
        goal.usage.tokens_used = 75;

        let previous_status = apply_goal_usage_delta(
            &mut goal, /*token_delta*/ 25, /*duration_delta_seconds*/ 3,
        );

        assert_eq!(previous_status, GoalStatus::Active);
        assert_eq!(goal.status, GoalStatus::Active);
        assert_eq!(goal.usage.tokens_used, 100);
        assert_eq!(goal.usage.duration_seconds, 3);
    }
}
