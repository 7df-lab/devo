use super::*;

pub(in crate::runtime::handlers) fn legacy_error_to_acp(
    request_id: serde_json::Value,
    legacy_response: serde_json::Value,
) -> serde_json::Value {
    if let Ok(error) = serde_json::from_value::<ErrorResponse>(legacy_response) {
        acp_error_response(request_id, AcpErrorCode::ServerError, error.error.message)
    } else {
        acp_error_response(
            request_id,
            AcpErrorCode::InternalError,
            "failed to decode internal runtime response",
        )
    }
}
