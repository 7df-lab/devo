use reqwest::Client;
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, Content};
use serde_json::{Value, json};

use super::{upstream, valid_fetch_url};

const SEARCH_ENDPOINT: &str = "https://api.exa.ai/search";
const FETCH_ENDPOINT: &str = "https://api.exa.ai/contents";

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
    let value = upstream::request_json(
        "Exa",
        http.post(SEARCH_ENDPOINT)
            .header("x-api-key", api_key)
            .json(&json!({
                "query": query,
                "numResults": 5,
                "contents": { "highlights": true },
            })),
    )
    .await?;
    if !value.get("results").is_some_and(Value::is_array) {
        return Err(ErrorData::internal_error(
            "Exa search returned an invalid result",
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
    let value = upstream::request_json(
        "Exa",
        http.post(FETCH_ENDPOINT)
            .header("x-api-key", api_key)
            .json(&json!({
                "urls": [url],
                "text": { "maxCharacters": 100_000 },
            })),
    )
    .await?;
    fetch_result(&value, api_key)
}

fn fetch_result(value: &Value, api_key: &str) -> Result<CallToolResult, ErrorData> {
    // Contents reports per-URL failures in otherwise successful HTTP responses.
    // A single-URL call succeeds only if it returned actual page text.
    let text = value
        .pointer("/results/0/text")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| {
            ErrorData::internal_error("Exa could not extract content from this URL", None)
        })?;
    Ok(CallToolResult::success(vec![Content::text(
        text.replace(api_key, "[redacted]"),
    )]))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn contents_requires_page_text_and_redacts_credentials() {
        assert_eq!(
            fetch_result(
                &json!({"results": [{"text":"# Title test-secret"}]}),
                "test-secret"
            )
            .unwrap(),
            CallToolResult::success(vec![Content::text("# Title [redacted]")]),
        );
        assert_eq!(
            fetch_result(
                &json!({"results": [], "statuses": [{"status":"error"}]}),
                "test-secret"
            )
            .unwrap_err(),
            ErrorData::internal_error("Exa could not extract content from this URL", None),
        );
    }
}
