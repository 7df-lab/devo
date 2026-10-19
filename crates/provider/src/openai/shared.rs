use devo_protocol::ReasoningEffort;
use devo_protocol::RequestRole;
use devo_protocol::ToolDefinition;
use devo_protocol::Usage;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use tracing::warn;

use super::OpenAIRole;
use super::capabilities::OpenAIReasoningMode;
use super::capabilities::OpenAIRequestProfile;

pub(crate) fn request_role(role: &str) -> OpenAIRole {
    match role.parse::<RequestRole>() {
        Ok(RequestRole::System) => OpenAIRole::System,
        Ok(RequestRole::Developer) => OpenAIRole::Developer,
        Ok(RequestRole::User) => OpenAIRole::User,
        Ok(RequestRole::Assistant) => OpenAIRole::Assistant,
        Ok(RequestRole::Tool) => OpenAIRole::Tool,
        Ok(RequestRole::Function) => OpenAIRole::Function,
        Err(_) => {
            warn!(
                role = role,
                fallback = "user",
                "unknown OpenAI request role; defaulting to user"
            );
            OpenAIRole::User
        }
    }
}

pub(crate) enum OpenAIReasoningValue {
    Effort(ReasoningEffort),
    Thinking {
        enabled: bool,
    },
    ThinkingWithEffort {
        enabled: bool,
        effort: Option<ReasoningEffort>,
    },
}

pub(crate) fn reasoning_value(
    profile: OpenAIRequestProfile,
    thinking: Option<&str>,
    reasoning_effort: Option<ReasoningEffort>,
) -> Option<OpenAIReasoningValue> {
    match profile.reasoning_mode {
        OpenAIReasoningMode::Effort => reasoning_effort.map(OpenAIReasoningValue::Effort),
        OpenAIReasoningMode::Thinking => {
            let enabled = !thinking_is_disabled(thinking);
            Some(OpenAIReasoningValue::Thinking { enabled })
        }
        OpenAIReasoningMode::ThinkingWithEffort => {
            let enabled = !thinking_is_disabled(thinking);
            Some(OpenAIReasoningValue::ThinkingWithEffort {
                enabled,
                effort: if enabled { reasoning_effort } else { None },
            })
        }
    }
}

fn thinking_is_disabled(thinking: Option<&str>) -> bool {
    let Some(thinking) = thinking.map(str::trim) else {
        return false;
    };
    thinking.eq_ignore_ascii_case("disabled") || thinking.eq_ignore_ascii_case("none")
}

pub(crate) fn deserialize_null_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<Vec<T>>::deserialize(deserializer).map(|v| v.unwrap_or_default())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAICompletionUsage {
    pub(super) prompt_tokens: usize,
    pub(super) completion_tokens: usize,
    #[serde(default)]
    pub(super) total_tokens: Option<usize>,
    #[serde(default)]
    pub(super) prompt_tokens_details: Option<OpenAIPromptTokenDetails>,
    #[serde(default)]
    pub(super) completion_tokens_details: Option<OpenAICompletionTokenDetails>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAIPromptTokenDetails {
    #[serde(default)]
    pub(super) audio_tokens: Option<usize>,
    #[serde(default)]
    pub(super) cached_tokens: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAICompletionTokenDetails {
    #[serde(default)]
    pub(super) accepted_prediction_tokens: Option<usize>,
    #[serde(default)]
    pub(super) audio_tokens: Option<usize>,
    #[serde(default)]
    pub(super) reasoning_tokens: Option<usize>,
    #[serde(default)]
    pub(super) rejected_prediction_tokens: Option<usize>,
}

pub(crate) fn completion_usage_to_usage(usage: &OpenAICompletionUsage) -> Usage {
    Usage {
        input_tokens: usage.prompt_tokens,
        output_tokens: usage.completion_tokens,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: usage
            .prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens),
        reasoning_output_tokens: usage
            .completion_tokens_details
            .as_ref()
            .and_then(|details| details.reasoning_tokens),
        total_tokens: usage.total_tokens,
    }
}

#[cfg(test)]
pub(crate) fn completion_usage_from_value(value: &Value) -> Option<Usage> {
    let usage: OpenAICompletionUsage = serde_json::from_value(value.clone()).ok()?;
    Some(completion_usage_to_usage(&usage))
}

pub(crate) fn tool_definitions(tools: &[ToolDefinition]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                let mut function = json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                });
                if let Some(output_schema) = &tool.output_schema {
                    function["output_schema"] = output_schema.clone();
                }
                json!({
                    "type": "function",
                    "function": function
                })
            })
            .collect(),
    )
}

/// OpenAI Responses API function tools use a flat shape (`name` on the tool
/// object), not the Chat Completions nested `function` wrapper.
pub(crate) fn responses_tool_definitions(tools: &[ToolDefinition]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                let mut value = json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                });
                if let Some(output_schema) = &tool.output_schema {
                    value["output_schema"] = output_schema.clone();
                }
                value
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use devo_protocol::Usage;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    #[test]
    fn thinking_disabled_check_is_case_insensitive_without_lowercase_allocation() {
        assert!(thinking_is_disabled(Some(" disabled ")));
        assert!(thinking_is_disabled(Some("NONE")));
        assert!(!thinking_is_disabled(Some("enabled")));
        assert!(!thinking_is_disabled(None));
    }

    #[test]
    fn completion_usage_from_value_reads_chat_completion_usage_shape() {
        let usage = completion_usage_from_value(&json!({
            "prompt_tokens": 11,
            "completion_tokens": 7,
            "total_tokens": 18,
            "prompt_tokens_details": {
                "cached_tokens": 5,
                "audio_tokens": 0
            },
            "completion_tokens_details": {
                "reasoning_tokens": 2,
                "audio_tokens": 0,
                "accepted_prediction_tokens": 1,
                "rejected_prediction_tokens": 0
            }
        }))
        .expect("parse usage");

        assert_eq!(
            usage,
            Usage {
                input_tokens: 11,
                output_tokens: 7,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: Some(5),
                reasoning_output_tokens: Some(2),
                total_tokens: Some(18),
            }
        );
    }
}
