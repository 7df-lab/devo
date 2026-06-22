use std::str::FromStr;

use crate::AcpAvailableCommand;
use crate::AcpAvailableCommandInput;

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
    Research,
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
            SlashCommand::Btw => {
                "Ask a quick side question without interrupting the main conversation"
            }
            SlashCommand::Goal => "set or view the goal for a long-running task",
            SlashCommand::Research => "run a deep research workflow",
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
            SlashCommand::Research => "research",
            SlashCommand::Exit => "exit",
        }
    }

    pub fn supports_inline_args(self) -> bool {
        matches!(
            self,
            SlashCommand::Model | SlashCommand::Btw | SlashCommand::Goal | SlashCommand::Research
        )
    }

    pub fn parameter_hint(self) -> Option<&'static str> {
        match self {
            SlashCommand::Btw => Some("<side conversation message>"),
            SlashCommand::Goal => Some("<objective for autonomous work>"),
            SlashCommand::Research => Some("<research question>"),
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
                | SlashCommand::Research
                | SlashCommand::Resume
        )
    }

    pub fn available_over_acp(self) -> bool {
        matches!(
            self,
            SlashCommand::Compact | SlashCommand::Goal | SlashCommand::Research
        )
    }

    fn acp_input_hint(self) -> Option<&'static str> {
        match self {
            SlashCommand::Goal => Some("objective, pause, resume, or clear"),
            SlashCommand::Research => Some("research question"),
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
            | SlashCommand::Exit
            | SlashCommand::Btw => None,
        }
    }
}

impl FromStr for SlashCommand {
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
            "research" => Ok(Self::Research),
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
        ("research", SlashCommand::Research),
        ("btw", SlashCommand::Btw),
        ("exit", SlashCommand::Exit),
    ]
}

pub fn acp_slash_commands() -> Vec<SlashCommand> {
    vec![
        SlashCommand::Compact,
        SlashCommand::Goal,
        SlashCommand::Research,
    ]
}

pub fn acp_available_slash_commands() -> Vec<AcpAvailableCommand> {
    acp_slash_commands()
        .into_iter()
        .map(|command| AcpAvailableCommand {
            name: command.command().to_string(),
            description: command.description().to_string(),
            input: command
                .acp_input_hint()
                .map(|hint| AcpAvailableCommandInput {
                    hint: hint.to_string(),
                    meta: None,
                }),
            meta: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn acp_slash_commands_export_server_backed_subset() {
        assert_eq!(
            acp_slash_commands(),
            vec![
                SlashCommand::Compact,
                SlashCommand::Goal,
                SlashCommand::Research
            ]
        );
        assert_eq!(
            acp_available_slash_commands(),
            vec![
                AcpAvailableCommand {
                    name: "compact".to_string(),
                    description: "compact the current session context".to_string(),
                    input: None,
                    meta: None,
                },
                AcpAvailableCommand {
                    name: "goal".to_string(),
                    description: "set or view the goal for a long-running task".to_string(),
                    input: Some(AcpAvailableCommandInput {
                        hint: "objective, pause, resume, or clear".to_string(),
                        meta: None,
                    }),
                    meta: None,
                },
                AcpAvailableCommand {
                    name: "research".to_string(),
                    description: "run a deep research workflow".to_string(),
                    input: Some(AcpAvailableCommandInput {
                        hint: "research question".to_string(),
                        meta: None,
                    }),
                    meta: None,
                },
            ]
        );
    }

    #[test]
    fn tui_only_slash_commands_are_not_available_over_acp() {
        assert!(!SlashCommand::Theme.available_over_acp());
        assert!(!SlashCommand::Model.available_over_acp());
        assert!(!SlashCommand::Btw.available_over_acp());
        assert!(!SlashCommand::Exit.available_over_acp());
    }
}
