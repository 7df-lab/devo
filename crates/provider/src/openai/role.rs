use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// OpenAI chat-completion roles supported by the wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenAIRole {
    System,
    Developer,
    User,
    Assistant,
    Tool,
    Function,
}

impl fmt::Display for OpenAIRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            OpenAIRole::System => "system",
            OpenAIRole::Developer => "developer",
            OpenAIRole::User => "user",
            OpenAIRole::Assistant => "assistant",
            OpenAIRole::Tool => "tool",
            OpenAIRole::Function => "function",
        })
    }
}

impl FromStr for OpenAIRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("system") {
            Ok(OpenAIRole::System)
        } else if s.eq_ignore_ascii_case("developer") {
            Ok(OpenAIRole::Developer)
        } else if s.eq_ignore_ascii_case("user") {
            Ok(OpenAIRole::User)
        } else if s.eq_ignore_ascii_case("assistant") {
            Ok(OpenAIRole::Assistant)
        } else if s.eq_ignore_ascii_case("tool") {
            Ok(OpenAIRole::Tool)
        } else if s.eq_ignore_ascii_case("function") {
            Ok(OpenAIRole::Function)
        } else {
            Err(format!("invalid OpenAI role: {}", s.to_ascii_lowercase()))
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn openai_role_parses_case_insensitive_trimmed_values() {
        assert_eq!(
            " ASSISTANT ".parse::<OpenAIRole>(),
            Ok(OpenAIRole::Assistant)
        );
        assert_eq!("developer".parse::<OpenAIRole>(), Ok(OpenAIRole::Developer));
    }

    #[test]
    fn openai_role_error_keeps_normalized_value() {
        assert_eq!(
            "CUSTOM".parse::<OpenAIRole>(),
            Err("invalid OpenAI role: custom".to_string())
        );
    }
}
