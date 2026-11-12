use super::*;
use crate::goal::GoalStatus;

impl ServerRuntime {
    pub(super) async fn account_goal_turn_completed(
        &self,
        turn: &devo_protocol::native::turn::Turn,
    ) {
        let turn_id = turn.id;
        let session_id = turn.session_id;
        let continuation_goal_id = self
            .goal_continuation_turn_goals
            .lock()
            .await
            .remove(&turn_id);
        let Some(usage) = turn.usage.as_ref() else {
            return;
        };
        if self.session_is_plan_mode(session_id).await {
            return;
        }

        let token_delta = goal_token_delta(usage);
        let duration_delta_seconds = turn_duration_seconds(turn);
        if token_delta == 0 && duration_delta_seconds == 0 {
            return;
        }

        let mut stores = self.goal_stores.lock().await;
        let Some(goal) = stores.get_mut(&session_id).and_then(GoalStore::get_mut) else {
            return;
        };
        // A Python pause/cancel can finish its current turn normally. Charge
        // that final usage to its original goal regardless of final status,
        // but never charge a concurrently created replacement goal.
        if let Some(goal_id) = continuation_goal_id.as_ref()
            && &goal.goal_id != goal_id
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
                turn_id,
                token_delta,
                /*turn_delta*/ 0,
                duration_delta_seconds,
            )
            .await
        {
            tracing::warn!(%session_id, %turn_id, error = %error, "failed to persist goal token accounting record");
        }

        if previous_status != durable_goal.status {
            if let Err(error) = self
                .goal_durable_store
                .append_status_changed(&durable_goal, previous_status, None)
                .await
            {
                tracing::warn!(%session_id, %turn_id, error = %error, "failed to persist goal budget status record");
            }
            self.sync_core_session_goal(session_id, None).await;
        } else if durable_goal.status == GoalStatus::Active {
            self.sync_core_session_goal(session_id, Some(&durable_goal))
                .await;
        } else {
            self.sync_core_session_goal(session_id, None).await;
        }
    }

    async fn session_is_plan_mode(&self, session_id: SessionId) -> bool {
        self.session_collaboration_mode(session_id).await
            == Some(devo_protocol::CollaborationMode::Plan)
    }
}

fn goal_token_delta(usage: &devo_protocol::native::usage::TurnUsage) -> i64 {
    let non_cached_input = usage
        .query
        .input_tokens
        .saturating_sub(usage.query.cache_read_input_tokens);
    i64::try_from(non_cached_input.saturating_add(usage.query.output_tokens)).unwrap_or(i64::MAX)
}

fn turn_duration_seconds(turn: &devo_protocol::native::turn::Turn) -> u64 {
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
    goal.usage.time_used_seconds = goal
        .usage
        .time_used_seconds
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
            session_id: SessionId::from_legacy_uuid(uuid::Uuid::now_v7()),
            objective: "finish accounting".to_string(),
            token_budget: Some(max_tokens),
            replace_existing: false,
        })
        .expect("goal")
    }

    #[test]
    fn goal_token_delta_counts_non_cached_input_and_output() {
        let usage = devo_protocol::native::usage::TurnUsage {
            query: devo_protocol::native::usage::UsageTotals {
                total_tokens: 150,
                input_tokens: 120,
                output_tokens: 30,
                cache_creation_input_tokens: 40,
                cache_read_input_tokens: 70,
                ..Default::default()
            },
            overhead: Default::default(),
        };

        assert_eq!(goal_token_delta(&usage), 80);
    }

    #[test]
    fn apply_goal_usage_delta_leaves_budget_limit_for_wrap_up_turn() {
        let mut goal = goal_with_budget(100);
        goal.usage.tokens_used = 75;

        let previous_status = apply_goal_usage_delta(
            &mut goal, /*token_delta*/ 25, /*duration_delta_seconds*/ 3,
        );

        assert_eq!(previous_status, GoalStatus::Active);
        assert_eq!(goal.status, GoalStatus::Active);
        assert_eq!(goal.usage.tokens_used, 100);
        assert_eq!(goal.usage.time_used_seconds, 3);
    }
}
