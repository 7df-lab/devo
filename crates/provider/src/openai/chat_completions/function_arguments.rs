use anyhow::Context;
use anyhow::Result;
use devo_protocol::StopReason;
use serde_json::Value;
use serde_json::json;

pub(super) fn parse(arguments: &str, stop_reason: Option<&StopReason>) -> Result<Value> {
    let output_limit_reached = stop_reason == Some(&StopReason::MaxTokens);
    // Parameterless calls from compatible servers may omit arguments, but an
    // explicit token-limit finish is not evidence that an empty call is complete.
    if arguments.trim().is_empty() && !output_limit_reached {
        return Ok(json!({}));
    }
    serde_json::from_str(arguments).with_context(|| {
        if output_limit_reached {
            "invalid JSON in openai function call arguments; response reached its output limit (finish_reason=length); invalid tool call rejected".to_string()
        } else {
            "invalid JSON in openai function call arguments".to_string()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn preserves_complete_and_parameterless_calls() {
        for stop_reason in [
            None,
            Some(&StopReason::ToolUse),
            Some(&StopReason::MaxTokens),
        ] {
            assert_eq!(parse(r#"{"x":1}"#, stop_reason).unwrap(), json!({"x":1}));
        }
        assert_eq!(parse("", None).unwrap(), json!({}));
        assert_eq!(parse("  ", Some(&StopReason::ToolUse)).unwrap(), json!({}));
    }

    #[test]
    fn never_repairs_invalid_nonempty_arguments() {
        for stop_reason in [
            None,
            Some(&StopReason::ToolUse),
            Some(&StopReason::MaxTokens),
        ] {
            for arguments in ["{", "{broken}", r#"{"x":"secret""#] {
                let error = parse(arguments, stop_reason).unwrap_err();
                assert!(!error.to_string().contains("secret"));
            }
        }
    }
}
