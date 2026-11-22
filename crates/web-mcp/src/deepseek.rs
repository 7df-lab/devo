use reqwest::Client;
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, Content};
use serde_json::{Value, json};

const SEARCH_ENDPOINT: &str = "https://api.deepseek.com/v1/tools/search";
const FETCH_ENDPOINT: &str = "https://api.deepseek.com/v1/tools/fetch";
const MAX_SEARCH_QUERY_CHARS: usize = 128;
use super::{upstream, valid_fetch_url};
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

pub(super) async fn search(
    http: &Client,
    api_key: &str,
    query: String,
) -> Result<CallToolResult, ErrorData> {
    if query.trim().is_empty() || query.chars().count() > MAX_SEARCH_QUERY_CHARS {
        return Err(ErrorData::invalid_params(
            "query must contain 1–128 non-whitespace characters",
            None,
        ));
    }

    match request(http, api_key, SEARCH_ENDPOINT, json!({ "query": query })).await {
        Ok(result) => Ok(result),
        Err(upstream::UpstreamError::AuthRejected) => {
            Ok(upstream::auth_rejected_result("DeepSeek"))
        }
        Err(upstream::UpstreamError::Other(error)) => Err(error),
    }
}

pub(super) async fn fetch(
    http: &Client,
    api_key: &str,
    url: String,
) -> Result<CallToolResult, ErrorData> {
    if !valid_fetch_url(&url) {
        return Err(ErrorData::invalid_params(
            "url must be an HTTP(S) URL with a host and no username or password, at most 8192 bytes",
            None,
        ));
    }

    match request(http, api_key, FETCH_ENDPOINT, json!({ "url": url })).await {
        Ok(result) => Ok(result),
        Err(upstream::UpstreamError::AuthRejected) => {
            Ok(upstream::auth_rejected_result("DeepSeek"))
        }
        Err(upstream::UpstreamError::Other(error)) => Err(error),
    }
}

async fn request(
    http: &Client,
    api_key: &str,
    endpoint: &str,
    payload: Value,
) -> Result<CallToolResult, upstream::UpstreamError> {
    let mut response = http
        .post(endpoint)
        .bearer_auth(api_key)
        .json(&payload)
        .send()
        .await
        .map_err(|_| ErrorData::internal_error("DeepSeek request failed", None))?;

    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(upstream::UpstreamError::AuthRejected);
    }
    if !response.status().is_success() {
        let message = match response.status().as_u16() {
            402 => "DeepSeek account has insufficient balance",
            403 => "DeepSeek denied the request",
            429 => "DeepSeek rate limit exceeded",
            _ => "DeepSeek service returned an error",
        };
        return Err(ErrorData::internal_error(message, None).into());
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ErrorData::internal_error("DeepSeek response could not be read", None))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(ErrorData::internal_error(
                "DeepSeek response exceeded the size limit",
                None,
            )
            .into());
        }
        body.extend_from_slice(&chunk);
    }

    let value: Value = serde_json::from_slice(&body)
        .map_err(|_| ErrorData::internal_error("DeepSeek returned an invalid response", None))?;
    let text = serde_json::to_string(&value)
        .map_err(|_| ErrorData::internal_error("DeepSeek response could not be encoded", None))?
        .replace(api_key, "[redacted]");
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn accepts_http_and_https_urls_with_hosts() {
        assert!(valid_fetch_url("https://example.com/page?q=hello"));
        assert!(valid_fetch_url("http://localhost:8080/"));
    }

    #[test]
    fn rejects_invalid_or_credentialed_fetch_urls() {
        for url in [
            "",
            "ftp://example.com/file",
            "https:///missing-host",
            "https://user@example.com/",
            "https://user:secret@example.com/",
            "not a URL",
        ] {
            assert!(!valid_fetch_url(url), "unexpectedly accepted {url:?}");
        }
    }

    #[test]
    fn enforces_fetch_url_byte_limit() {
        let url = format!("https://example.com/{}", "a".repeat(8192));
        assert!(!valid_fetch_url(&url));
    }

    #[test]
    fn search_length_uses_unicode_characters() {
        assert_eq!("界".repeat(MAX_SEARCH_QUERY_CHARS).chars().count(), 128);
        assert!("界".repeat(MAX_SEARCH_QUERY_CHARS + 1).chars().count() > 128);
    }
}
