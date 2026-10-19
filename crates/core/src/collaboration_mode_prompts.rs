const BUILD_PROMPT_TEMPLATE: &str = include_str!("../prompts/collaboration-mode/build.md");
const PLAN_PROMPT_TEMPLATE: &str = include_str!("../prompts/collaboration-mode/plan.md");

pub(crate) fn mode_introductions_prompt(mode: devo_protocol::CollaborationMode) -> String {
    let introduction = match mode {
        devo_protocol::CollaborationMode::Build => BUILD_PROMPT_TEMPLATE,
        devo_protocol::CollaborationMode::Plan => PLAN_PROMPT_TEMPLATE,
    };
    format!(
        "<collaboration_mode_introduction>\n{}\n</collaboration_mode_introduction>",
        introduction.trim_end()
    )
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn mode_prompt_renders_only_the_active_mode_introduction() {
        for (mode, introduction, inactive_introduction) in [
            (
                devo_protocol::CollaborationMode::Build,
                include_str!("../prompts/collaboration-mode/build.md").trim_end(),
                "<collaboration_mode_plan>",
            ),
            (
                devo_protocol::CollaborationMode::Plan,
                include_str!("../prompts/collaboration-mode/plan.md").trim_end(),
                "<collaboration_mode_build>",
            ),
        ] {
            let prompt = mode_introductions_prompt(mode);
            assert_eq!(
                prompt,
                format!(
                    "<collaboration_mode_introduction>\n{introduction}\n</collaboration_mode_introduction>"
                )
            );
            assert!(prompt.starts_with("<collaboration_mode_introduction>"));
            assert!(prompt.contains(introduction));
            assert!(!prompt.contains(inactive_introduction));
            assert!(prompt.ends_with("</collaboration_mode_introduction>"));
        }
    }

    /// Trace: prompt-payload audit (R27)
    /// Verifies: no sentence is stated twice verbatim — the "Assumptions-first
    /// execution" section used to repeat the grouping rule and the
    /// no-reaction-means-accepted rule that the "Use reasonable assumptions"
    /// principle already covers.
    #[test]
    fn mode_prompt_states_each_assumption_rule_once() {
        let prompt = mode_introductions_prompt(devo_protocol::CollaborationMode::Build);
        for sentence in [
            "If the user does not react to a proposed suggestion, consider it accepted.",
            "Group your assumptions logically",
        ] {
            assert_eq!(
                prompt.matches(sentence).count(),
                1,
                "sentence stated more than once: {sentence}"
            );
        }
    }
}
