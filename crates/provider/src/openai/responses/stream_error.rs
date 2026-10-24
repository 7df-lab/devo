//! Normalize terminal Responses SSE errors into the shared provider taxonomy.

use devo_protocol::ModelRequest;
use serde_json::{Value, json};

use super::super::error_payload::provider_error_from_payload;
use crate::error::ProviderError;

pub(super) fn provider_error_from_event(
    event: &str,
    chunk: &Value,
    request: &ModelRequest,
) -> Option<ProviderError> {
    let (error, fallback) = match event {
        "error" => (
            chunk.get("error").unwrap_or(chunk),
            "OpenAI Responses stream returned an error without details",
        ),
        "response.failed" => (
            chunk.pointer("/response/error").unwrap_or(&Value::Null),
            "OpenAI Responses request failed without error details",
        ),
        // Non-terminal events still belong to the ordinary stream decoder.
        _ => return None,
    };
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or(fallback);
    // Normalize nullable/missing/malformed fields so an explicit failure can
    // never disappear merely because the upstream error has an unfamiliar shape.
    let envelope = json!({
        "error": {
            "message": message,
            "type": error.get("type").and_then(Value::as_str),
            "code": error.get("code"),
        }
    });
    provider_error_from_payload(&envelope, request)
}
