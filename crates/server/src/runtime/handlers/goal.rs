//! Goal lifecycle handlers — create, pause, resume, complete, cancel, clear.
//!
//! Implements L3-BEH-SERVER-004 client protocol surface.
#![allow(dead_code)]

use devo_protocol::SessionId;
use serde::{Deserialize, Serialize};

use crate::goal::{
    CreateGoalParams, Goal, GoalAction, GoalBudget, GoalError, GoalId, GoalMutation, GoalStatus,
    GoalUsage,
};

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

    pub fn create(&mut self, params: CreateGoalParams) -> Result<Goal, GoalError> {
        if self.active_goal.is_some() {
            return Err(GoalError::AlreadyActive);
        }
        let goal = Goal {
            goal_id: GoalId::new(),
            session_id: params.session_id,
            prompt: params.prompt,
            description: params.description,
            status: GoalStatus::Active,
            created_turn_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            budget: GoalBudget {
                max_turns: params.max_iterations,
                max_tokens: params.max_tokens,
                max_duration_seconds: params.max_duration_seconds,
            },
            usage: GoalUsage::default(),
            progress_summary: None,
            blocker_summary: None,
            verification_summary: None,
        };
        let result = goal.clone();
        self.active_goal = Some(goal);
        Ok(result)
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
                return Err(GoalError::NotFound("cleared".into()));
            }
        }
        goal.updated_at = chrono::Utc::now();
        let result = goal.clone();
        self.active_goal = Some(goal);
        Ok(result)
    }
}

// ── Handler Params / Results ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalCreateParams {
    pub session_id: SessionId,
    pub prompt: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub max_iterations: Option<u32>,
    #[serde(default)]
    pub max_tokens: Option<i64>,
    #[serde(default)]
    pub max_duration_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalCreateResult {
    pub goal_id: String,
    pub status: String,
}

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

    fn make_params() -> CreateGoalParams {
        CreateGoalParams {
            session_id: SessionId::new(),
            prompt: "Refactor auth".into(),
            description: Some("Make testable".into()),
            max_iterations: Some(10),
            max_tokens: Some(100000),
            max_duration_seconds: None,
        }
    }

    #[test]
    fn goal_create_and_get() {
        let mut store = GoalStore::new();
        let params = CreateGoalParams {
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
        let params = CreateGoalParams {
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
        let params = CreateGoalParams {
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
        let params = CreateGoalParams {
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
    fn goal_clear_removes() {
        let mut store = GoalStore::new();
        let params = CreateGoalParams {
            session_id: SessionId::new(),
            ..make_params()
        };
        let goal = store.create(params).expect("create");
        let goal_id = goal.goal_id.clone();

        // Complete first (clear only works on terminal goals)
        store
            .mutate(GoalMutation {
                goal_id: goal_id.clone(),
                action: GoalAction::Complete { summary: None },
            })
            .expect("complete");

        store
            .mutate(GoalMutation {
                goal_id,
                action: GoalAction::Clear,
            })
            .expect_err("clear removes goal from store");
        assert!(store.get().is_none());
    }

    #[test]
    fn goal_already_active_errors() {
        let mut store = GoalStore::new();
        let params = CreateGoalParams {
            session_id: SessionId::new(),
            ..make_params()
        };
        store.create(params.clone()).expect("create");
        let result = store.create(params);
        assert!(result.is_err());
    }

    #[test]
    fn goal_projection_from_goal() {
        let goal = Goal {
            goal_id: GoalId::new(),
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
