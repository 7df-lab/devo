use anyhow::{Result, ensure};
use devo_protocol::{ModelRequest, ReasoningEffort};
use serde_json::{Value, json};

use super::{AnthropicOutputConfig, AnthropicThinkingConfig};

pub(super) fn build_thinking(level: &str) -> Option<AnthropicThinkingConfig> {
    match level.trim().to_ascii_lowercase().as_str() {
        "" | "default" => None,
        "disabled" => Some(AnthropicThinkingConfig { r#type: "disabled" }),
        "adaptive" => Some(AnthropicThinkingConfig { r#type: "adaptive" }),
        "between_tools" => Some(AnthropicThinkingConfig {
            r#type: "between_tools",
        }),
        "enabled" | "low" | "medium" | "high" | "xhigh" | "max" => {
            Some(AnthropicThinkingConfig { r#type: "enabled" })
        }
        _ => None,
    }
}

pub(super) fn build_output_config(
    thinking: Option<&str>,
    reasoning_effort: Option<ReasoningEffort>,
) -> Option<AnthropicOutputConfig> {
    if matches!(
        thinking
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "" | "default" | "disabled"
    ) {
        return None;
    }

    let effort = match reasoning_effort {
        Some(ReasoningEffort::Low) => "low",
        Some(ReasoningEffort::Medium) => "medium",
        Some(ReasoningEffort::High) => "high",
        Some(ReasoningEffort::XHigh) => "xhigh",
        Some(ReasoningEffort::Max) => "max",
        Some(ReasoningEffort::None | ReasoningEffort::Minimal) => return None,
        None => match thinking?.trim().to_ascii_lowercase().as_str() {
            "enabled" => "high",
            "low" => "low",
            "medium" => "medium",
            "high" => "high",
            "xhigh" => "xhigh",
            "max" => "max",
            _ => return None,
        },
    };

    Some(AnthropicOutputConfig { effort })
}

/// Translate the shared thinking controls to Claude's model-specific contract.
/// Compatible providers such as DeepSeek retain their existing wire options.
/// Explicit extra_body overrides are merged by the caller after these defaults.
pub(super) fn apply_claude_options(root: &mut Value, request: &ModelRequest) -> Result<()> {
    let Some(model) = request.model.strip_prefix("claude-") else {
        return Ok(());
    };
    let mut parts = model.split('-');
    let family = parts.next().unwrap_or_default();
    let major = parts.next().and_then(|value| value.parse::<u32>().ok());
    let minor = parts
        .next()
        .filter(|value| value.len() < 4)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let manual_thinking = match (family, major) {
        ("opus" | "sonnet", Some(major)) => major < 4 || (major == 4 && minor < 6),
        ("haiku", Some(major)) => major < 5,
        // Older model IDs put the version before the model family.
        ("3", _) => true,
        _ => false,
    };
    let supports_effort = !manual_thinking || (family == "opus" && major == Some(4) && minor == 5);
    let supports_xhigh = !manual_thinking
        && !(major == Some(4) && minor == 6)
        && model != "mythos-preview";

    if let Some(config) = build_output_config(Some("enabled"), request.reasoning_effort)
        && request.reasoning_effort.is_some()
    {
        root["output_config"] = json!(config);
    }
    if !supports_effort {
        root.as_object_mut()
            .expect("request object")
            .remove("output_config");
    } else if let Some(effort) = root.pointer_mut("/output_config/effort")
        && ((effort.as_str() == Some("xhigh") && !supports_xhigh)
            || (effort.as_str() == Some("max") && manual_thinking))
    {
        *effort = json!("high");
    }

    let thinking_enabled = matches!(
        root.pointer("/thinking/type").and_then(Value::as_str),
        Some("enabled" | "adaptive" | "between_tools")
    );
    if root.pointer("/thinking/type").and_then(Value::as_str) == Some("enabled") {
        if manual_thinking {
            ensure!(
                request.max_tokens > 1024,
                "Claude extended thinking requires max_tokens greater than 1024; increase max_tokens or disable thinking"
            );
            let effort = request
                .reasoning_effort
                .map(|effort| match effort {
                    ReasoningEffort::Low | ReasoningEffort::Minimal | ReasoningEffort::None => "low",
                    ReasoningEffort::Medium => "medium",
                    ReasoningEffort::High => "high",
                    ReasoningEffort::XHigh => "xhigh",
                    ReasoningEffort::Max => "max",
                })
                .unwrap_or_else(|| request.request_thinking.as_deref().unwrap_or("high"));
            let budget: usize = match effort.trim().to_ascii_lowercase().as_str() {
                "low" => 1024,
                "medium" => 4096,
                "xhigh" => 16384,
                "max" => 32768,
                _ => 8192,
            };
            root["thinking"]["budget_tokens"] = json!(budget.min(request.max_tokens - 1));
        } else {
            root["thinking"]["type"] = json!("adaptive");
        }
    }

    // The 2023-06-01 API permits sampling on older models, but not alongside
    // thinking (apart from top_p >= 0.95). Newer Claude models reject it outright.
    let supports_sampling = manual_thinking || (major == Some(4) && minor == 6);
    let object = root.as_object_mut().expect("request object");
    if thinking_enabled || !supports_sampling {
        object.remove("temperature");
        object.remove("top_k");
        if !supports_sampling
            || object
                .get("top_p")
                .and_then(Value::as_f64)
                .is_some_and(|value| value < 0.95)
        {
            object.remove("top_p");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
