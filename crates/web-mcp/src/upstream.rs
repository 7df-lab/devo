use reqwest::{RequestBuilder, StatusCode};
use rmcp::ErrorData;
use serde_json::Value;

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// Read a bounded response without including upstream bodies or request details in errors.
pub(super) async fn request_json(
    supplier: &str,
    request: RequestBuilder,
) -> Result<Value, ErrorData> {
    let mut response = request
        .send()
        .await
        .map_err(|_| ErrorData::internal_error(format!("{supplier} request failed"), None))?;
    if !response.status().is_success() {
        let message = match response.status() {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                "rejected the API key or denied access"
            }
            StatusCode::PAYMENT_REQUIRED | StatusCode::TOO_MANY_REQUESTS => {
                "credits exhausted or rate limit exceeded"
            }
            _ => "returned an error",
        };
        return Err(ErrorData::internal_error(
            format!("{supplier} {message}"),
            None,
        ));
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        ErrorData::internal_error(format!("{supplier} response could not be read"), None)
    })? {
        if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
            return Err(ErrorData::internal_error(
                format!("{supplier} response exceeded the size limit"),
                None,
            ));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| {
        ErrorData::internal_error(format!("{supplier} returned an invalid response"), None)
    })
}
