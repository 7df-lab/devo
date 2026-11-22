use reqwest::{RequestBuilder, StatusCode};
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, Content};
use serde_json::{Value, json};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

pub(super) const AUTH_REJECTED_CODE: &str = "devo_credential_rejected";

pub(super) enum UpstreamError {
    AuthRejected,
    Other(ErrorData),
}

impl From<ErrorData> for UpstreamError {
    fn from(error: ErrorData) -> Self {
        Self::Other(error)
    }
}

pub(super) fn auth_rejected_result(supplier: &str) -> CallToolResult {
    CallToolResult {
        content: vec![Content::text(format!("{supplier} rejected the API key"))],
        structured_content: Some(json!({"_devo":{"code":AUTH_REJECTED_CODE}})),
        is_error: Some(true),
        meta: None,
    }
}

/// Read a bounded response without including upstream bodies or request details in errors.
pub(super) async fn request_json(
    supplier: &str,
    request: RequestBuilder,
) -> Result<Value, UpstreamError> {
    let mut response = request
        .send()
        .await
        .map_err(|_| ErrorData::internal_error(format!("{supplier} request failed"), None))?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(UpstreamError::AuthRejected);
    }
    if !response.status().is_success() {
        let message = match response.status() {
            StatusCode::FORBIDDEN => "denied access",
            StatusCode::PAYMENT_REQUIRED | StatusCode::TOO_MANY_REQUESTS => {
                "credits exhausted or rate limit exceeded"
            }
            _ => "returned an error",
        };
        return Err(ErrorData::internal_error(format!("{supplier} {message}"), None).into());
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        ErrorData::internal_error(format!("{supplier} response could not be read"), None)
    })? {
        if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
            return Err(ErrorData::internal_error(
                format!("{supplier} response exceeded the size limit"),
                None,
            )
            .into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| {
        ErrorData::internal_error(format!("{supplier} returned an invalid response"), None).into()
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn auth_rejection_is_an_error_tool_reply_with_private_code() {
        let result = auth_rejected_result("Exa");
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["isError"], true);
        assert_eq!(
            value["structuredContent"]["_devo"]["code"],
            AUTH_REJECTED_CODE,
        );
        assert_eq!(value["content"][0]["text"], "Exa rejected the API key");
    }
}
