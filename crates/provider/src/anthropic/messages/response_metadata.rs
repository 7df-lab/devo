use devo_protocol::Usage;
use serde_json::{Value, json};

use super::{AnthropicMessageResponse, AnthropicUsage};

pub(super) fn map_usage(usage: &AnthropicUsage) -> Usage {
    let cache_creation_input_tokens = usage.cache_creation_input_tokens.unwrap_or(0);
    let cache_read_input_tokens = usage.cache_read_input_tokens.unwrap_or(0);
    Usage {
        unreported_fields: None,
        reported: usage.input_tokens.is_some() || usage.output_tokens.is_some(),
        input_tokens: usage
            .input_tokens
            .unwrap_or(0)
            .saturating_add(cache_creation_input_tokens)
            .saturating_add(cache_read_input_tokens),
        output_tokens: usage.output_tokens.unwrap_or(0),
        cache_creation_input_tokens: usage.cache_creation_input_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens,
        reasoning_output_tokens: usage
            .output_tokens_details
            .as_ref()
            .and_then(|details| details.thinking_tokens),
        total_tokens: None,
    }
    .with_field_coverage(usage.input_tokens, usage.output_tokens)
}

pub(super) fn build_provider_specific_response_payload(
    response: &AnthropicMessageResponse,
) -> Option<Value> {
    let mut payload = serde_json::Map::new();

    if let Some(container) = &response.container {
        payload.insert("container".to_string(), json!(container));
    }
    if !response.content.is_empty() {
        payload.insert("content".to_string(), json!(response.content));
    }
    if let Some(model) = &response.model {
        payload.insert("model".to_string(), json!(model));
    }
    if let Some(role) = &response.role {
        payload.insert("role".to_string(), json!(role));
    }
    if let Some(stop_details) = &response.stop_details {
        payload.insert("stop_details".to_string(), json!(stop_details));
    }
    if let Some(stop_sequence) = &response.stop_sequence {
        payload.insert("stop_sequence".to_string(), json!(stop_sequence));
    }
    if let Some(stop_reason) = &response.stop_reason {
        payload.insert("stop_reason".to_string(), json!(stop_reason));
    }
    if let Some(kind) = &response.kind {
        payload.insert("type".to_string(), json!(kind));
    }
    if let Some(usage) = &response.usage {
        payload.insert("usage".to_string(), json!(usage));
    }

    if payload.is_empty() {
        None
    } else {
        Some(Value::Object(payload))
    }
}

