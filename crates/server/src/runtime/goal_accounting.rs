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
        let previous_status =
            apply_goal_usage_delta(goal, turn, token_delta, duration_delta_seconds);
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
                .append_status_changed(
                    &durable_goal,
                    previous_status,
                    durable_goal.blocker_summary.clone(),
                )
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
    turn: &devo_protocol::native::turn::Turn,
    token_delta: i64,
    duration_delta_seconds: u64,
) -> GoalStatus {
    let previous_status = goal.status;
    // Accounting advances updated_at too. Apply failure suppression before that
    // bookkeeping update can masquerade as an explicit resume after the failure.
    // A newer owner change (or a non-active goal) must still win.
    if goal.status == GoalStatus::Active
        && turn.status == devo_protocol::native::turn::TurnStatus::Failed
        && turn.completed_at.unwrap_or(turn.started_at) > goal.updated_at
    {
        goal.status = GoalStatus::Paused;
        goal.blocker_summary = Some(super::goal_continuation::goal_failure_blocker_summary(
            turn.error.as_ref().map(|error| error.message.as_str()),
        ));
    }
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

    fn completed_turn(
        status: devo_protocol::native::turn::TurnStatus,
        completed_at: chrono::DateTime<Utc>,
    ) -> devo_protocol::native::turn::Turn {
        devo_protocol::native::turn::Turn {
            id: TurnId::new(),
            session_id: SessionId::new(),
            sequence: 1,
            kind: devo_protocol::native::turn::TurnKind::GoalContinuation,
            status,
            model: devo_protocol::native::model::ModelBinding {
                provider: "test".into(),
                model: "test".into(),
                variant: None,
                reasoning_effort: None,
            },
            collaboration_mode: None,
            started_at: completed_at - chrono::Duration::seconds(2),
            completed_at: Some(completed_at),
            error: Some(devo_protocol::native::error::AgentError::new(
                "PROVIDER_ERROR",
                "400 Bad Request",
            )),
            usage: None,
        }
    }

    #[test]
    fn failed_turn_accounting_pauses_before_advancing_updated_at() {
        let mut goal = goal_with_budget(100);
        let completed_at = goal.updated_at + chrono::Duration::seconds(1);
        let turn = completed_turn(
            devo_protocol::native::turn::TurnStatus::Failed,
            completed_at,
        );
        let previous = apply_goal_usage_delta(&mut goal, &turn, 0, 2);
        assert_eq!(previous, GoalStatus::Active);
        assert_eq!(goal.status, GoalStatus::Paused);
        assert_eq!(goal.usage.time_used_seconds, 2);
        assert_eq!(
            goal.blocker_summary,
            Some(
                super::super::goal_continuation::goal_failure_blocker_summary(Some(
                    "400 Bad Request"
                ))
            )
        );
    }

    #[test]
    fn failed_turn_accounting_preserves_newer_owner_resume_and_inactive_status() {
        let mut goal = goal_with_budget(100);
        let turn = completed_turn(
            devo_protocol::native::turn::TurnStatus::Failed,
            goal.updated_at - chrono::Duration::seconds(1),
        );
        apply_goal_usage_delta(&mut goal, &turn, 3, 2);
        assert_eq!(goal.status, GoalStatus::Active);
        assert_eq!(goal.usage.tokens_used, 3);
        for status in [
            GoalStatus::Paused,
            GoalStatus::Canceled,
            GoalStatus::Completed,
        ] {
            goal.status = status;
            goal.updated_at = turn.started_at;
            apply_goal_usage_delta(&mut goal, &turn, 0, 2);
            assert_eq!(goal.status, status);
        }
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
            &mut goal,
            &completed_turn(
                devo_protocol::native::turn::TurnStatus::Completed,
                Utc::now(),
            ),
            /*token_delta*/ 25,
            /*duration_delta_seconds*/ 3,
        );

        assert_eq!(previous_status, GoalStatus::Active);
        assert_eq!(goal.status, GoalStatus::Active);
        assert_eq!(goal.usage.tokens_used, 100);
        assert_eq!(goal.usage.time_used_seconds, 3);
    }
}
