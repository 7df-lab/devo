use reqwest::Client;
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, Content};
use serde_json::{Value, json};

use super::{upstream, valid_fetch_url};

const SEARCH_ENDPOINT: &str = "https://google.serper.dev/search";
const FETCH_ENDPOINT: &str = "https://scrape.serper.dev/";

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
    let value = match upstream::request_json(
        "Serper",
        http.post(SEARCH_ENDPOINT)
            .header("X-API-KEY", api_key)
            .json(&json!({ "q": query })),
    )
    .await
    {
        Ok(value) => value,
        Err(upstream::UpstreamError::AuthRejected) => {
            return Ok(upstream::auth_rejected_result("Serper"));
        }
        Err(upstream::UpstreamError::Other(error)) => return Err(error),
    };
    search_result(value, api_key)
}

fn search_result(value: Value, api_key: &str) -> Result<CallToolResult, ErrorData> {
    // Serper can omit `organic` for a query with no ordinary results.
    if !value.get("organic").is_some_and(Value::is_array)
        && !value.get("searchParameters").is_some_and(Value::is_object)
    {
        return Err(ErrorData::internal_error(
            "Serper search returned an invalid result",
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
    let value = match upstream::request_json(
        "Serper",
        http.post(FETCH_ENDPOINT)
            .header("X-API-KEY", api_key)
            .json(&json!({ "url": url, "includeMarkdown": true })),
    )
    .await
    {
        Ok(value) => value,
        Err(upstream::UpstreamError::AuthRejected) => {
            return Ok(upstream::auth_rejected_result("Serper"));
        }
        Err(upstream::UpstreamError::Other(error)) => return Err(error),
    };
    fetch_result(&value, api_key)
}

fn fetch_result(value: &Value, api_key: &str) -> Result<CallToolResult, ErrorData> {
    let markdown = value
        .get("markdown")
        .and_then(Value::as_str)
        .filter(|markdown| !markdown.trim().is_empty())
        .ok_or_else(|| {
            ErrorData::internal_error("Serper returned no Markdown for this URL", None)
        })?;
    Ok(CallToolResult::success(vec![Content::text(
        markdown.replace(api_key, "[redacted]"),
    )]))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn search_keeps_citations_and_accepts_no_organic_results() {
        let source = json!({"organic": [{"title": "Test", "link": "https://example.com/"}]});
        assert_eq!(
            search_result(source.clone(), "test-secret").unwrap(),
            CallToolResult::success(vec![Content::text(source.to_string())]),
        );
        let no_results = json!({"searchParameters": {"q": "nothing"}});
        assert_eq!(
            search_result(no_results.clone(), "test-secret").unwrap(),
            CallToolResult::success(vec![Content::text(no_results.to_string())]),
        );
        assert_eq!(
            search_result(json!({"message": "test-secret"}), "test-secret").unwrap_err(),
            ErrorData::internal_error("Serper search returned an invalid result", None),
        );
    }

    #[test]
    fn fetch_uses_markdown_and_redacts_credentials() {
        assert_eq!(
            fetch_result(
                &json!({"text": "plain text", "markdown": "# Title test-secret"}),
                "test-secret"
            )
            .unwrap(),
            CallToolResult::success(vec![Content::text("# Title [redacted]")]),
        );
        assert_eq!(
            fetch_result(
                &json!({"text": "plain text", "markdown": " "}),
                "test-secret"
            )
            .unwrap_err(),
            ErrorData::internal_error("Serper returned no Markdown for this URL", None),
        );
    }
}
