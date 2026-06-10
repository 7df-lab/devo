use devo_protocol::{ThreadGoal, ThreadGoalStatus};

const CONTINUATION_TEMPLATE: &str = include_str!("../prompts/goals/continuation.md");
const BUDGET_LIMIT_TEMPLATE: &str = include_str!("../prompts/goals/budget_limit.md");
const OBJECTIVE_UPDATED_TEMPLATE: &str = include_str!("../prompts/goals/objective_updated.md");

pub fn render_goal_continuation_prompt(goal: &ThreadGoal) -> Option<String> {
    match goal.status {
        ThreadGoalStatus::Active if token_budget_exhausted(goal) => {
            Some(render_goal_template(BUDGET_LIMIT_TEMPLATE, goal))
        }
        ThreadGoalStatus::Active => Some(render_goal_template(CONTINUATION_TEMPLATE, goal)),
        ThreadGoalStatus::BudgetLimited => Some(render_goal_template(BUDGET_LIMIT_TEMPLATE, goal)),
        ThreadGoalStatus::Paused | ThreadGoalStatus::Complete => None,
    }
}

pub fn render_goal_budget_limit_prompt(goal: &ThreadGoal) -> String {
    render_goal_template(BUDGET_LIMIT_TEMPLATE, goal)
}

pub fn render_goal_objective_updated_prompt(goal: &ThreadGoal) -> String {
    render_goal_template(OBJECTIVE_UPDATED_TEMPLATE, goal)
}

fn render_goal_template(template: &str, goal: &ThreadGoal) -> String {
    let token_budget = goal
        .token_budget
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());
    let remaining_tokens = goal
        .token_budget
        .map(|value| (value - goal.tokens_used).max(0).to_string())
        .unwrap_or_else(|| "unlimited".to_string());

    template
        .replace("{{ objective }}", &escape_xml_text(&goal.objective))
        .replace("{{ tokens_used }}", &goal.tokens_used.to_string())
        .replace("{{ token_budget }}", &token_budget)
        .replace("{{ remaining_tokens }}", &remaining_tokens)
        .replace(
            "{{ time_used_seconds }}",
            &goal.time_used_seconds.to_string(),
        )
}

fn escape_xml_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn token_budget_exhausted(goal: &ThreadGoal) -> bool {
    goal.token_budget
        .is_some_and(|budget| goal.tokens_used >= budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devo_protocol::SessionId;

    fn active_goal(objective: &str, token_budget: Option<i64>) -> ThreadGoal {
        ThreadGoal {
            thread_id: SessionId::new(),
            objective: objective.to_string(),
            status: ThreadGoalStatus::Active,
            token_budget,
            tokens_used: 17,
            time_used_seconds: 3,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn continuation_prompt_escapes_untrusted_objective_xml() {
        // Trace: L2-DES-GOAL-001
        let goal = active_goal("finish <goal> & report \"done\"", Some(100));

        let prompt = render_goal_continuation_prompt(&goal).expect("active goal prompt");

        assert!(prompt.contains("finish &lt;goal&gt; &amp; report &quot;done&quot;"));
        assert!(!prompt.contains("finish <goal> & report \"done\""));
        assert!(prompt.contains("Completion audit:"));
        assert!(prompt.contains("call update_goal with status \"complete\""));
    }

    #[test]
    fn continuation_prompt_does_not_fabricate_default_budget() {
        // Trace: L2-DES-GOAL-001
        let goal = active_goal("finish goal", None);

        let prompt = render_goal_continuation_prompt(&goal).expect("active goal prompt");

        assert!(prompt.contains("- Token budget: none"));
        assert!(prompt.contains("- Tokens remaining: unlimited"));
    }

    #[test]
    fn exhausted_goal_budget_renders_budget_limit_prompt() {
        // Trace: L2-DES-GOAL-001
        let mut goal = active_goal("finish goal", Some(17));
        goal.tokens_used = 17;

        let prompt = render_goal_continuation_prompt(&goal).expect("budget prompt");

        assert!(prompt.contains("has reached its token budget"));
        assert!(prompt.contains("do not start new substantive work"));
    }
}
