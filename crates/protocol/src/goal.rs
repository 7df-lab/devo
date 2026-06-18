use serde::Deserialize;
use serde::Serialize;

use crate::SessionId;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThreadGoalStatus {
    Active,
    Paused,
    BudgetLimited,
    Complete,
}

pub const MAX_THREAD_GOAL_OBJECTIVE_CHARS: usize = 4_000;

pub fn validate_thread_goal_objective(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("goal objective must not be empty".to_string());
    }
    if value.chars().nth(MAX_THREAD_GOAL_OBJECTIVE_CHARS).is_some() {
        return Err(format!(
            "goal objective must be at most {MAX_THREAD_GOAL_OBJECTIVE_CHARS} characters"
        ));
    }
    Ok(())
}

pub fn validate_thread_goal_token_budget(value: Option<i64>) -> Result<(), String> {
    if let Some(value) = value
        && value <= 0
    {
        return Err("goal budgets must be positive when provided".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoal {
    pub thread_id: SessionId,
    pub objective: String,
    pub status: ThreadGoalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalCreateParams {
    pub session_id: SessionId,
    pub objective: String,
    #[serde(default)]
    pub token_budget: Option<i64>,
    #[serde(default)]
    pub replace_existing: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalCreateResult {
    pub goal: ThreadGoal,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalSetParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ThreadGoalStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalSetResult {
    pub goal: ThreadGoal,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalStatusParams {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalStatusResult {
    pub goal: Option<ThreadGoal>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalSetStatusParams {
    pub session_id: SessionId,
    pub status: ThreadGoalStatus,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalSetStatusResult {
    pub goal: ThreadGoal,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalClearParams {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalClearResult {
    pub cleared: bool,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::{
        MAX_THREAD_GOAL_OBJECTIVE_CHARS, validate_thread_goal_objective,
        validate_thread_goal_token_budget,
    };

    #[test]
    fn objective_validation_accepts_exact_character_limit() {
        let objective = "a".repeat(MAX_THREAD_GOAL_OBJECTIVE_CHARS);

        assert_eq!(validate_thread_goal_objective(&objective), Ok(()));
    }

    #[test]
    fn objective_validation_rejects_first_character_over_limit() {
        let objective = "a".repeat(MAX_THREAD_GOAL_OBJECTIVE_CHARS + 1);

        assert_eq!(
            validate_thread_goal_objective(&objective),
            Err(format!(
                "goal objective must be at most {MAX_THREAD_GOAL_OBJECTIVE_CHARS} characters"
            ))
        );
    }

    #[test]
    fn token_budget_validation_accepts_absent_or_positive_budget() {
        assert_eq!(validate_thread_goal_token_budget(None), Ok(()));
        assert_eq!(validate_thread_goal_token_budget(Some(1)), Ok(()));
    }
}
