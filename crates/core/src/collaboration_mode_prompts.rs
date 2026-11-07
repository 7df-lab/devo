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

    #[test]
    fn build_assumptions_do_not_replace_approval() {
        let prompt = mode_introductions_prompt(devo_protocol::CollaborationMode::Build);
        assert_eq!(prompt.matches("reasonable assumptions").count(), 1);
        assert!(prompt.contains("Assumptions never replace required permission or approval"));
        assert!(prompt.contains("goal, target environment, authorization, or compatibility"));
        assert!(prompt.contains("low-risk, reversible implementation details"));
        assert!(!prompt.contains("at least one"));
        assert!(prompt.contains("await rlm.update_plan"));
        assert!(!prompt.contains("consider it accepted"));
        assert!(!prompt.contains("no more than 60 seconds"));
    }

    #[test]
    fn plan_preserves_no_mutation_and_finalization_contracts() {
        let prompt = mode_introductions_prompt(devo_protocol::CollaborationMode::Plan);
        for constraint in [
            "<collaboration_mode><current>plan</current>",
            "do not change the mode",
            "do not edit or write source files",
            "must not update tracked snapshots",
            "do not mutate the session plan",
            "await rlm.request_user_input",
            "Do not treat silence as permission",
            "at most one `<proposed_plan>` block per turn",
            "replace the entire previous plan",
            "final response must be plan-only",
        ] {
            assert!(
                prompt.contains(constraint),
                "missing constraint: {constraint}"
            );
        }
    }
}
