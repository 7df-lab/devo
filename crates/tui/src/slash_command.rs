pub use devo_protocol::SlashCommand;
pub use devo_protocol::built_in_slash_commands;

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn mcp_slash_command_parses_and_is_listed() {
        assert_eq!("mcp".parse::<SlashCommand>(), Ok(SlashCommand::Mcp));
        assert!(
            built_in_slash_commands()
                .iter()
                .any(|(name, command)| *name == "mcp" && *command == SlashCommand::Mcp)
        );
    }

    #[test]
    fn goal_slash_command_parses_and_accepts_inline_args_during_tasks() {
        assert_eq!("goal".parse::<SlashCommand>(), Ok(SlashCommand::Goal));
        assert!(SlashCommand::Goal.supports_inline_args());
        assert!(SlashCommand::Goal.available_during_task());
        assert_eq!(
            SlashCommand::Goal.parameter_hint(),
            Some("<objective for autonomous work>")
        );
        assert!(
            built_in_slash_commands()
                .iter()
                .any(|(name, command)| *name == "goal" && *command == SlashCommand::Goal)
        );
    }

    #[test]
    fn research_slash_command_parses_and_accepts_inline_args() {
        // Trace: L2-DES-RESEARCH-001
        // Verifies: /research is discoverable and accepts a question parameter.
        assert_eq!(
            "research".parse::<SlashCommand>(),
            Ok(SlashCommand::Research)
        );
        assert!(SlashCommand::Research.supports_inline_args());
        assert!(!SlashCommand::Research.available_during_task());
        assert_eq!(
            SlashCommand::Research.parameter_hint(),
            Some("<research question>")
        );
        assert!(
            built_in_slash_commands()
                .iter()
                .any(|(name, command)| *name == "research" && *command == SlashCommand::Research)
        );
    }

    #[test]
    fn agents_slash_command_is_not_available() {
        assert_eq!("agents".parse::<SlashCommand>(), Err(()));
        assert!(
            !built_in_slash_commands()
                .iter()
                .any(|(name, _command)| *name == "agents")
        );
    }
}
