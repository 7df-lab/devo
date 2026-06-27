use async_trait::async_trait;
use devo_config::LocalWebSearchProviderKind;
use devo_config::ResolvedLocalWebSearchConfig;
use serde::Deserialize;
use serde_json::Value;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolCapabilityTag, ToolExecutionMode, ToolOutputMode, ToolSpec};
use crate::tools::websearch_prompt::web_search_prompt;

const LOCAL_CONFIG_KEY: &str = "__devo_local_web_search";
const DEFAULT_EXA_BASE_URL: &str = "https://api.exa.ai/search";
const DEFAULT_TAVILY_BASE_URL: &str = "https://api.tavily.com/search";
const DEFAULT_MAX_RESULTS: u32 = 5;

pub struct WebSearchHandler {
    spec: ToolSpec,
}

impl Default for WebSearchHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearchHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "web_search".into(),
                description: web_search_prompt(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([(
                        "query".to_string(),
                        JsonSchema::string(Some("The search query")),
                    )]),
                    Some(vec!["query".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![ToolCapabilityTag::NetworkAccess],
                supports_parallel: true,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
        }
    }
}

#[async_trait]
impl ToolHandler for WebSearchHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let query = input["query"].as_str().unwrap_or("").trim();
        if query.is_empty() {
            return Ok(tool_error("Search query is required"));
        }
        let config = input
            .get(LOCAL_CONFIG_KEY)
            .cloned()
            .ok_or_else(|| {
                ToolCallError::NeedsConfiguration(
                    "local web_search provider is not configured for this turn".to_string(),
                )
            })
            .and_then(|value| {
                serde_json::from_value::<ResolvedLocalWebSearchConfig>(value).map_err(|error| {
                    ToolCallError::NeedsConfiguration(format!(
                        "invalid local web_search provider configuration: {error}"
                    ))
                })
            })?;
        let max_results = config
            .max_results
            .or_else(|| {
                input["max_results"]
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
            })
            .unwrap_or(DEFAULT_MAX_RESULTS);

        let response = match config.kind {
            LocalWebSearchProviderKind::Exa => {
                search_exa(
                    &config,
                    query,
                    max_results,
                    ctx.network_proxy.clone(),
                    ctx.network_no_proxy.clone(),
                )
                .await?
            }
            LocalWebSearchProviderKind::Tavily => {
                search_tavily(
                    &config,
                    query,
                    max_results,
                    ctx.network_proxy.clone(),
                    ctx.network_no_proxy.clone(),
                )
                .await?
            }
        };

        Ok(ToolResult::success(
            ToolResultContent::Text(response),
            "Search completed",
        ))
    }
}

async fn search_exa(
    config: &ResolvedLocalWebSearchConfig,
    query: &str,
    max_results: u32,
    network_proxy: Option<String>,
    network_no_proxy: Option<String>,
) -> Result<String, ToolCallError> {
    let url = config.base_url.as_deref().unwrap_or(DEFAULT_EXA_BASE_URL);
    let proxy_config = devo_network_proxy::NetworkProxyConfig {
        proxy_url: network_proxy,
        no_proxy: network_no_proxy,
    };
    let client = devo_network_proxy::build_client_config(&proxy_config).map_err(|error| {
        ToolCallError::ExecutionFailed(format!("Failed to create HTTP client: {error}"))
    })?;
    let response = client
        .post(url)
        .header("x-api-key", &config.api_key)
        .json(&serde_json::json!({
            "query": query,
            "type": "auto",
            "numResults": max_results,
            "contents": { "highlights": true }
        }))
        .send()
        .await
        .map_err(|error| ToolCallError::ExecutionFailed(format!("Exa search failed: {error}")))?;
    response_text("Exa", response)
        .await
        .map(|value| format_exa_results(query, &value))
}

async fn search_tavily(
    config: &ResolvedLocalWebSearchConfig,
    query: &str,
    max_results: u32,
    network_proxy: Option<String>,
    network_no_proxy: Option<String>,
) -> Result<String, ToolCallError> {
    let url = config
        .base_url
        .as_deref()
        .unwrap_or(DEFAULT_TAVILY_BASE_URL);
    let proxy_config = devo_network_proxy::NetworkProxyConfig {
        proxy_url: network_proxy,
        no_proxy: network_no_proxy,
    };
    let client = devo_network_proxy::build_client_config(&proxy_config).map_err(|error| {
        ToolCallError::ExecutionFailed(format!("Failed to create HTTP client: {error}"))
    })?;
    let response = client
        .post(url)
        .bearer_auth(&config.api_key)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&serde_json::json!({
            "query": query,
            "auto_parameters": false,
            "topic": "general",
            "search_depth": "basic",
            "chunks_per_source": 3,
            "max_results": max_results,
            "time_range": null,
            "include_answer": false,
            "include_raw_content": false,
            "include_images": false,
            "include_image_descriptions": false,
            "include_favicon": false,
            "include_domains": [],
            "exclude_domains": [],
            "country": null,
            "include_usage": false
        }))
        .send()
        .await
        .map_err(|error| {
            ToolCallError::ExecutionFailed(format!("Tavily search failed: {error}"))
        })?;
    response_text("Tavily", response)
        .await
        .map(|value| format_tavily_results(query, &value))
}

