//! Goal lifecycle — creation, mutation, budget tracking, autonomous continuation.
//!
//! Implements L3-BEH-SERVER-004. Tracks active goal state with budget
//! accounting, continuation triggers, and status transitions.

use chrono::{DateTime, Utc};
use devo_protocol::SessionId;
use serde::{Deserialize, Serialize};

// ── Goal State ──────────────────────────────────────────────────────

/// Active goal tracked per-session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Goal {
    pub goal_id: GoalId,
    pub session_id: SessionId,
    pub prompt: String,
    pub description: Option<String>,
    pub status: GoalStatus,
    pub created_turn_id: Option<TurnRef>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub budget: GoalBudget,
    pub usage: GoalUsage,
    pub progress_summary: Option<String>,
    pub blocker_summary: Option<String>,
    pub verification_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GoalId(pub String);

impl Default for GoalId {
    fn default() -> Self {
        Self::new()
    }
}

impl GoalId {
    pub fn new() -> Self {
        Self(format!("goal-{}", devo_protocol::SessionId::new()))
    }
}

impl std::fmt::Display for GoalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Reference to a turn by its id and sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnRef {
    pub turn_id: devo_protocol::TurnId,
    pub sequence: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Paused,
    Completed,
    Failed,
    Blocked,
    Canceled,
    Cleared,
}

impl GoalStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Canceled | Self::Cleared
        )
    }
}

// ── Budget ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GoalBudget {
    pub max_turns: Option<u32>,
    pub max_tokens: Option<i64>,
    pub max_duration_seconds: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GoalUsage {
    pub turns_used: u32,
    pub tokens_used: i64,
    pub duration_seconds: u64,
}

impl GoalUsage {
    pub fn record_turn(&mut self) {
        self.turns_used += 1;
    }

    pub fn record_tokens(&mut self, tokens: i64) {
        self.tokens_used += tokens;
    }
}

// ── Goal Mutation Commands ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateGoalParams {
    pub session_id: SessionId,
    pub prompt: String,
    pub description: Option<String>,
    pub max_iterations: Option<u32>,
    pub max_tokens: Option<i64>,
    pub max_duration_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalMutation {
    pub goal_id: GoalId,
    pub action: GoalAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalAction {
    Pause,
    Resume,
    Complete { summary: Option<String> },
    Fail { reason: String },
    Block { reason: String },
    Cancel,
    Clear,
}

// ── Continuation ────────────────────────────────────────────────────

/// Whether the goal system should trigger an autonomous continuation turn.
#[derive(Debug, Clone)]
pub struct GoalContinuationDecision {
    pub should_continue: bool,
    pub reason: Option<String>,
}

impl Goal {
    /// Check whether this goal should trigger a continuation turn.
    pub fn check_continuation(&self) -> GoalContinuationDecision {
        if self.status != GoalStatus::Active {
            return GoalContinuationDecision {
                should_continue: false,
                reason: Some(format!("goal status is {:?}", self.status)),
            };
        }

        if let Some(max_turns) = self.budget.max_turns
            && self.usage.turns_used >= max_turns
        {
            return GoalContinuationDecision {
                should_continue: false,
                reason: Some("max turns reached".into()),
            };
        }

        if let Some(max_tokens) = self.budget.max_tokens
            && self.usage.tokens_used >= max_tokens
        {
            return GoalContinuationDecision {
                should_continue: false,
                reason: Some("max tokens reached".into()),
            };
        }

        GoalContinuationDecision {
            should_continue: true,
            reason: None,
        }
    }
}

// ── Goal Error ──────────────────────────────────────────────────────

#[derive(Debug, Clone, thiserror::Error)]
pub enum GoalError {
    #[error("goal not found: {0}")]
    NotFound(String),
    #[error("goal already active in session")]
    AlreadyActive,
    #[error("invalid transition")]
    InvalidTransition,
    #[error("budget exhausted: {0}")]
    BudgetExhausted(String),
    #[error("goal persistence failure: {0}")]
    PersistenceFailure(String),
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_active_goal() -> Goal {
        Goal {
            goal_id: GoalId::new(),
            session_id: SessionId::new(),
            prompt: "Refactor auth module".into(),
            description: Some("Make it more testable".into()),
            status: GoalStatus::Active,
            created_turn_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            budget: GoalBudget::default(),
            usage: GoalUsage::default(),
            progress_summary: None,
            blocker_summary: None,
            verification_summary: None,
        }
    }

    #[test]
    fn active_goal_continues() {
        let goal = make_active_goal();
        let decision = goal.check_continuation();
        assert!(decision.should_continue);
    }

    #[test]
    fn completed_goal_does_not_continue() {
        let mut goal = make_active_goal();
        goal.status = GoalStatus::Completed;
        assert!(!goal.check_continuation().should_continue);
    }

    #[test]
    fn turn_budget_exhausted_stops_continuation() {
        let mut goal = make_active_goal();
        goal.budget.max_turns = Some(5);
        goal.usage.turns_used = 5;
        assert!(!goal.check_continuation().should_continue);
    }

    #[test]
    fn token_budget_exhausted_stops_continuation() {
        let mut goal = make_active_goal();
        goal.budget.max_tokens = Some(1000);
        goal.usage.tokens_used = 1000;
        assert!(!goal.check_continuation().should_continue);
    }

    #[test]
    fn goal_status_is_terminal() {
        assert!(GoalStatus::Completed.is_terminal());
        assert!(GoalStatus::Failed.is_terminal());
        assert!(GoalStatus::Canceled.is_terminal());
        assert!(GoalStatus::Cleared.is_terminal());
        assert!(!GoalStatus::Active.is_terminal());
        assert!(!GoalStatus::Paused.is_terminal());
    }

    #[test]
    fn goal_status_serde_roundtrip() {
        for status in &[
            GoalStatus::Active,
            GoalStatus::Paused,
            GoalStatus::Completed,
            GoalStatus::Failed,
            GoalStatus::Blocked,
            GoalStatus::Canceled,
            GoalStatus::Cleared,
        ] {
            let json = serde_json::to_string(status).expect("serialize");
            let restored: GoalStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *status);
        }
    }

    #[test]
    fn usage_records_turns_and_tokens() {
        let mut usage = GoalUsage::default();
        assert_eq!(usage.turns_used, 0);
        usage.record_turn();
        assert_eq!(usage.turns_used, 1);
        usage.record_tokens(500);
        assert_eq!(usage.tokens_used, 500);
    }
}
