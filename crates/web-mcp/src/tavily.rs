use reqwest::Client;

use super::{upstream, valid_fetch_url};
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, Content};
use serde_json::{Value, json};

const SEARCH_ENDPOINT: &str = "https://api.tavily.com/search";
const FETCH_ENDPOINT: &str = "https://api.tavily.com/extract";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

pub(super) async fn search(
    http: &Client,
    api_key: &str,
    query: String,
) -> Result<CallToolResult, ErrorData> {
    if query.trim().is_empty() || query.len() > 8_000 {
        return Err(ErrorData::invalid_params(
            "query must be non-empty and at most 8000 bytes",
            None,
        ));
    }
    let value = match request(
        http,
        api_key,
        SEARCH_ENDPOINT,
        json!({"query":query, "max_results": 5}),
    )
    .await
    {
        Ok(value) => value,
        Err(upstream::UpstreamError::AuthRejected) => {
            return Ok(upstream::auth_rejected_result("Tavily"));
        }
        Err(upstream::UpstreamError::Other(error)) => return Err(error),
    };
    if !value.get("results").is_some_and(Value::is_array) {
        return Err(ErrorData::internal_error(
            "Tavily search returned an invalid result",
            None,
        ));
    }
    Ok(CallToolResult::success(vec![Content::text(
        value.to_string().replace(api_key, "[redacted]"),
    )]))
}

pub(super) async fn fetch(
    http: &Client,
    api_key: &str,
    url: String,
) -> Result<CallToolResult, ErrorData> {
    if !valid_fetch_url(&url) {
        return Err(ErrorData::invalid_params(
            "url must be HTTP(S), without credentials, and at most 8192 bytes",
            None,
        ));
    }
    let value = match request(
        http,
        api_key,
        FETCH_ENDPOINT,
        json!({"urls": url, "format": "markdown"}),
    )
    .await
    {
        Ok(value) => value,
        Err(upstream::UpstreamError::AuthRejected) => {
            return Ok(upstream::auth_rejected_result("Tavily"));
        }
        Err(upstream::UpstreamError::Other(error)) => return Err(error),
    };
    let content = value
        .pointer("/results/0/raw_content")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ErrorData::internal_error("Tavily could not extract content from this URL", None)
        })?;
    Ok(CallToolResult::success(vec![Content::text(
        content.replace(api_key, "[redacted]"),
    )]))
}

async fn request(
    http: &Client,
    api_key: &str,
    endpoint: &str,
    payload: Value,
) -> Result<Value, upstream::UpstreamError> {
    let mut response = http
        .post(endpoint)
        .bearer_auth(api_key)
        .json(&payload)
        .send()
        .await
        .map_err(|_| ErrorData::internal_error("Tavily request failed", None))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(upstream::UpstreamError::AuthRejected);
    }
    if !response.status().is_success() {
        return Err(ErrorData::internal_error(
            match response.status().as_u16() {
                403 => "Tavily denied access",
                429 => "Tavily rate limit exceeded",
                _ => "Tavily returned an error",
            },
            None,
        )
        .into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ErrorData::internal_error("Tavily response could not be read", None))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(
                ErrorData::internal_error("Tavily response exceeded the size limit", None).into(),
            );
        }
        body.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&body)
        .map_err(|_| ErrorData::internal_error("Tavily returned an invalid response", None))?)
}
