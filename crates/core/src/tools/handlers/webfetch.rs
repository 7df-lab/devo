use async_trait::async_trait;
use base64::Engine;
use tokio::time::{Duration, timeout};

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolCapabilityTag, ToolExecutionMode, ToolOutputMode, ToolSpec};

const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024;
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 120_000;
const WEBFETCH_DESCRIPTION: &str = include_str!("../webfetch.txt");

pub struct WebFetchHandler {
    spec: ToolSpec,
}

impl Default for WebFetchHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFetchHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "webfetch".into(),
                description: WEBFETCH_DESCRIPTION.into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "url".to_string(),
                            JsonSchema::string(Some("The URL to fetch content from")),
                        ),
                        (
                            "format".to_string(),
                            JsonSchema::string(Some(
                                "The format to return the content in (text, markdown, html)",
                            )),
                        ),
                        (
                            "timeout".to_string(),
                            JsonSchema::integer(Some("Optional timeout in seconds")),
                        ),
                    ]),
                    Some(vec!["url".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Mixed,
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
impl ToolHandler for WebFetchHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let url = input["url"].as_str().unwrap_or("");
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Ok(ToolResult::error(
                ToolResultContent::Text("URL must start with http:// or https://".into()),
                "Invalid URL",
                ToolCallError::InvalidInput("URL must start with http:// or https://".into()),
            ));
        }

        let format = input["format"].as_str().unwrap_or("markdown");
        let timeout_ms = input["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_MS / 1000)
            .saturating_mul(1000)
            .min(MAX_TIMEOUT_MS);

        let accept = match format {
            "markdown" => {
                "text/markdown;q=1.0, text/x-markdown;q=0.9, text/plain;q=0.8, text/html;q=0.7, */*;q=0.1"
            }
            "text" => "text/plain;q=1.0, text/markdown;q=0.9, text/html;q=0.8, */*;q=0.1",
            "html" => {
                "text/html;q=1.0, application/xhtml+xml;q=0.9, text/plain;q=0.8, text/markdown;q=0.7, */*;q=0.1"
            }
            _ => "*/*",
        };

        let client = devo_network_proxy::apply_proxy(
            reqwest::Client::builder(),
            ctx.network_proxy.as_deref(),
        )
        .map_err(|e| {
            ToolCallError::ExecutionFailed(format!("Failed to configure HTTP proxy: {e}"))
        })?
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36")
            .build()
            .map_err(|e| ToolCallError::ExecutionFailed(format!("Failed to create HTTP client: {e}")))?;

        let request = client
            .get(url)
            .header(reqwest::header::ACCEPT, accept)
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9");

        let response = timeout(Duration::from_millis(timeout_ms), request.send()).await;
        let response = match response {
            Ok(result) => result
                .map_err(|e| ToolCallError::ExecutionFailed(format!("Request failed: {e}")))?,
            Err(_) => {
                return Ok(ToolResult::error(
                    ToolResultContent::Text("Request timed out".into()),
                    "Timeout",
                    ToolCallError::TimedOut(timeout_ms / 1000),
                ));
            }
        };

        if !response.status().is_success() {
            let msg = format!("Request failed with status code: {}", response.status());
            return Ok(ToolResult::error(
                ToolResultContent::Text(msg.clone()),
                "HTTP error",
                ToolCallError::ExecutionFailed(msg),
            ));
        }

        if response
            .content_length()
            .is_some_and(|len| len as usize > MAX_RESPONSE_SIZE)
        {
            return Ok(ToolResult::error(
                ToolResultContent::Text("Response too large (exceeds 5MB limit)".into()),
                "Response too large",
                ToolCallError::ExecutionFailed("response too large".into()),
            ));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let mime = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let title = format!("{url} ({content_type})");

        let bytes = response
            .bytes()
            .await
            .map_err(|e| ToolCallError::ExecutionFailed(format!("Failed to read response: {e}")))?;

        if bytes.len() > MAX_RESPONSE_SIZE {
            return Ok(ToolResult::error(
                ToolResultContent::Text("Response too large (exceeds 5MB limit)".into()),
                "Response too large",
                ToolCallError::ExecutionFailed("response too large".into()),
            ));
        }

        if is_image_mime(&mime) {
            return Ok(ToolResult::success(
                ToolResultContent::Mixed {
                    text: Some("Image fetched successfully".to_string()),
                    json: Some(serde_json::json!({
                        "title": title,
                        "mime": mime,
                        "image_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
                    })),
                },
                "Image fetched",
            ));
        }

        let content = String::from_utf8_lossy(&bytes).into_owned();
        let output = match format {
            "text" => {
                if content_type.contains("text/html") {
                    extract_text_from_html(&content)
                } else {
                    content
                }
            }
            "html" => content,
            "markdown" => {
                if content_type.contains("text/html") {
                    convert_html_to_markdown(&content)
                } else {
                    content
                }
            }
            _ => content,
        };

        Ok(ToolResult::success(
            ToolResultContent::Mixed {
                text: Some(output),
                json: Some(serde_json::json!({ "title": title, "mime": mime })),
            },
            "Content fetched",
        ))
    }
}

fn is_image_mime(mime: &str) -> bool {
    mime.starts_with("image/") && mime != "image/svg+xml" && mime != "image/vnd.fastbidsheet"
}

fn extract_text_from_html(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut skip = false;
    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let lower_bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if lower_bytes[i..].starts_with(b"<script")
                || lower_bytes[i..].starts_with(b"<style")
                || lower_bytes[i..].starts_with(b"<noscript")
                || lower_bytes[i..].starts_with(b"<iframe")
                || lower_bytes[i..].starts_with(b"<object")
                || lower_bytes[i..].starts_with(b"<embed")
            {
                skip = true;
            }
            in_tag = true;
        } else if bytes[i] == b'>' {
            in_tag = false;
            if skip
                && (lower_bytes[i.saturating_sub(10)..=i]
                    .windows(2)
                    .any(|w| w == b"</"))
            {
                skip = false;
            }
        } else if !in_tag && !skip {
            text.push(bytes[i] as char);
        }
        i += 1;
    }
    text.trim().to_string()
}

fn convert_html_to_markdown(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}
