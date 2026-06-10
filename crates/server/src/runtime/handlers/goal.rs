//! Goal lifecycle handlers — create, pause, resume, complete, cancel, clear.
//!
//! Implements L3-BEH-SERVER-004 client protocol surface.
#![allow(dead_code)]

use devo_protocol::GoalCreateParams;
use devo_protocol::GoalSetParams;
use devo_protocol::SessionId;
use devo_protocol::ThreadGoalStatus;
use devo_protocol::validate_thread_goal_objective;
use devo_protocol::validate_thread_goal_token_budget;
use serde::{Deserialize, Serialize};

use crate::goal::{Goal, GoalAction, GoalError, GoalMutation, GoalStatus};
#[cfg(test)]
use crate::goal::{GoalBudget, GoalId, GoalUsage};

// ── Goal State Store (in-memory placeholder) ───────────────────────

/// In-memory goal store for a single session.
#[derive(Debug, Clone, Default)]
pub struct GoalStore {
    pub active_goal: Option<Goal>,
}

impl GoalStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self) -> Option<&Goal> {
        self.active_goal.as_ref()
    }

    pub fn get_mut(&mut self) -> Option<&mut Goal> {
        self.active_goal.as_mut()
    }

    pub fn create(&mut self, params: GoalCreateParams) -> Result<Goal, GoalError> {
        if self.active_goal.is_some() && !params.replace_existing {
            return Err(GoalError::AlreadyActive);
        }
        let goal = Goal::from_create_params(params)?;
        let result = goal.clone();
        self.active_goal = Some(goal);
        Ok(result)
    }

    pub fn set(&mut self, params: GoalSetParams) -> Result<Goal, GoalError> {
        validate_thread_goal_token_budget(params.token_budget)
            .map_err(GoalError::InvalidObjective)?;

        if let Some(objective) = params.objective.as_deref() {
            let objective = objective.trim();
            validate_thread_goal_objective(objective).map_err(GoalError::InvalidObjective)?;

            if let Some(goal) = self.active_goal.as_mut() {
                goal.prompt = objective.to_string();
                apply_goal_update(goal, params.status, params.token_budget);
                goal.updated_at = chrono::Utc::now();
                return Ok(goal.clone());
            }

            let mut goal = Goal::from_create_params(GoalCreateParams {
                session_id: params.session_id,
                objective: objective.to_string(),
                token_budget: params.token_budget,
                replace_existing: false,
            })?;
            if let Some(status) = params.status {
                goal.status = GoalStatus::from_thread_goal_status(status);
                apply_goal_budget_limit(&mut goal);
            }
            let result = goal.clone();
            self.active_goal = Some(goal);
            return Ok(result);
        }

        let Some(goal) = self.active_goal.as_mut() else {
            return Err(GoalError::NotFound("current".to_string()));
        };
        if params.status.is_none() && params.token_budget.is_none() {
            return Err(GoalError::InvalidTransition);
        }
        apply_goal_update(goal, params.status, params.token_budget);
        goal.updated_at = chrono::Utc::now();
        Ok(goal.clone())
    }

    pub fn mutate(&mut self, mutation: GoalMutation) -> Result<Goal, GoalError> {
        if self.active_goal.is_none() {
            return Err(GoalError::NotFound(mutation.goal_id.0.clone()));
        }
        let mut goal = self.active_goal.take().unwrap();

        if goal.goal_id != mutation.goal_id {
            self.active_goal = Some(goal);
            return Err(GoalError::NotFound(mutation.goal_id.0.clone()));
        }

        match mutation.action {
            GoalAction::Pause => {
                if goal.status != GoalStatus::Active {
                    self.active_goal = Some(goal);
                    return Err(GoalError::InvalidTransition);
                }
                goal.status = GoalStatus::Paused;
            }
            GoalAction::Resume => {
                if goal.status != GoalStatus::Paused && goal.status != GoalStatus::Blocked {
                    self.active_goal = Some(goal);
                    return Err(GoalError::InvalidTransition);
                }
                goal.status = GoalStatus::Active;
            }
            GoalAction::Complete { summary } => {
                if goal.status.is_terminal() {
                    self.active_goal = Some(goal);
                    return Err(GoalError::InvalidTransition);
                }
                goal.status = GoalStatus::Completed;
                goal.verification_summary = summary;
            }
            GoalAction::Fail { reason } => {
                if goal.status.is_terminal() {
                    self.active_goal = Some(goal);
                    return Err(GoalError::InvalidTransition);
                }
                goal.status = GoalStatus::Failed;
                goal.blocker_summary = Some(reason);
            }
            GoalAction::Block { reason } => {
                if goal.status != GoalStatus::Active {
                    self.active_goal = Some(goal);
                    return Err(GoalError::InvalidTransition);
                }
                goal.status = GoalStatus::Blocked;
                goal.blocker_summary = Some(reason);
            }
            GoalAction::Cancel => {
                if goal.status.is_terminal() {
                    self.active_goal = Some(goal);
                    return Err(GoalError::InvalidTransition);
                }
                goal.status = GoalStatus::Canceled;
            }
            GoalAction::Clear => {
                self.active_goal = None;
                return Ok(goal);
            }
        }
        goal.updated_at = chrono::Utc::now();
        let result = goal.clone();
        self.active_goal = Some(goal);
        Ok(result)
    }

    pub fn set_status(&mut self, status: ThreadGoalStatus) -> Result<Goal, GoalError> {
        let Some(goal) = self.active_goal.as_mut() else {
            return Err(GoalError::NotFound("current".to_string()));
        };
        if goal.status.is_terminal() {
            return Err(GoalError::InvalidTransition);
        }
        goal.status = GoalStatus::from_thread_goal_status(status);
        goal.updated_at = chrono::Utc::now();
        Ok(goal.clone())
    }

    pub fn clear(&mut self) -> bool {
        self.active_goal.take().is_some()
    }
}

