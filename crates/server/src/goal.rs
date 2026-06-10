//! Goal lifecycle — creation, mutation, budget tracking, autonomous continuation.
//!
//! Implements L3-BEH-SERVER-004. Tracks active goal state with budget
//! accounting, continuation triggers, and status transitions.

use chrono::{DateTime, Utc};
use devo_protocol::GoalCreateParams;
use devo_protocol::SessionId;
use devo_protocol::ThreadGoal;
use devo_protocol::ThreadGoalStatus;
use devo_protocol::validate_thread_goal_objective;
use devo_protocol::validate_thread_goal_token_budget;
use serde::{Deserialize, Serialize};

// ── Goal State ──────────────────────────────────────────────────────

/// Active goal tracked per-session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Goal {
    pub goal_id: GoalId,
    pub durable_goal_id: devo_core::GoalId,
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
        Self::from_durable(devo_core::GoalId::new())
    }

    pub fn from_durable(goal_id: devo_core::GoalId) -> Self {
        Self(format!("goal-{}", goal_id.0))
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
    BudgetLimited,
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
            Self::BudgetLimited | Self::Completed | Self::Failed | Self::Canceled | Self::Cleared
        )
    }

    pub fn as_thread_goal_status(self) -> ThreadGoalStatus {
        match self {
            Self::Active => ThreadGoalStatus::Active,
            Self::Paused | Self::Blocked => ThreadGoalStatus::Paused,
            Self::BudgetLimited => ThreadGoalStatus::BudgetLimited,
            Self::Completed | Self::Failed | Self::Canceled | Self::Cleared => {
                ThreadGoalStatus::Complete
            }
        }
    }

    pub fn from_thread_goal_status(status: ThreadGoalStatus) -> Self {
        match status {
            ThreadGoalStatus::Active => Self::Active,
            ThreadGoalStatus::Paused => Self::Paused,
            ThreadGoalStatus::BudgetLimited => Self::BudgetLimited,
            ThreadGoalStatus::Complete => Self::Completed,
        }
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
    pub fn from_create_params(params: GoalCreateParams) -> Result<Self, GoalError> {
        let objective = params.objective.trim().to_string();
        validate_thread_goal_objective(&objective).map_err(GoalError::InvalidObjective)?;
        validate_thread_goal_token_budget(params.token_budget)
            .map_err(GoalError::InvalidObjective)?;
        let now = Utc::now();
        let durable_goal_id = devo_core::GoalId::new();
        Ok(Self {
            goal_id: GoalId::from_durable(durable_goal_id),
            durable_goal_id,
            session_id: params.session_id,
            prompt: objective,
            description: None,
            status: GoalStatus::Active,
            created_turn_id: None,
            created_at: now,
            updated_at: now,
            budget: GoalBudget {
                max_turns: None,
                max_tokens: params.token_budget,
                max_duration_seconds: None,
            },
            usage: GoalUsage::default(),
            progress_summary: None,
            blocker_summary: None,
            verification_summary: None,
        })
    }

    pub fn to_thread_goal(&self) -> ThreadGoal {
        ThreadGoal {
            thread_id: self.session_id,
            objective: self.prompt.clone(),
            status: self.status.as_thread_goal_status(),
            token_budget: self.budget.max_tokens,
            tokens_used: self.usage.tokens_used,
            time_used_seconds: i64::try_from(self.usage.duration_seconds).unwrap_or(i64::MAX),
            created_at: self.created_at.timestamp(),
            updated_at: self.updated_at.timestamp(),
        }
    }

    pub fn continuation_prompt(&self) -> Option<String> {
        devo_core::render_goal_continuation_prompt(&self.to_thread_goal())
    }

    pub fn token_budget_exhausted(&self) -> bool {
        self.budget
            .max_tokens
            .is_some_and(|max_tokens| self.usage.tokens_used >= max_tokens)
    }

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

        if self.token_budget_exhausted() {
            return GoalContinuationDecision {
                should_continue: true,
                reason: Some("token budget wrap-up".into()),
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
    #[error("{0}")]
    InvalidObjective(String),
    #[error("budget exhausted: {0}")]
    BudgetExhausted(String),
    #[error("goal persistence failure: {0}")]
    PersistenceFailure(String),
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn make_active_goal() -> Goal {
        let durable_goal_id = devo_core::GoalId::new();
        Goal {
            goal_id: GoalId::from_durable(durable_goal_id),
            durable_goal_id,
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
    fn token_budget_exhausted_allows_budget_wrap_up_continuation() {
        // Trace: L2-DES-GOAL-001
        let mut goal = make_active_goal();
        goal.budget.max_tokens = Some(1000);
        goal.usage.tokens_used = 1000;
        assert_eq!(
            goal.check_continuation().reason,
            Some("token budget wrap-up".to_string())
        );
        assert!(goal.check_continuation().should_continue);
    }

    #[test]
    fn continuation_prompt_escapes_untrusted_objective_xml() {
        // Trace: L2-DES-GOAL-001
        let mut goal = make_active_goal();
        goal.prompt = "finish <goal> & report \"done\"".into();
        goal.budget.max_tokens = Some(100);
        goal.usage.tokens_used = 17;

        let prompt = goal.continuation_prompt().expect("active goal prompt");

        assert!(prompt.contains("finish &lt;goal&gt; &amp; report &quot;done&quot;"));
        assert!(!prompt.contains("finish <goal> & report \"done\""));
        assert!(prompt.contains("Completion audit:"));
    }

    #[test]
    fn continuation_prompt_does_not_fabricate_default_budget() {
        // Trace: L2-DES-GOAL-001
        let goal = make_active_goal();

        let prompt = goal.continuation_prompt().expect("active goal prompt");

        assert!(prompt.contains("- Token budget: none"));
        assert!(prompt.contains("- Tokens remaining: unlimited"));
    }

    #[test]
    fn goal_status_is_terminal() {
        assert!(GoalStatus::BudgetLimited.is_terminal());
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
            GoalStatus::BudgetLimited,
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
