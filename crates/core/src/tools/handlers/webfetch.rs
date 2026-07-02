use async_trait::async_trait;
use base64::Engine;
use tokio::time::Duration;
use tokio::time::timeout;

use crate::contracts::ToolCallError;
use crate::contracts::ToolContext;
use crate::contracts::ToolProgressSender;
use crate::contracts::ToolResult;
use crate::contracts::ToolResultContent;
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolCapabilityTag;
use crate::tool_spec::ToolExecutionMode;
use crate::tool_spec::ToolOutputMode;
use crate::tool_spec::ToolSpec;

const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;
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
                supports_cancellation: Some(true),
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
        let timeout_secs = parse_timeout_secs(&input);
        let timeout_ms = timeout_secs.saturating_mul(1_000);
        let timeout_duration = Duration::from_millis(timeout_ms);

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

        let proxy_config = devo_network_proxy::NetworkProxyConfig {
            proxy_url: ctx.network_proxy.clone(),
            no_proxy: ctx.network_no_proxy.clone(),
        };
        let client = devo_network_proxy::apply_proxy_config(
            reqwest::Client::builder()
                .connect_timeout(devo_provider::timeout::connect_timeout())
                .timeout(timeout_duration),
            &proxy_config,
        )
        .map_err(|e| {
            ToolCallError::ExecutionFailed(format!("Failed to configure HTTP proxy: {e}"))
        })?
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36")
            .build()
            .map_err(|e| ToolCallError::ExecutionFailed(format!("Failed to create HTTP client: {e}")))?;

        let cancel_token = ctx.cancel_token.clone();
        let url = url.to_string();
        let format = format.to_string();
        let operation = async move {
            let response = client
                .get(&url)
                .header(reqwest::header::ACCEPT, accept)
                .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
                .send()
                .await
                .map_err(|error| map_webfetch_request_error(error, timeout_secs))?;
            if !response.status().is_success() {
                let msg = format!("Request failed with status code: {}", response.status());
                return Err(ToolCallError::ExecutionFailed(msg));
            }
            if response
                .content_length()
                .is_some_and(|len| len as usize > MAX_RESPONSE_SIZE)
            {
                return Err(ToolCallError::ExecutionFailed("response too large".into()));
            }
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            let bytes = response
                .bytes()
                .await
                .map_err(|error| map_webfetch_request_error(error, timeout_secs))?;
            if bytes.len() > MAX_RESPONSE_SIZE {
                return Err(ToolCallError::ExecutionFailed("response too large".into()));
            }

            let url = url.clone();
            let format = format.clone();
            let bytes = bytes.to_vec();
            tokio::task::spawn_blocking(move || {
                build_webfetch_result(&url, bytes, &content_type, &format)
            })
            .await
            .map_err(|error| {
                ToolCallError::ExecutionFailed(format!("response processing failed: {error}"))
            })?
        };

        let fetch_result = timeout(timeout_duration, async {
            tokio::select! {
                result = operation => result,
                () = cancel_token.cancelled() => Err(ToolCallError::Cancelled),
            }
        })
        .await;

        match fetch_result {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => {
                if matches!(error, ToolCallError::Cancelled) {
                    return Ok(ToolResult::error(
                        ToolResultContent::Text("Request cancelled".into()),
                        "Cancelled",
                        error,
                    ));
                }
                if matches!(error, ToolCallError::TimedOut(_)) {
                    return Ok(ToolResult::error(
                        ToolResultContent::Text("Request timed out".into()),
                        "Timeout",
                        error,
                    ));
                }
                let message = error.to_string();
                Ok(ToolResult::error(
                    ToolResultContent::Text(message.clone()),
                    "HTTP error",
                    error,
                ))
            }
            Err(_) => Ok(ToolResult::error(
                ToolResultContent::Text("Request timed out".into()),
                "Timeout",
                ToolCallError::TimedOut(timeout_secs),
            )),
        }
    }
}

fn build_webfetch_result(
    url: &str,
    bytes: Vec<u8>,
    content_type: &str,
    format: &str,
) -> Result<ToolResult, ToolCallError> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let title = format!("{url} ({content_type})");

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

fn parse_timeout_secs(input: &serde_json::Value) -> u64 {
    let raw = &input["timeout"];
    let secs = raw
        .as_u64()
        .or_else(|| raw.as_i64().and_then(|value| u64::try_from(value).ok()))
        .or_else(|| {
            raw.as_f64().and_then(|value| {
                if value.is_finite() && value > 0.0 {
                    Some(value.round() as u64)
                } else {
                    None
                }
            })
        })
        .or_else(|| raw.as_str().and_then(|value| value.parse().ok()))
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    secs.clamp(1, MAX_TIMEOUT_SECS)
}

fn map_webfetch_request_error(error: reqwest::Error, timeout_secs: u64) -> ToolCallError {
    if error.is_timeout() {
        ToolCallError::TimedOut(timeout_secs)
    } else {
        ToolCallError::ExecutionFailed(format!("Request failed: {error}"))
    }
}

fn is_image_mime(mime: &str) -> bool {
    mime.starts_with("image/") && mime != "image/svg+xml" && mime != "image/vnd.fastbidsheet"
}

fn extract_text_from_html(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut skip = false;
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if starts_with_ignore_ascii_case(&bytes[i..], b"<script")
                || starts_with_ignore_ascii_case(&bytes[i..], b"<style")
                || starts_with_ignore_ascii_case(&bytes[i..], b"<noscript")
                || starts_with_ignore_ascii_case(&bytes[i..], b"<iframe")
                || starts_with_ignore_ascii_case(&bytes[i..], b"<object")
                || starts_with_ignore_ascii_case(&bytes[i..], b"<embed")
            {
                skip = true;
            }
            in_tag = true;
        } else if bytes[i] == b'>' {
            in_tag = false;
            if skip
                && (bytes[i.saturating_sub(10)..=i]
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
    if let Some(start) = text.find(|ch: char| !ch.is_whitespace()) {
        if start > 0 {
            text.drain(..start);
        }
        text.truncate(text.trim_end().len());
        text
    } else {
        String::new()
    }
}

fn starts_with_ignore_ascii_case(text: &[u8], prefix: &[u8]) -> bool {
    text.get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::extract_text_from_html;
    use super::parse_timeout_secs;

    #[test]
    fn parse_timeout_secs_accepts_integers_floats_and_strings() {
        assert_eq!(parse_timeout_secs(&serde_json::json!({ "timeout": 5 })), 5);
        assert_eq!(
            parse_timeout_secs(&serde_json::json!({ "timeout": 5.0 })),
            5
        );
        assert_eq!(
            parse_timeout_secs(&serde_json::json!({ "timeout": "10" })),
            10
        );
        assert_eq!(parse_timeout_secs(&serde_json::json!({})), 30);
        assert_eq!(
            parse_timeout_secs(&serde_json::json!({ "timeout": 999 })),
            120
        );
    }

    #[test]
    fn extract_text_from_html_skips_case_insensitive_script_blocks() {
        let html = "  <HTML><BODY>Hello<SCRIPT>hidden</SCRIPT> world</BODY></HTML>\n";

        assert_eq!(extract_text_from_html(html), "Hello world");
    }

    #[test]
    fn extract_text_from_html_trims_without_content() {
        assert_eq!(extract_text_from_html(" \n\t "), "");
    }
}
