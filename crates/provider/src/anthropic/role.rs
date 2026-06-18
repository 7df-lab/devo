use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Anthropic Messages API roles supported by the wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicAIRole {
    User,
    Assistant,
}

impl fmt::Display for AnthropicAIRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AnthropicAIRole::User => "user",
            AnthropicAIRole::Assistant => "assistant",
        })
    }
}

impl FromStr for AnthropicAIRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("user") {
            Ok(AnthropicAIRole::User)
        } else if s.eq_ignore_ascii_case("assistant") {
            Ok(AnthropicAIRole::Assistant)
        } else {
            Err(format!(
                "invalid Anthropic role: {}",
                s.to_ascii_lowercase()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn anthropic_role_parses_case_insensitive_trimmed_values() {
        assert_eq!(
            " ASSISTANT ".parse::<AnthropicAIRole>(),
            Ok(AnthropicAIRole::Assistant)
        );
        assert_eq!("user".parse::<AnthropicAIRole>(), Ok(AnthropicAIRole::User));
    }

    #[test]
    fn anthropic_role_error_keeps_normalized_value() {
        assert_eq!(
            "SYSTEM".parse::<AnthropicAIRole>(),
            Err("invalid Anthropic role: system".to_string())
        );
    }
}
