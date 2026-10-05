//! Goal lifecycle — creation, mutation, budget tracking, autonomous continuation.
//!
//! Implements L3-BEH-SERVER-004. Tracks active goal state with budget
//! accounting, continuation triggers, and status transitions.
//!
//! Live field names match Native (`objective` / `token_budget` /
//! `tokens_used` / `time_used_seconds`). Durable GoalCreated still stores
//! `prompt`; GoalBudget JSON accepts legacy `max_tokens`. Projection to
//! Native is ID bridges + Cleared→Canceled only.

use chrono::{DateTime, Utc};
use devo_protocol::GoalCreateParams;
use devo_protocol::SessionId;
use devo_protocol::ThreadGoal;
use devo_protocol::ThreadGoalStatus;
use devo_protocol::validate_thread_goal_objective;
use devo_protocol::validate_thread_goal_token_budget;
use serde::{Deserialize, Serialize};

pub use devo_core::GoalBudget;
pub use devo_core::GoalStatus;
pub use devo_protocol::native::ids::GoalId;

// ── Goal State ──────────────────────────────────────────────────────

/// Active goal tracked per-session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Goal {
    pub goal_id: GoalId,
    pub session_id: SessionId,
    /// Native-aligned; primary GoalState snapshots accept legacy `prompt`.
    #[serde(alias = "prompt")]
    pub objective: String,
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

/// Reference to a turn by its id and sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnRef {
    pub turn_id: devo_protocol::TurnId,
    pub sequence: u32,
}

/// Legacy ThreadGoalStatus adapters (v1 / ACP / core-session wire only).
/// Native `session/goal/*` must use live [`GoalStatus`] identity — do not
/// collapse Blocked→Paused (or similar) on the Native path.
pub fn thread_goal_status_from_goal(status: GoalStatus) -> ThreadGoalStatus {
    match status {
        GoalStatus::Active => ThreadGoalStatus::Active,
        GoalStatus::Paused | GoalStatus::Blocked | GoalStatus::UsageLimited => {
            ThreadGoalStatus::Paused
        }
        GoalStatus::BudgetLimited => ThreadGoalStatus::BudgetLimited,
        GoalStatus::Completed | GoalStatus::Failed | GoalStatus::Canceled | GoalStatus::Cleared => {
            ThreadGoalStatus::Complete
        }
    }
}

