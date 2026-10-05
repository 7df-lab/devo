//! DTOs for the current in-tree Pi agent-core and pi-ai message/event wire shapes.
//!
//! This module is intentionally isolated until Native event projection is wired.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use ts_rs::TS;

/// The core Pi message union. Custom AgentMessage extensions are intentionally
/// excluded; this represents the in-tree pi-ai `Message` core only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "role", rename_all = "camelCase")]
#[ts(tag = "role")]
pub enum AgentMessage {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UserMessage {
    pub content: UserContent,
    pub timestamp: f64,
}

/// Pi accepts either a plain user string or text/image content blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<UserContentBlock>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type")]
pub enum UserContentBlock {
    Text(TextContent),
    Image(ImageContent),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub content: Vec<AssistantContentBlock>,
    /// Pi's `Api` and `Provider` types both accept arbitrary strings.
    pub api: String,
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<AssistantMessageDiagnostic>>,
    pub usage: Usage,
    pub stop_reason: StopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason_raw: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub timestamp: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type")]
pub enum AssistantContentBlock {
    Text(TextContent),
    Thinking(ThinkingContent),
    ToolCall(ToolCallData),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TextContent {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingContent {
    pub thinking: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ImageContent {
    /// Base64-encoded image content, as in Pi's `ImageContent`.
    pub data: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallData {
    pub id: String,
    pub name: String,
    /// Pi declares tool-call arguments as an open string-keyed object.
    pub arguments: std::collections::BTreeMap<String, JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

/// A Pi `ToolCall` content item, including its required JSON discriminator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type")]
pub enum ToolCall {
    ToolCall(ToolCallData),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[ts(type = "number")]
    pub input: u64,
    #[ts(type = "number")]
    pub output: u64,
    #[ts(type = "number")]
    pub cache_read: u64,
    #[ts(type = "number")]
    pub cache_write: u64,
    #[ts(type = "number")]
    pub total_tokens: u64,
    pub cost: UsageCost,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessageDiagnostic {
    #[serde(rename = "type")]
    #[ts(rename = "type")]
    pub diagnostic_type: String,
    pub timestamp: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<DiagnosticErrorInfo>,
    /// Pi intentionally leaves diagnostic details open-ended (`unknown`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<JsonValue>,
}

// Pi names this field `type`; avoid Rust's reserved keyword without changing
// the wire or TypeScript property name.
impl AssistantMessageDiagnostic {
    // Keep constructor-free DTOs; the serde/TS rename is applied on the field.
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticErrorInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<DiagnosticErrorCode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum DiagnosticErrorCode {
    Number(f64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<UserContentBlock>,
    /// Pi's generic details parameter defaults to `any`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<JsonValue>,
    pub is_error: bool,
    pub timestamp: f64,
}

/// Pi's agent-loop lifecycle and tool-execution event union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[ts(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    AgentStart,
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
    TurnStart,
    TurnEnd {
        message: AgentMessage,
        tool_results: Vec<ToolResultMessage>,
    },
    MessageStart {
        message: AgentMessage,
    },
    MessageUpdate {
        message: AgentMessage,
        assistant_message_event: Box<AssistantMessageEvent>,
    },
    MessageEnd {
        message: AgentMessage,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        /// Pi types this payload as `any`.
        args: JsonValue,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        /// Pi types these payloads as `any`.
        args: JsonValue,
        partial_result: JsonValue,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        /// Pi types this payload as `any`.
        result: JsonValue,
        is_error: bool,
    },
}

/// Nested pi-ai streaming events carried by `AgentEvent::MessageUpdate`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[ts(tag = "type", rename_all = "snake_case")]
pub enum AssistantMessageEvent {
    Start {
        partial: AssistantMessage,
    },
    TextStart {
        #[ts(type = "number")]
        content_index: u64,
        partial: AssistantMessage,
    },
    TextDelta {
        #[ts(type = "number")]
        content_index: u64,
        delta: String,
        partial: AssistantMessage,
    },
    TextEnd {
        #[ts(type = "number")]
        content_index: u64,
        content: String,
        partial: AssistantMessage,
    },
    ThinkingStart {
        #[ts(type = "number")]
        content_index: u64,
        partial: AssistantMessage,
    },
    ThinkingDelta {
        #[ts(type = "number")]
        content_index: u64,
        delta: String,
        partial: AssistantMessage,
    },
    ThinkingEnd {
        #[ts(type = "number")]
        content_index: u64,
        content: String,
        partial: AssistantMessage,
    },
    ToolcallStart {
        #[ts(type = "number")]
        content_index: u64,
        partial: AssistantMessage,
    },
    ToolcallDelta {
        #[ts(type = "number")]
        content_index: u64,
        delta: String,
        partial: AssistantMessage,
    },
    ToolcallEnd {
        #[ts(type = "number")]
        content_index: u64,
        tool_call: ToolCall,
        partial: AssistantMessage,
    },
    Done {
        reason: SuccessfulStopReason,
        message: AssistantMessage,
    },
    Error {
        reason: FailedStopReason,
        error: AssistantMessage,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum SuccessfulStopReason {
    Stop,
    Length,
    ToolUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum FailedStopReason {
    Aborted,
    Error,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    fn usage() -> Usage {
        Usage {
            input: 3,
            output: 2,
            cache_read: 1,
            cache_write: 0,
            total_tokens: 5,
            cost: UsageCost {
                input: 0.3,
                output: 0.2,
                cache_read: 0.1,
                cache_write: 0.0,
                total: 0.6,
            },
        }
    }

    fn assistant() -> AssistantMessage {
        AssistantMessage {
            content: vec![AssistantContentBlock::Text(TextContent {
                text: "hello".into(),
                text_signature: None,
            })],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-test".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: usage(),
            stop_reason: StopReason::Stop,
            stop_reason_raw: None,
            error_message: None,
            timestamp: 1234.0,
        }
    }

    #[test]
    fn message_union_serializes_role_and_content_discriminants() {
        let message = AgentMessage::Assistant(assistant());
        let value = serde_json::to_value(message).unwrap();
        assert_eq!(value["role"], "assistant");
        assert_eq!(value["content"][0]["type"], "text");
        assert_eq!(value["stopReason"], "stop");
        assert!(value.get("responseModel").is_none());

        let user = AgentMessage::User(UserMessage {
            content: UserContent::Blocks(vec![UserContentBlock::Image(ImageContent {
                data: "aW1hZ2U=".into(),
                mime_type: "image/png".into(),
            })]),
            timestamp: 10.0,
        });
        let value = serde_json::to_value(user).unwrap();
        assert_eq!(value["role"], "user");
        assert_eq!(value["content"][0]["type"], "image");
        assert_eq!(value["content"][0]["mimeType"], "image/png");

        let result = AgentMessage::ToolResult(ToolResultMessage {
            tool_call_id: "call-1".into(),
            tool_name: "lookup".into(),
            content: vec![UserContentBlock::Text(TextContent {
                text: "found".into(),
                text_signature: None,
            })],
            details: Some(json!({"extension": [1, true]})),
            is_error: false,
            timestamp: 11.0,
        });
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["role"], "toolResult");
        assert_eq!(value["toolCallId"], "call-1");
        assert_eq!(value["details"]["extension"], json!([1, true]));
    }

    #[test]
    fn agent_and_nested_assistant_events_use_pi_discriminants() {
        let event = AgentEvent::ToolExecutionUpdate {
            tool_call_id: "call-2".into(),
            tool_name: "lookup".into(),
            args: json!({"query": "rust"}),
            partial_result: json!({"items": ["a"]}),
        };
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({
                "type": "tool_execution_update",
                "toolCallId": "call-2",
                "toolName": "lookup",
                "args": {"query": "rust"},
                "partialResult": {"items": ["a"]}
            })
        );

        let event = AgentEvent::MessageUpdate {
            message: AgentMessage::Assistant(assistant()),
            assistant_message_event: Box::new(AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: " world".into(),
                partial: assistant(),
            }),
        };
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["type"], "message_update");
        assert_eq!(value["assistantMessageEvent"]["type"], "text_delta");
        assert_eq!(value["assistantMessageEvent"]["contentIndex"], 0);
        assert_eq!(value["assistantMessageEvent"]["delta"], " world");

        let tool_call = AssistantMessageEvent::ToolcallEnd {
            content_index: 1,
            tool_call: ToolCall::ToolCall(ToolCallData {
                id: "call-3".into(),
                name: "lookup".into(),
                arguments: std::collections::BTreeMap::from([("q".into(), json!("rust"))]),
                thought_signature: None,
            }),
            partial: assistant(),
        };
        let value = serde_json::to_value(tool_call).unwrap();
        assert_eq!(value["toolCall"]["type"], "toolCall");
        assert_eq!(value["toolCall"]["arguments"]["q"], "rust");

        let done = AssistantMessageEvent::Done {
            reason: SuccessfulStopReason::ToolUse,
            message: assistant(),
        };
        let value = serde_json::to_value(done).unwrap();
        assert_eq!(value["type"], "done");
        assert_eq!(value["reason"], "toolUse");
        assert!(value.get("message").is_some());
    }
}
