use serde_json::Value;

use crate::error::{ProviderError, context_limit_error};

/// Errors can arrive after HTTP 200 in the Messages event stream.
pub(super) fn from_payload(payload: &Value) -> ProviderError {
    let error = payload.get("error").unwrap_or(payload);
    let kind = error.get("type").and_then(Value::as_str);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| error.to_string());
    if let Some(error) = context_limit_error(message.clone(), kind, /*error_code*/ None) {
        return error;
    }
    let provider_name = Some("anthropic".to_string());
    match kind {
        Some("overloaded_error") => ProviderError::ProviderServerError {
            message,
            status_code: Some(529),
            provider_name,
        },
        Some("api_error") => ProviderError::ProviderServerError {
            message,
            status_code: Some(500),
            provider_name,
        },
        Some("rate_limit_error") => ProviderError::RateLimitError {
            message,
            retry_after_seconds: None,
            provider_name,
        },
        Some("authentication_error" | "permission_error") => ProviderError::AuthenticationError {
            message,
            status_code: None,
            provider_name,
        },
        Some("invalid_request_error" | "request_too_large") => ProviderError::InvalidRequestError {
            message,
            details: Some(error.to_string()),
        },
        Some("not_found_error") => ProviderError::ModelNotFoundError {
            message,
            model_name: None,
        },
        _ => ProviderError::UnknownError {
            message,
            status_code: None,
        },
    }
}
