const BUILD_PROMPT_TEMPLATE: &str = include_str!("../prompts/collaboration-mode/build.md");
const PLAN_PROMPT_TEMPLATE: &str = include_str!("../prompts/collaboration-mode/plan.md");

pub(crate) fn mode_introductions_prompt() -> String {
    format!(
        "<collaboration_mode_introduction>\n{}\n\n{}\n</collaboration_mode_introduction>",
        BUILD_PROMPT_TEMPLATE.trim_end(),
        PLAN_PROMPT_TEMPLATE.trim_end()
    )
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn mode_prompt_renders_all_mode_introductions() {
        assert_eq!(
            mode_introductions_prompt(),
            format!(
                "<collaboration_mode_introduction>\n{}\n\n{}\n</collaboration_mode_introduction>",
                include_str!("../prompts/collaboration-mode/build.md").trim_end(),
                include_str!("../prompts/collaboration-mode/plan.md").trim_end()
            )
        );

        let prompt = mode_introductions_prompt();
        assert!(prompt.starts_with("<collaboration_mode_introduction>"));
        assert!(prompt.contains("<collaboration_mode_build>"));
        assert!(prompt.contains("<collaboration_mode_plan>"));
        assert!(prompt.ends_with("</collaboration_mode_introduction>"));
    }
}
