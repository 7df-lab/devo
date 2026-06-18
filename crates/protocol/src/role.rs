use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A normalized message role used by provider adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestRole {
    System,
    Developer,
    User,
    Assistant,
    Tool,
    Function,
}

impl RequestRole {
    /// Returns the stable wire label for this role.
    pub fn as_str(self) -> &'static str {
        match self {
            RequestRole::System => "system",
            RequestRole::Developer => "developer",
            RequestRole::User => "user",
            RequestRole::Assistant => "assistant",
            RequestRole::Tool => "tool",
            RequestRole::Function => "function",
        }
    }
}

impl fmt::Display for RequestRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RequestRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("system") {
            Ok(RequestRole::System)
        } else if s.eq_ignore_ascii_case("developer") {
            Ok(RequestRole::Developer)
        } else if s.eq_ignore_ascii_case("user") {
            Ok(RequestRole::User)
        } else if s.eq_ignore_ascii_case("assistant") {
            Ok(RequestRole::Assistant)
        } else if s.eq_ignore_ascii_case("tool") {
            Ok(RequestRole::Tool)
        } else if s.eq_ignore_ascii_case("function") {
            Ok(RequestRole::Function)
        } else {
            Err(format!("invalid request role: {}", s.to_ascii_lowercase()))
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn request_role_parses_case_insensitive_trimmed_values() {
        assert_eq!(
            " ASSISTANT ".parse::<RequestRole>(),
            Ok(RequestRole::Assistant)
        );
        assert_eq!(
            "developer".parse::<RequestRole>(),
            Ok(RequestRole::Developer)
        );
    }

    #[test]
    fn request_role_error_keeps_normalized_value() {
        assert_eq!(
            "CUSTOM".parse::<RequestRole>(),
            Err("invalid request role: custom".to_string())
        );
    }
}