async fn response_text(
    provider: &str,
    response: reqwest::Response,
) -> Result<Value, ToolCallError> {
    let status = response.status();
    let text = response.text().await.map_err(|error| {
        ToolCallError::ExecutionFailed(format!("{provider} search response read failed: {error}"))
    })?;
    if !status.is_success() {
        return Err(ToolCallError::ExecutionFailed(format!(
            "{provider} search error ({status}): {text}"
        )));
    }
    serde_json::from_str(&text).map_err(|error| {
        ToolCallError::ExecutionFailed(format!(
            "{provider} search returned invalid JSON: {error}; body: {text}"
        ))
    })
}

#[derive(Deserialize)]
struct ExaResult {
    title: Option<String>,
    url: Option<String>,
    text: Option<String>,
    summary: Option<String>,
    highlights: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct ExaResponse {
    results: Option<Vec<ExaResult>>,
}

fn format_exa_results(query: &str, value: &Value) -> String {
    let parsed = serde_json::from_value::<ExaResponse>(value.clone())
        .unwrap_or(ExaResponse { results: None });
    format_results(
        query,
        parsed
            .results
            .unwrap_or_default()
            .into_iter()
            .map(|result| {
                let snippet = result.summary.or(result.text).or_else(|| {
                    result
                        .highlights
                        .and_then(|values| values.into_iter().next())
                });
                SearchResultLine {
                    title: result.title,
                    url: result.url,
                    snippet,
                }
            }),
    )
}

#[derive(Deserialize)]
struct TavilyResult {
    title: Option<String>,
    url: Option<String>,
    content: Option<String>,
}

#[derive(Deserialize)]
struct TavilyResponse {
    answer: Option<String>,
    results: Option<Vec<TavilyResult>>,
}

fn format_tavily_results(query: &str, value: &Value) -> String {
    let parsed =
        serde_json::from_value::<TavilyResponse>(value.clone()).unwrap_or(TavilyResponse {
            answer: None,
            results: None,
        });
    let mut text = format_results(
        query,
        parsed
            .results
            .unwrap_or_default()
            .into_iter()
            .map(|result| SearchResultLine {
                title: result.title,
                url: result.url,
                snippet: result.content,
            }),
    );
    if let Some(answer) = parsed.answer.filter(|answer| !answer.trim().is_empty()) {
        text.push_str("\nAnswer:\n");
        text.push_str(answer.trim());
    }
    text
}

struct SearchResultLine {
    title: Option<String>,
    url: Option<String>,
    snippet: Option<String>,
}

fn format_results(query: &str, results: impl IntoIterator<Item = SearchResultLine>) -> String {
    let mut output = format!("Search results for: {query}\n");
    for (index, result) in results.into_iter().enumerate() {
        let title = result
            .title
            .unwrap_or_else(|| "Untitled result".to_string());
        if let Some(url) = result.url.filter(|url| !url.trim().is_empty()) {
            output.push_str(&format!("\n{}. [{title}]({})\n", index + 1, url.trim()));
        } else {
            output.push_str(&format!("\n{}. {title}\n", index + 1));
        }
        if let Some(snippet) = result.snippet.filter(|snippet| !snippet.trim().is_empty()) {
            output.push_str("Snippet: ");
            output.push_str(snippet.trim());
            output.push('\n');
        }
    }
    output
}

fn tool_error(message: &str) -> ToolResult {
    ToolResult::error(
        ToolResultContent::Text(message.to_string()),
        "Search error",
        ToolCallError::InvalidInput(message.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use std::io;

    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    use super::*;

    #[test]
    fn exa_results_format_title_url_and_snippet() {
        let text = format_exa_results(
            "rust async",
            &json!({
                "results": [
                    {
                        "title": "Async Rust",
                        "url": "https://example.com/rust",
                        "summary": "A concise summary."
                    }
                ]
            }),
        );

        assert_eq!(
            text,
            "Search results for: rust async\n\n1. [Async Rust](https://example.com/rust)\nSnippet: A concise summary.\n"
        );
    }

    #[test]
    fn tavily_results_append_answer() {
        let text = format_tavily_results(
            "search api",
            &json!({
                "answer": "Short answer.",
                "results": [
                    {
                        "title": "Search API",
                        "url": "https://example.com/search",
                        "content": "Result content."
                    }
                ]
            }),
        );

        assert_eq!(
            text,
            "Search results for: search api\n\n1. [Search API](https://example.com/search)\nSnippet: Result content.\n\nAnswer:\nShort answer."
        );
    }

    #[test]
    fn web_search_handler_description_uses_sources_prompt() {
        let handler = WebSearchHandler::new();

        assert!(handler.spec().description.contains("Sources:"));
        assert!(handler.spec().description.contains("The current month is "));
    }

    #[tokio::test]
    async fn tavily_search_request_matches_api_shape() {
        let (base_url, capture) = spawn_tavily_server(json!({
            "results": [
                {
                    "title": "Lionel Messi",
                    "url": "https://example.com/messi",
                    "content": "Lionel Messi is an Argentine footballer."
                }
            ]
        }))
        .await;
        let config = ResolvedLocalWebSearchConfig {
            provider_id: "tavily".to_string(),
            kind: LocalWebSearchProviderKind::Tavily,
            api_key: "tavily-key".to_string(),
            base_url: Some(base_url),
            max_results: Some(1),
        };

        let text = search_tavily(
            &config,
            "who is Leo Messi?",
            1,
            /*network_proxy*/ None,
            /*network_no_proxy*/ None,
        )
        .await
        .expect("Tavily search should succeed");
        let request = capture.await.expect("capture request");
        let body: Value = serde_json::from_str(&request.body).expect("request body JSON");

        assert_eq!(
            text,
            "Search results for: who is Leo Messi?\n\n1. [Lionel Messi](https://example.com/messi)\nSnippet: Lionel Messi is an Argentine footballer.\n"
        );
        assert_eq!(
            request.headers.lines().next(),
            Some("POST /search HTTP/1.1")
        );
        assert_eq!(
            header_value(&request.headers, "authorization"),
            Some("Bearer tavily-key")
        );
        assert_eq!(
            header_value(&request.headers, "content-type"),
            Some("application/json")
        );
        assert_eq!(
            body,
            json!({
                "query": "who is Leo Messi?",
                "auto_parameters": false,
                "topic": "general",
                "search_depth": "basic",
                "chunks_per_source": 3,
                "max_results": 1,
                "time_range": null,
                "include_answer": false,
                "include_raw_content": false,
                "include_images": false,
                "include_image_descriptions": false,
                "include_favicon": false,
                "include_domains": [],
                "exclude_domains": [],
                "country": null,
                "include_usage": false
            })
        );
    }

    #[tokio::test]
    async fn exa_live_search_works_when_api_key_is_configured() {
        let Some(api_key) = live_api_key("EXA_API_KEY", "Exa") else {
            return;
        };

        let config = ResolvedLocalWebSearchConfig {
            provider_id: "exa".to_string(),
            kind: LocalWebSearchProviderKind::Exa,
            api_key,
            base_url: None,
            max_results: Some(2),
        };

        let text = search_exa(
            &config,
            "Rust programming language official website",
            2,
            /*network_proxy*/ None,
            /*network_no_proxy*/ None,
        )
        .await
        .expect("Exa live search should succeed");

        assert!(text.contains("Search results for: Rust programming language official website"));
        assert!(text.contains("]("));
    }

    #[tokio::test]
    async fn tavily_live_search_works_when_api_key_is_configured() {
        let Some(api_key) = live_api_key("TAVILY_API_KEY", "Tavily") else {
            return;
        };

        let config = ResolvedLocalWebSearchConfig {
            provider_id: "tavily".to_string(),
            kind: LocalWebSearchProviderKind::Tavily,
            api_key,
            base_url: None,
            max_results: Some(1),
        };

        let text = search_tavily(
            &config,
            "who is Leo Messi?",
            1,
            /*network_proxy*/ None,
            /*network_no_proxy*/ None,
        )
        .await
        .expect("Tavily live search should succeed");

        assert!(text.contains("Search results for: who is Leo Messi?"));
        assert!(text.contains("]("));
    }

    struct CapturedHttpRequest {
        headers: String,
        body: String,
    }

    async fn spawn_tavily_server(
        response_body: Value,
    ) -> (String, tokio::task::JoinHandle<CapturedHttpRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept request");
            let request = read_http_request(&mut socket).await.expect("read request");
            let response_body = response_body.to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
            request
        });

        (format!("http://{addr}/search"), handle)
    }

    async fn read_http_request(
        socket: &mut tokio::net::TcpStream,
    ) -> io::Result<CapturedHttpRequest> {
        let mut bytes = Vec::new();
        let mut buffer = [0; 1024];
        let header_end = loop {
            let count = socket.read(&mut buffer).await?;
            if count == 0 {
                break bytes.len();
            }
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]).to_string();
        let content_length = content_length(&headers).unwrap_or_default();
        while bytes.len() < header_end + content_length {
            let count = socket.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        let body =
            String::from_utf8_lossy(&bytes[header_end..header_end + content_length]).to_string();

        Ok(CapturedHttpRequest { headers, body })
    }

    fn content_length(headers: &str) -> Option<usize> {
        header_value(headers, "content-length")?.parse().ok()
    }

    fn header_value<'a>(request: &'a str, name: &str) -> Option<&'a str> {
        request.lines().skip(1).find_map(|line| {
            let (header_name, value) = line.split_once(':')?;
            header_name.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    fn live_api_key(env_name: &str, provider: &str) -> Option<String> {
        let Ok(api_key) = std::env::var(env_name) else {
            eprintln!("skipping {provider} live search test because {env_name} is not set");
            return None;
        };
        let api_key = api_key.trim().to_string();
        if api_key.is_empty() {
            eprintln!("skipping {provider} live search test because {env_name} is empty");
            return None;
        }
        Some(api_key)
    }
}
