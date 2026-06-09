/// Commands that can be invoked by starting a message with a leading slash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlashCommand {
    Theme,
    Model,
    Skills,
    Mcp,
    Compact,
    Resume,
    New,
    Status,
    Permissions,
    Clear,
    Diff,
    Exit,
    Btw,
    Goal,
}

impl SlashCommand {
    pub fn description(self) -> &'static str {
        match self {
            SlashCommand::Theme => "switch the UI theme",
            SlashCommand::Model => "choose the active model",
            SlashCommand::Skills => "show available skills",
            SlashCommand::Mcp => "show configured MCP servers",
            SlashCommand::Compact => "compact the current session context",
            SlashCommand::Resume => "resume a saved chat",
            SlashCommand::New => "start a new chat",
            SlashCommand::Status => "show current session configuration and token usage",
            SlashCommand::Permissions => "choose what Devo is allowed to do",
            SlashCommand::Clear => "clear the current transcript",
            SlashCommand::Diff => "show git diff (including untracked files)",
            SlashCommand::Btw => "inject text into the current turn immediately",
            SlashCommand::Goal => "set or view the goal for a long-running task",
            SlashCommand::Exit => "exit Devo",
        }
    }

    pub fn command(self) -> &'static str {
        match self {
            SlashCommand::Theme => "theme",
            SlashCommand::Model => "model",
            SlashCommand::Skills => "skills",
            SlashCommand::Mcp => "mcp",
            SlashCommand::Compact => "compact",
            SlashCommand::Resume => "resume",
            SlashCommand::New => "new",
            SlashCommand::Status => "status",
            SlashCommand::Permissions => "permissions",
            SlashCommand::Clear => "clear",
            SlashCommand::Diff => "diff",
            SlashCommand::Btw => "btw",
            SlashCommand::Goal => "goal",
            SlashCommand::Exit => "exit",
        }
    }

    pub fn supports_inline_args(self) -> bool {
        matches!(
            self,
            SlashCommand::Model | SlashCommand::Btw | SlashCommand::Goal
        )
    }

    pub fn parameter_hint(self) -> Option<&'static str> {
        match self {
            SlashCommand::Btw => Some("<side conversation message>"),
            SlashCommand::Goal => Some("<objective for autonomous work>"),
            SlashCommand::Theme
            | SlashCommand::Model
            | SlashCommand::Skills
            | SlashCommand::Mcp
            | SlashCommand::Compact
            | SlashCommand::Resume
            | SlashCommand::New
            | SlashCommand::Status
            | SlashCommand::Permissions
            | SlashCommand::Clear
            | SlashCommand::Diff
            | SlashCommand::Exit => None,
        }
    }

    pub fn available_during_task(self) -> bool {
        !matches!(
            self,
            SlashCommand::Model
                | SlashCommand::Theme
                | SlashCommand::Compact
                | SlashCommand::Diff
                | SlashCommand::New
                | SlashCommand::Resume
        )
    }
}

impl std::str::FromStr for SlashCommand {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "theme" => Ok(Self::Theme),
            "model" => Ok(Self::Model),
            "skills" => Ok(Self::Skills),
            "mcp" => Ok(Self::Mcp),
            "compact" => Ok(Self::Compact),
            "resume" => Ok(Self::Resume),
            "new" => Ok(Self::New),
            "status" => Ok(Self::Status),
            "permissions" | "approvals" => Ok(Self::Permissions),
            "clear" => Ok(Self::Clear),
            "diff" => Ok(Self::Diff),
            "btw" => Ok(Self::Btw),
            "goal" => Ok(Self::Goal),
            "exit" => Ok(Self::Exit),
            _ => Err(()),
        }
    }
}

pub fn built_in_slash_commands() -> Vec<(&'static str, SlashCommand)> {
    vec![
        ("theme", SlashCommand::Theme),
        ("model", SlashCommand::Model),
        ("skills", SlashCommand::Skills),
        ("mcp", SlashCommand::Mcp),
        ("compact", SlashCommand::Compact),
        ("resume", SlashCommand::Resume),
        ("new", SlashCommand::New),
        ("status", SlashCommand::Status),
        ("permissions", SlashCommand::Permissions),
        ("clear", SlashCommand::Clear),
        ("diff", SlashCommand::Diff),
        ("goal", SlashCommand::Goal),
        ("btw", SlashCommand::Btw),
        ("exit", SlashCommand::Exit),
    ]
}

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
    fn agents_slash_command_is_not_available() {
        assert_eq!("agents".parse::<SlashCommand>(), Err(()));
        assert!(
            !built_in_slash_commands()
                .iter()
                .any(|(name, _command)| *name == "agents")
        );
    }
}