fn apply_goal_update(goal: &mut Goal, status: Option<ThreadGoalStatus>, token_budget: Option<i64>) {
    if let Some(token_budget) = token_budget {
        goal.budget.max_tokens = Some(token_budget);
    }
    if let Some(status) = status {
        goal.status = GoalStatus::from_thread_goal_status(status);
    }
    apply_goal_budget_limit(goal);
}

fn apply_goal_budget_limit(goal: &mut Goal) {
    if goal.status == GoalStatus::Active
        && goal
            .budget
            .max_tokens
            .is_some_and(|budget| goal.usage.tokens_used >= budget)
    {
        goal.status = GoalStatus::BudgetLimited;
    }
}

// ── Handler Params / Results ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalPauseParams {
    pub session_id: SessionId,
    pub goal_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalResumeParams {
    pub session_id: SessionId,
    pub goal_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalCompleteParams {
    pub session_id: SessionId,
    pub goal_id: String,
    #[serde(default)]
    pub verification_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalCancelParams {
    pub session_id: SessionId,
    pub goal_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalClearParams {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalActionResult {
    pub goal_id: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalStatusResult {
    pub goal: Option<GoalProjection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalProjection {
    pub goal_id: String,
    pub prompt: String,
    pub status: String,
    pub turns_used: u32,
    pub tokens_used: i64,
    pub progress_summary: Option<String>,
}

impl From<&Goal> for GoalProjection {
    fn from(g: &Goal) -> Self {
        Self {
            goal_id: g.goal_id.0.clone(),
            prompt: g.prompt.clone(),
            status: format!("{:?}", g.status).to_lowercase(),
            turns_used: g.usage.turns_used,
            tokens_used: g.usage.tokens_used,
            progress_summary: g.progress_summary.clone(),
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn make_params() -> GoalCreateParams {
        GoalCreateParams {
            session_id: SessionId::new(),
            objective: "Refactor auth".into(),
            token_budget: Some(100_000),
            replace_existing: false,
        }
    }

    #[test]
    fn goal_create_and_get() {
        let mut store = GoalStore::new();
        let params = GoalCreateParams {
            session_id: SessionId::new(),
            ..make_params()
        };
        let goal = store.create(params).expect("create");
        assert_eq!(goal.status, GoalStatus::Active);
        assert!(store.get().is_some());
    }

    #[test]
    fn goal_pause_and_resume() {
        let mut store = GoalStore::new();
        let params = GoalCreateParams {
            session_id: SessionId::new(),
            ..make_params()
        };
        let goal = store.create(params).expect("create");
        let goal_id = goal.goal_id.clone();

        store
            .mutate(GoalMutation {
                goal_id: goal_id.clone(),
                action: GoalAction::Pause,
            })
            .expect("pause");
        assert_eq!(store.get().unwrap().status, GoalStatus::Paused);

        store
            .mutate(GoalMutation {
                goal_id,
                action: GoalAction::Resume,
            })
            .expect("resume");
        assert_eq!(store.get().unwrap().status, GoalStatus::Active);
    }

    #[test]
    fn goal_complete_is_terminal() {
        let mut store = GoalStore::new();
        let params = GoalCreateParams {
            session_id: SessionId::new(),
            ..make_params()
        };
        let goal = store.create(params).expect("create");
        let goal_id = goal.goal_id.clone();

        store
            .mutate(GoalMutation {
                goal_id: goal_id.clone(),
                action: GoalAction::Complete { summary: None },
            })
            .expect("complete");
        assert!(store.get().unwrap().status.is_terminal());

        // Cannot pause a completed goal
        let result = store.mutate(GoalMutation {
            goal_id,
            action: GoalAction::Pause,
        });
        assert!(result.is_err());
    }

    #[test]
    fn goal_cancel() {
        let mut store = GoalStore::new();
        let params = GoalCreateParams {
            session_id: SessionId::new(),
            ..make_params()
        };
        let goal = store.create(params).expect("create");
        let goal_id = goal.goal_id.clone();

        store
            .mutate(GoalMutation {
                goal_id,
                action: GoalAction::Cancel,
            })
            .expect("cancel");
        assert_eq!(store.get().unwrap().status, GoalStatus::Canceled);
    }

    #[test]
    fn set_status_cannot_reactivate_terminal_goal() {
        // Trace: L2-DES-GOAL-001
        let mut store = GoalStore::new();
        let params = GoalCreateParams {
            session_id: SessionId::new(),
            ..make_params()
        };
        store.create(params).expect("create");
        store
            .set_status(ThreadGoalStatus::Complete)
            .expect("complete");

        let result = store.set_status(ThreadGoalStatus::Active);

        assert!(result.is_err());
        assert_eq!(store.get().unwrap().status, GoalStatus::Completed);
    }

    #[test]
    fn goal_clear_removes() {
        let mut store = GoalStore::new();
        let params = GoalCreateParams {
            session_id: SessionId::new(),
            ..make_params()
        };
        store.create(params).expect("create");

        assert!(store.clear());
        assert!(store.get().is_none());
    }

    #[test]
    fn goal_already_active_errors() {
        let mut store = GoalStore::new();
        let params = GoalCreateParams {
            session_id: SessionId::new(),
            ..make_params()
        };
        store.create(params.clone()).expect("create");
        let result = store.create(params);
        assert!(result.is_err());
    }

    #[test]
    fn goal_set_updates_objective_without_resetting_usage() {
        let mut store = GoalStore::new();
        let session_id = SessionId::new();
        store
            .create(GoalCreateParams {
                session_id,
                objective: "Refactor auth".into(),
                token_budget: Some(100_000),
                replace_existing: false,
            })
            .expect("create");
        let goal = store.active_goal.as_mut().expect("goal");
        goal.usage.tokens_used = 1_500;
        goal.usage.duration_seconds = 42;

        let updated = store
            .set(GoalSetParams {
                session_id,
                objective: Some("Refactor auth and payments".into()),
                status: Some(ThreadGoalStatus::Paused),
                token_budget: Some(80_000),
            })
            .expect("set");

        let expected = Goal {
            prompt: "Refactor auth and payments".into(),
            status: GoalStatus::Paused,
            budget: GoalBudget {
                max_tokens: Some(80_000),
                ..GoalBudget::default()
            },
            usage: GoalUsage {
                turns_used: 0,
                tokens_used: 1_500,
                duration_seconds: 42,
            },
            ..updated.clone()
        };
        assert_eq!(updated, expected);
    }

    #[test]
    fn goal_projection_from_goal() {
        let durable_goal_id = devo_core::GoalId::new();
        let goal = Goal {
            goal_id: GoalId::from_durable(durable_goal_id),
            durable_goal_id,
            session_id: SessionId::new(),
            prompt: "test".into(),
            description: None,
            status: GoalStatus::Active,
            created_turn_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            budget: GoalBudget::default(),
            usage: GoalUsage {
                turns_used: 3,
                tokens_used: 1500,
                duration_seconds: 0,
            },
            progress_summary: Some("making progress".into()),
            blocker_summary: None,
            verification_summary: None,
        };
        let proj = GoalProjection::from(&goal);
        assert_eq!(proj.turns_used, 3);
        assert_eq!(proj.tokens_used, 1500);
        assert!(proj.progress_summary.is_some());
    }
}