/// Inverse of [`thread_goal_status_from_goal`] for legacy v1 params only.
pub fn goal_status_from_thread(status: ThreadGoalStatus) -> GoalStatus {
    match status {
        ThreadGoalStatus::Active => GoalStatus::Active,
        ThreadGoalStatus::Paused => GoalStatus::Paused,
        ThreadGoalStatus::BudgetLimited => GoalStatus::BudgetLimited,
        ThreadGoalStatus::Complete => GoalStatus::Completed,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GoalUsage {
    pub turns_used: u32,
    pub tokens_used: i64,
    /// Native-aligned; primary GoalState snapshots accept legacy `duration_seconds`.
    #[serde(alias = "duration_seconds")]
    pub time_used_seconds: u64,
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
        Ok(Self {
            goal_id: GoalId::new(),
            session_id: params.session_id,
            objective,
            description: None,
            status: GoalStatus::Active,
            created_turn_id: None,
            created_at: now,
            updated_at: now,
            budget: GoalBudget {
                max_turns: None,
                token_budget: params.token_budget,
                max_duration_seconds: None,
            },
            usage: GoalUsage::default(),
            progress_summary: None,
            blocker_summary: None,
            verification_summary: None,
        })
    }

    /// Project to legacy ThreadGoal (v1 / ACP / core-session prompts).
    /// Status collapse (Blocked→Paused, …) is intentional for that wire only.
    pub fn to_thread_goal(&self) -> ThreadGoal {
        ThreadGoal {
            thread_id: self.session_id,
            objective: self.objective.clone(),
            status: thread_goal_status_from_goal(self.status),
            token_budget: self.budget.token_budget,
            tokens_used: self.usage.tokens_used,
            time_used_seconds: i64::try_from(self.usage.time_used_seconds).unwrap_or(i64::MAX),
            created_at: self.created_at.timestamp(),
            updated_at: self.updated_at.timestamp(),
        }
    }

    /// Native live surface: status identity except Cleared→Canceled, plus ID bridges.
    /// First-party field names already match Native.
    pub fn to_native_goal(&self) -> devo_protocol::native::goal::Goal {
        use devo_protocol::native::goal::GoalStatus as NativeStatus;
        let status = match self.status {
            GoalStatus::Active => NativeStatus::Active,
            GoalStatus::Paused => NativeStatus::Paused,
            GoalStatus::Blocked => NativeStatus::Blocked,
            GoalStatus::UsageLimited => NativeStatus::UsageLimited,
            GoalStatus::BudgetLimited => NativeStatus::BudgetLimited,
            GoalStatus::Completed => NativeStatus::Completed,
            GoalStatus::Failed => NativeStatus::Failed,
            GoalStatus::Canceled | GoalStatus::Cleared => NativeStatus::Canceled,
        };
        devo_protocol::native::goal::Goal {
            id: self.goal_id,
            session_id: self.session_id,
            objective: self.objective.clone(),
            status,
            token_budget: self
                .budget
                .token_budget
                .and_then(|budget| u64::try_from(budget).ok()),
            tokens_used: u64::try_from(self.usage.tokens_used).unwrap_or(0),
            time_used_seconds: self.usage.time_used_seconds,
            progress_summary: self.progress_summary.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    pub fn continuation_prompt(&self) -> Option<String> {
        devo_core::render_goal_continuation_prompt(&self.to_thread_goal())
    }

    pub fn token_budget_exhausted(&self) -> bool {
        self.budget
            .token_budget
            .is_some_and(|token_budget| self.usage.tokens_used >= token_budget)
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
        Goal {
            goal_id: GoalId::new(),
            session_id: SessionId::new(),
            objective: "Refactor auth module".into(),
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
        goal.budget.token_budget = Some(1000);
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
        goal.objective = "finish <goal> & report \"done\"".into();
        goal.budget.token_budget = Some(100);
        goal.usage.tokens_used = 17;

        let prompt = goal.continuation_prompt().expect("active goal prompt");

        assert!(prompt.contains("finish &lt;goal&gt; &amp; report &quot;done&quot;"));
        assert!(!prompt.contains("finish <goal> & report \"done\""));
        assert!(prompt.contains("await goal.complete()"));
    }

    #[test]
    fn continuation_prompt_does_not_fabricate_default_budget() {
        // Trace: L2-DES-GOAL-001
        let goal = make_active_goal();

        let prompt = goal.continuation_prompt().expect("active goal prompt");

        assert!(prompt.contains("- token budget: none"));
        assert!(prompt.contains("- remaining tokens: unlimited"));
    }

    #[test]
    fn goal_status_is_terminal() {
        assert!(GoalStatus::BudgetLimited.is_terminal());
        assert!(GoalStatus::UsageLimited.is_terminal());
        assert!(GoalStatus::Completed.is_terminal());
        assert!(GoalStatus::Failed.is_terminal());
        assert!(GoalStatus::Canceled.is_terminal());
        assert!(GoalStatus::Cleared.is_terminal());
        assert!(!GoalStatus::Active.is_terminal());
        assert!(!GoalStatus::Paused.is_terminal());
        assert!(!GoalStatus::Blocked.is_terminal());
    }

    #[test]
    fn goal_status_serde_roundtrip() {
        for status in &[
            GoalStatus::Active,
            GoalStatus::Paused,
            GoalStatus::Blocked,
            GoalStatus::UsageLimited,
            GoalStatus::BudgetLimited,
            GoalStatus::Completed,
            GoalStatus::Failed,
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

    #[test]
    fn primary_snapshot_accepts_legacy_prompt_and_duration_fields() {
        let json = serde_json::json!({
            "goal_id": GoalId::new(),

            "session_id": SessionId::new(),
            "prompt": "legacy objective",
            "description": null,
            "status": "active",
            "created_turn_id": null,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "budget": { "max_tokens": 42 },
            "usage": { "turns_used": 0, "tokens_used": 0, "duration_seconds": 9 },
            "progress_summary": null,
            "blocker_summary": null,
            "verification_summary": null,
        });
        let goal: Goal = serde_json::from_value(json).expect("legacy snapshot");
        assert_eq!(goal.objective, "legacy objective");
        assert_eq!(goal.budget.token_budget, Some(42));
        assert_eq!(goal.usage.time_used_seconds, 9);
    }
}
