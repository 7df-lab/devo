use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{RequestContent, RequestMessage, Usage};

pub use crate::native::ids::{ItemId, OpaqueId, QueueItemId, SessionId, TurnId};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum SessionTitleState {
    #[default]
    #[serde(alias = "Unset")]
    Unset,
    #[serde(alias = "Provisional", alias = "Generating")]
    Generating,
    #[serde(alias = "Final")]
    Final(SessionTitleFinalSource),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub enum SessionTitleFinalSource {
    ModelGenerated,
    UserRename,
    ExplicitCreate,
    /// Truncated first user message applied before optional LLM polish.
    Heuristic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub enum TurnStatus {
    Pending,
    Running,
    WaitingApproval,
    Interrupted,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TurnUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: Option<u32>,
    pub cache_read_input_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
}

impl TurnUsage {
    pub fn from_usage(usage: &Usage) -> Self {
        Self {
            input_tokens: saturating_u32(usage.input_tokens),
            output_tokens: saturating_u32(usage.output_tokens),
            cache_creation_input_tokens: usage.cache_creation_input_tokens.map(saturating_u32),
            cache_read_input_tokens: usage.cache_read_input_tokens.map(saturating_u32),
            reasoning_output_tokens: usage.reasoning_output_tokens.map(saturating_u32),
            total_tokens: usage.total_tokens.map(saturating_u32),
        }
    }

    pub fn derived_total_tokens(&self) -> usize {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .try_into()
            .unwrap_or(usize::MAX)
    }

    pub fn display_total_tokens(&self) -> usize {
        self.total_tokens
            .map(|tokens| tokens.try_into().unwrap_or(usize::MAX))
            .unwrap_or_else(|| self.derived_total_tokens())
    }

    /// Persistence / ACP boundary only — first-party live paths use Native
    /// [`crate::native::usage::TurnUsage`] directly.
    pub fn to_native(&self) -> crate::native::usage::TurnUsage {
        crate::native::usage::TurnUsage {
            query: crate::native::usage::UsageTotals {
                total_tokens: u64::from(
                    self.total_tokens
                        .unwrap_or(self.input_tokens.saturating_add(self.output_tokens)),
                ),
                input_tokens: u64::from(self.input_tokens),
                output_tokens: u64::from(self.output_tokens),
                reasoning_tokens: u64::from(self.reasoning_output_tokens.unwrap_or(0)),
                cache_read_input_tokens: u64::from(self.cache_read_input_tokens.unwrap_or(0)),
                cache_creation_input_tokens: u64::from(
                    self.cache_creation_input_tokens.unwrap_or(0),
                ),
                call_count: 0,
                metered_call_count: 1,
                ..crate::native::usage::UsageTotals::default()
            },
            overhead: crate::native::usage::UsageTotals::default(),
        }
    }

    /// Migrate-only: inverse of [`Self::to_native`] for packed `TurnRecord`
    /// / fixture inverse projection. Not for live persistence.
    ///
    /// Optional cache/reasoning counters map `0 → None` so round-trips match
    /// packed legacy records that omitted those fields.
    pub fn from_native(usage: &crate::native::usage::TurnUsage) -> Self {
        Self {
            input_tokens: u32::try_from(usage.query.input_tokens).unwrap_or(u32::MAX),
            output_tokens: u32::try_from(usage.query.output_tokens).unwrap_or(u32::MAX),
            cache_creation_input_tokens: (usage.query.cache_creation_input_tokens > 0).then(|| {
                u32::try_from(usage.query.cache_creation_input_tokens).unwrap_or(u32::MAX)
            }),
            cache_read_input_tokens: (usage.query.cache_read_input_tokens > 0)
                .then(|| u32::try_from(usage.query.cache_read_input_tokens).unwrap_or(u32::MAX)),
            reasoning_output_tokens: (usage.query.reasoning_tokens > 0)
                .then(|| u32::try_from(usage.query.reasoning_tokens).unwrap_or(u32::MAX)),
            total_tokens: Some(u32::try_from(usage.query.total_tokens).unwrap_or(u32::MAX)),
        }
    }
}

fn saturating_u32(value: usize) -> u32 {
    value.try_into().unwrap_or(u32::MAX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "reasoning")]
    Reasoning { text: String },
    #[serde(rename = "provider_reasoning")]
    ProviderReasoning {
        provider: String,
        payload: serde_json::Value,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "hosted_tool_use")]
    HostedToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(rename = "image")]
    Image {
        mime_type: String,
        data_base64: String,
    },
}

