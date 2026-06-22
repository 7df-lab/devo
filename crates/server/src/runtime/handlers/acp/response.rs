use super::*;

pub(super) fn acp_success_response<T: serde::Serialize>(
    request_id: serde_json::Value,
    result: T,
) -> serde_json::Value {
    serde_json::to_value(AcpSuccessResponse::new(request_id, result))
        .expect("serialize ACP success response")
}

pub(super) fn acp_error_response(
    request_id: serde_json::Value,
    code: AcpErrorCode,
    message: impl Into<String>,
) -> serde_json::Value {
    serde_json::to_value(AcpErrorResponse::new(
        request_id,
        code,
        message,
        serde_json::Value::Null,
    ))
    .expect("serialize ACP error response")
}

pub(super) fn legacy_error_to_acp(
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
