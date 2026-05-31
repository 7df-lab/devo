/// Commands that can be invoked by starting a message with a leading slash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlashCommand {
    Theme,
    Model,
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
            SlashCommand::Compact => "compact the current session context",
            SlashCommand::Resume => "resume a saved chat",
            SlashCommand::New => "start a new chat",
            SlashCommand::Status => "show current session configuration and token usage",
            SlashCommand::Permissions => "choose what Devo is allowed to do",
            SlashCommand::Clear => "clear the current transcript",
            SlashCommand::Diff => "show git diff (including untracked files)",
            SlashCommand::Btw => "inject text into the current turn immediately",
            SlashCommand::Goal => "view and manage the current goal",
            SlashCommand::Exit => "exit Devo",
        }
    }

    pub fn command(self) -> &'static str {
        match self {
            SlashCommand::Theme => "theme",
            SlashCommand::Model => "model",
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
        matches!(self, SlashCommand::Model | SlashCommand::Btw)
    }

    pub fn available_during_task(self) -> bool {
        !matches!(
            self,
            SlashCommand::Model
                | SlashCommand::Theme
                | SlashCommand::Compact
                | SlashCommand::Diff
                | SlashCommand::Goal
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
