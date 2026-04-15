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
        match s.trim().to_ascii_lowercase().as_str() {
            "system" => Ok(RequestRole::System),
            "developer" => Ok(RequestRole::Developer),
            "user" => Ok(RequestRole::User),
            "assistant" => Ok(RequestRole::Assistant),
            "tool" => Ok(RequestRole::Tool),
            "function" => Ok(RequestRole::Function),
            other => Err(format!("invalid request role: {other}")),
        }
    }
}
