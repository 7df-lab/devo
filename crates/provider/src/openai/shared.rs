use devo_protocol::ReasoningEffort;
use devo_protocol::RequestRole;
use devo_protocol::ToolDefinition;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_disabled_check_is_case_insensitive_without_lowercase_allocation() {
        assert!(thinking_is_disabled(Some(" disabled ")));
        assert!(thinking_is_disabled(Some("NONE")));
        assert!(!thinking_is_disabled(Some("enabled")));
        assert!(!thinking_is_disabled(None));
    }
}
