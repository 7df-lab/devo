/// One supported slash command exposed by the interactive composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlashCommandSpec {
    /// Exact slash command text inserted into the composer.
    pub(crate) name: &'static str,
    /// Short human-readable description shown in the suggestion list.
    pub(crate) description: &'static str,
}

/// Canonical slash commands supported by the interactive TUI.
pub(crate) const SLASH_COMMANDS: [SlashCommandSpec; 6] = [
    SlashCommandSpec {
        name: "/model",
        description: "Show or change the active model",
    },
    SlashCommandSpec {
        name: "/new",
        description: "Create a new session",
    },
    SlashCommandSpec {
        name: "/rename",
        description: "Rename the current session",
    },
    SlashCommandSpec {
        name: "/sessions",
        description: "List sessions and switch between them",
    },
    SlashCommandSpec {
        name: "/status",
        description: "Show current session status",
    },
    SlashCommandSpec {
        name: "/exit",
        description: "Exit the interactive session",
    },
];

/// Computes the visible slash-command suggestions for the current composer text.
pub(crate) fn matching_slash_commands(input: &str) -> Vec<SlashCommandSpec> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('/') || trimmed.contains(char::is_whitespace) {
        return Vec::new();
    }

    SLASH_COMMANDS
        .into_iter()
        .filter(|command| command.name.starts_with(trimmed))
        .collect()
}