/// One role-tagged turn in the conversation (session / protocol shape).
///
/// A single `Message` may bundle several [`ContentBlock`]s—text, reasoning,
/// tool use, and tool results—because that matches how a model or user turn is
/// stored and exchanged. History management in `devo-core` uses a flatter
/// `ResponseItem` IR instead; see `message_to_response_items` there for why a
/// mixed assistant message is split into adjacent atomic items and later merged
/// again when building provider requests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn tool_uses(&self) -> Vec<(&str, &str, &serde_json::Value)> {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.as_str(), name.as_str(), input))
                }
                ContentBlock::Text { .. }
                | ContentBlock::Reasoning { .. }
                | ContentBlock::ProviderReasoning { .. }
                | ContentBlock::HostedToolUse { .. }
                | ContentBlock::ToolResult { .. }
                | ContentBlock::Image { .. } => None,
            })
            .collect()
    }

    pub fn to_request_message(&self) -> RequestMessage {
        let content = self
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => RequestContent::Text { text: text.clone() },
                ContentBlock::Reasoning { text } => {
                    RequestContent::Reasoning { text: text.clone() }
                }
                ContentBlock::ProviderReasoning { provider, payload } => {
                    RequestContent::ProviderReasoning {
                        provider: provider.clone(),
                        payload: payload.clone(),
                    }
                }
                ContentBlock::ToolUse { id, name, input } => RequestContent::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                },
                ContentBlock::HostedToolUse {
                    id,
                    name,
                    input,
                    output,
                    status,
                } => RequestContent::HostedToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                    output: output.clone(),
                    status: status.clone(),
                },
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => RequestContent::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: if *is_error { Some(true) } else { None },
                },
                ContentBlock::Image {
                    mime_type,
                    data_base64,
                } => RequestContent::Image {
                    mime_type: mime_type.clone(),
                    data_base64: data_base64.clone(),
                },
            })
            .collect();

        RequestMessage {
            role: self.role.as_str().to_string(),
            content,
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn role_system_as_str() {
        assert_eq!(Role::System.as_str(), "system");
    }

    #[test]
    fn role_user_as_str() {
        assert_eq!(Role::User.as_str(), "user");
    }

    #[test]
    fn role_assistant_as_str() {
        assert_eq!(Role::Assistant.as_str(), "assistant");
    }

    #[test]
    fn message_system_creates_system_role() {
        let msg = Message::system("budget notice");
        assert_eq!(msg.role, Role::System);
        assert_eq!(msg.content.len(), 1);
        assert!(
            matches!(msg.content[0], ContentBlock::Text { ref text } if text == "budget notice")
        );
    }

    #[test]
    fn message_system_to_request_message() {
        let msg = Message::system("system instruction");
        let req = msg.to_request_message();
        assert_eq!(req.role, "system");
    }

    #[test]
    fn turn_usage_from_usage_preserves_provider_total_breakdown() {
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_creation_input_tokens: Some(2),
            cache_read_input_tokens: Some(3),
            reasoning_output_tokens: Some(7),
            total_tokens: Some(20),
        };

        let turn_usage = TurnUsage::from_usage(&usage);

        assert_eq!(
            turn_usage,
            TurnUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: Some(2),
                cache_read_input_tokens: Some(3),
                reasoning_output_tokens: Some(7),
                total_tokens: Some(20),
            }
        );
        assert_eq!(turn_usage.derived_total_tokens(), 15);
        assert_eq!(turn_usage.display_total_tokens(), 20);
    }

    #[test]
    fn message_user_to_request_message_role() {
        let msg = Message::user("hello");
        let req = msg.to_request_message();
        assert_eq!(req.role, "user");
    }
}
