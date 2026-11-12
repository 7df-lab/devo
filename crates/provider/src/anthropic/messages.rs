use std::collections::BTreeMap;
use std::collections::HashMap;
use std::pin::Pin;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ProviderWireApi;
use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseExtra;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::normalize_tool_result_messages;
use futures::Stream;
use reqwest::Client;
use reqwest::header::ACCEPT_ENCODING;
use reqwest::header::CACHE_CONTROL;
use reqwest::header::CONTENT_TYPE;
use reqwest::header::HeaderValue;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use tracing::debug;

use super::AnthropicAIRole;
use crate::ModelProviderSDK;
use crate::ProviderAdapter;
use crate::ProviderCapabilities;
use crate::ProviderHttpOptions;
use crate::dsml::DsmlToolCallHealer;
use crate::hosted_tools::append_anthropic_hosted_tools;
use crate::http::invalid_status_error;
use crate::merge_extra_body;
mod request_options;
mod response_metadata;
mod stream;

use request_options::{apply_claude_options, build_output_config, build_thinking};
use response_metadata::{build_provider_specific_response_payload, map_usage};

/// <https://platform.claude.com/docs/en/api/messages>
/// Anthropic provider backed by the official HTTP API.
pub struct AnthropicProvider {
    client: Client,
    streaming_client: Client,
    base_url: String,
    api_key: Option<String>,
    /// When true, authenticate with OAuth bearer + beta headers instead of x-api-key.
    oauth: bool,
    http_options: ProviderHttpOptions,
}

impl AnthropicProvider {
    pub fn new(base_url: impl Into<String>) -> Self {
        let http_options = ProviderHttpOptions::default();
        Self {
            client: http_options
                .build_request_client()
                .unwrap_or_else(|_| Client::new()),
            streaming_client: http_options
                .build_streaming_client()
                .unwrap_or_else(|_| Client::new()),
            base_url: base_url.into(),
            api_key: None,
            oauth: false,
            http_options,
        }
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self.oauth = false;
        self
    }

    /// Authenticate with a Claude Pro/Max OAuth access token (L2-DES-AUTH-001).
    pub fn with_oauth_access(mut self, access_token: impl Into<String>) -> Self {
        self.api_key = Some(access_token.into());
        self.oauth = true;
        self
    }

    pub fn with_http_options(mut self, http_options: ProviderHttpOptions) -> Result<Self> {
        self.client = http_options.build_request_client()?;
        self.streaming_client = http_options.build_streaming_client()?;
        self.http_options = http_options;
        Ok(self)
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    fn post_builder(
        &self,
        client: &Client,
        body: &Value,
        headers: &BTreeMap<String, String>,
    ) -> reqwest::RequestBuilder {
        let builder = client
            .post(self.endpoint())
            .header("anthropic-version", "2023-06-01")
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let builder = self
            .http_options
            .apply_request_headers(self.http_options.apply_custom_headers(builder), headers);

        let builder = if let Some(api_key) = &self.api_key {
            if self.oauth {
                builder
                    .header("Authorization", format!("Bearer {api_key}"))
                    .header("anthropic-beta", "claude-code-20250219,oauth-2025-04-20")
                    .header("x-app", "cli")
            } else {
                builder
                    .header("x-api-key", api_key)
                    .header("Authorization", format!("Bearer {api_key}"))
            }
        } else {
            builder
        };
        builder.json(body)
    }

    fn request_builder(
        &self,
        body: &Value,
        headers: &BTreeMap<String, String>,
    ) -> reqwest::RequestBuilder {
        self.post_builder(&self.client, body, headers)
    }

    fn streaming_request_builder(
        &self,
        body: &Value,
        headers: &BTreeMap<String, String>,
    ) -> reqwest::RequestBuilder {
        self.post_builder(&self.streaming_client, body, headers)
            .header(CACHE_CONTROL, HeaderValue::from_static("no-cache"))
            .header(ACCEPT_ENCODING, HeaderValue::from_static("identity"))
    }
}

#[derive(Debug, Serialize)]
struct AnthropicMessagesRequest {
    model: String,
    max_tokens: usize,
    stream: bool,
    messages: Vec<AnthropicInputMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<AnthropicOutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
}

#[derive(Debug, Serialize)]
struct AnthropicInputMessage {
    role: AnthropicAIRole,
    content: Vec<AnthropicInputContentBlock>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum AnthropicInputContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "server_tool_use")]
    ServerToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "web_search_tool_result")]
    WebSearchToolResult { tool_use_id: String, content: Value },
    #[serde(rename = "web_fetch_tool_result")]
    WebFetchToolResult { tool_use_id: String, content: Value },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    #[serde(rename = "image")]
    Image { source: AnthropicImageSource },
}

#[derive(Debug, Serialize)]
struct AnthropicImageSource {
    r#type: &'static str,
    media_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
struct AnthropicToolDefinition {
    name: String,
    description: String,
    input_schema: Value,
}

#[derive(Debug, Serialize)]
struct AnthropicThinkingConfig {
    r#type: &'static str,
}

#[derive(Debug, Serialize)]
struct AnthropicOutputConfig {
    effort: &'static str,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicMessageResponse {
    id: String,
    #[serde(default)]
    container: Option<AnthropicContainer>,
    #[serde(default)]
    content: Vec<AnthropicResponseContentBlock>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    stop_details: Option<AnthropicStopDetails>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    stop_sequence: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicContainer {
    id: String,
    expires_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicStopDetails {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    explanation: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_tokens_details: Option<AnthropicOutputTokensDetails>,
    #[serde(default)]
    cache_creation_input_tokens: Option<usize>,
    #[serde(default)]
    cache_read_input_tokens: Option<usize>,
    #[serde(default)]
    cache_creation: Option<AnthropicCacheCreation>,
    #[serde(default)]
    inference_geo: Option<String>,
    #[serde(default)]
    server_tool_use: Option<AnthropicServerToolUsage>,
    #[serde(default)]
    service_tier: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicOutputTokensDetails {
    #[serde(default)]
    thinking_tokens: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicCacheCreation {
    #[serde(default)]
    ephemeral_1h_input_tokens: Option<usize>,
    #[serde(default)]
    ephemeral_5m_input_tokens: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicServerToolUsage {
    #[serde(default)]
    web_fetch_requests: Option<usize>,
    #[serde(default)]
    web_search_requests: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicResponseContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    citations: Option<Value>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    caller: Option<Value>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default)]
    content: Option<Value>,
    #[serde(default)]
    status: Option<String>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[async_trait]
impl ModelProviderSDK for AnthropicProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        let body = build_request(&request, /*stream*/ false)?;
        debug!(
            provider = "anthropic",
            api_base = %self.base_url,
            model = %request.model,
            messages = request.messages.len(),
            tools = request.tools.as_ref().map_or(0, Vec::len),
            max_tokens = request.max_tokens,
            "sending anthropic completion request"
        );

        let response = self
            .request_builder(&body, &crate::request_headers(request.extra_body.as_ref()))
            .send()
            .await
            .context("failed to send anthropic request")?;
        let response = match response.error_for_status_ref() {
            Ok(_) => response,
            Err(_) => {
                let status = response.status();
                return Err(invalid_status_error(
                    "anthropic",
                    &request.model,
                    "request",
                    status,
                    response,
                    &body,
                )
                .await);
            }
        };

        let value: Value = response
            .json()
            .await
            .context("failed to decode anthropic response")?;
        parse_response(value, &DsmlToolCallHealer::for_request(&request))
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        stream::completion_stream(self, request).await
    }

    fn name(&self) -> &str {
        "anthropic"
    }
}

#[async_trait]
impl ProviderAdapter for AnthropicProvider {
    fn family(&self) -> ProviderWireApi {
        ProviderWireApi::AnthropicMessages
    }

    fn capabilities(&self, _model: &str) -> ProviderCapabilities {
        ProviderCapabilities::anthropic()
    }
}

/// Anthropic messages request body. See <https://platform.claude.com/docs/en/api/messages>.
fn build_request(request: &ModelRequest, stream: bool) -> Result<Value> {
    let mut messages = request.messages.clone();
    normalize_tool_result_messages(&mut messages);
    let body = AnthropicMessagesRequest {
        model: request.model.clone(),
        max_tokens: request.max_tokens,
        stream,
        messages: messages
            .iter()
            .filter_map(build_message)
            .collect::<Vec<_>>(),
        system: request.system.clone(),
        tools: request.tools.as_ref().map(|tools| {
            tools
                .iter()
                .map(|tool| AnthropicToolDefinition {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                })
                .collect::<Vec<_>>()
        }),
        thinking: request.request_thinking.as_deref().and_then(build_thinking),
        output_config: build_output_config(
            request.request_thinking.as_deref(),
            request.reasoning_effort,
        ),
        temperature: request.sampling.temperature,
        top_p: request.sampling.top_p,
        top_k: request.sampling.top_k,
    };
    let mut root =
        serde_json::to_value(body).expect("anthropic request body serialization should succeed");

    append_anthropic_hosted_tools(&mut root, &request.hosted_tools);

    apply_claude_options(&mut root, request)?;

    merge_extra_body(&mut root, request.extra_body.as_ref());

    Ok(root)
}

/// Anthropic messages response body. See <https://platform.claude.com/docs/en/api/messages>.
fn parse_response(value: Value, dsml_healer: &DsmlToolCallHealer) -> Result<ModelResponse> {
    let response: AnthropicMessageResponse = serde_json::from_value(value.clone())
        .context("failed to deserialize anthropic messages response")?;
    let mut content = Vec::new();
    let mut metadata = ResponseMetadata::default();

    let mut hosted_tool_inputs: HashMap<String, Value> = HashMap::new();
    for block in &response.content {
        if let Some(mut parsed) = parse_response_content_block(block, &mut metadata) {
            if let ResponseContent::HostedToolUse {
                id,
                input,
                output,
                status,
                ..
            } = &mut parsed
            {
                if output.is_none() && status.is_none() {
                    hosted_tool_inputs.insert(id.clone(), input.clone());
                } else if matches!(input, Value::Object(map) if map.is_empty())
                    && let Some(previous_input) = hosted_tool_inputs.get(id)
                {
                    *input = previous_input.clone();
                }
            }
            content.push(parsed);
        }
    }
    let content = dsml_healer.heal_response_content(content);
    let stop_reason = response.stop_reason.as_deref().map(parse_stop_reason);
    let usage = response.usage.as_ref().map(map_usage).unwrap_or_default();

    if let Some(provider_payload) = build_provider_specific_response_payload(&response) {
        metadata.extras.push(ResponseExtra::ProviderSpecific {
            provider: "anthropic".to_string(),
            payload: provider_payload,
        });
    }

    Ok(ModelResponse {
        id: response.id,
        content,
        stop_reason,
        usage,
        metadata,
    })
}

fn build_message(message: &RequestMessage) -> Option<AnthropicInputMessage> {
    let role = message
        .role
        .parse::<AnthropicAIRole>()
        .unwrap_or(AnthropicAIRole::User);
    let content = message
        .content
        .iter()
        .filter_map(build_content_block)
        .collect::<Vec<_>>();

    (!content.is_empty()).then_some(AnthropicInputMessage { role, content })
}

fn build_content_block(block: &RequestContent) -> Option<AnthropicInputContentBlock> {
    match block {
        RequestContent::Text { text } => {
            Some(AnthropicInputContentBlock::Text { text: text.clone() })
        }
        RequestContent::Reasoning { .. } => None,
        RequestContent::ProviderReasoning { provider, payload } if provider == "anthropic" => {
            if payload.get("type").and_then(Value::as_str) == Some("redacted_thinking") {
                return Some(AnthropicInputContentBlock::RedactedThinking {
                    data: payload.get("data")?.as_str()?.to_string(),
                });
            }
            Some(AnthropicInputContentBlock::Thinking {
                thinking: payload
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                signature: payload
                    .get("signature")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        }
        RequestContent::ProviderReasoning { .. } => None,
        RequestContent::ToolUse { id, name, input } => Some(AnthropicInputContentBlock::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        }),
        RequestContent::HostedToolUse {
            id,
            name,
            input,
            output,
            ..
        } => Some(build_hosted_content_block(id, name, input, output)),
        RequestContent::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => Some(AnthropicInputContentBlock::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: content.clone(),
            is_error: *is_error,
        }),
        RequestContent::Image {
            mime_type,
            data_base64,
        } => Some(AnthropicInputContentBlock::Image {
            source: AnthropicImageSource {
                r#type: "base64",
                media_type: mime_type.clone(),
                data: data_base64.clone(),
            },
        }),
    }
}

fn build_hosted_content_block(
    id: &str,
    name: &str,
    input: &Value,
    output: &Option<Value>,
) -> AnthropicInputContentBlock {
    match output {
        None => AnthropicInputContentBlock::ServerToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: input.clone(),
        },
        Some(content) if name == "web_fetch" => AnthropicInputContentBlock::WebFetchToolResult {
            tool_use_id: id.to_string(),
            content: content.clone(),
        },
        Some(content) => AnthropicInputContentBlock::WebSearchToolResult {
            tool_use_id: id.to_string(),
            content: content.clone(),
        },
    }
}

fn hosted_result_tool_name(kind: &str) -> String {
    match kind {
        "web_fetch_tool_result" => "web_fetch".to_string(),
        "web_search_tool_result" => "web_search".to_string(),
        _ => "web_search".to_string(),
    }
}

fn parse_response_content_block(
    block: &AnthropicResponseContentBlock,
    metadata: &mut ResponseMetadata,
) -> Option<ResponseContent> {
    match block.kind.as_str() {
        "text" => Some(ResponseContent::Text(
            block.text.clone().unwrap_or_default(),
        )),
        "tool_use" => Some(ResponseContent::ToolUse {
            id: block.id.clone()?,
            name: block.name.clone()?,
            input: block
                .input
                .clone()
                .unwrap_or_else(|| Value::Object(serde_json::Map::new())),
        }),
        "server_tool_use" => Some(ResponseContent::HostedToolUse {
            id: block.id.clone()?,
            name: block
                .name
                .clone()
                .unwrap_or_else(|| "web_search".to_string()),
            input: block
                .input
                .clone()
                .unwrap_or_else(|| Value::Object(serde_json::Map::new())),
            output: None,
            status: None,
        }),
        "web_search_tool_result" | "web_fetch_tool_result" => {
            Some(ResponseContent::HostedToolUse {
                id: block.tool_use_id.clone().or_else(|| block.id.clone())?,
                name: hosted_result_tool_name(&block.kind),
                input: Value::Object(serde_json::Map::new()),
                output: block.content.clone(),
                status: Some(
                    block
                        .status
                        .clone()
                        .unwrap_or_else(|| "completed".to_string()),
                ),
            })
        }
        "thinking" => {
            if let Some(thinking) = &block.thinking
                && !thinking.is_empty()
            {
                metadata.extras.push(ResponseExtra::ReasoningText {
                    text: thinking.clone(),
                });
            }
            Some(ResponseContent::ProviderReasoning {
                provider: "anthropic".to_string(),
                payload: Value::Object(anthropic_thinking_payload_map(block)),
            })
        }
        "redacted_thinking" => Some(ResponseContent::ProviderReasoning {
            provider: "anthropic".to_string(),
            payload: json!({"type": "redacted_thinking", "data": block.data.as_ref()?}),
        }),
        _ => None,
    }
}

fn anthropic_thinking_payload_map(block: &AnthropicResponseContentBlock) -> Map<String, Value> {
    let mut payload = block.extra.clone();
    payload.insert("type".to_string(), json!("thinking"));
    payload.insert(
        "thinking".to_string(),
        json!(block.thinking.clone().unwrap_or_default()),
    );
    if let Some(signature) = &block.signature {
        payload.insert("signature".to_string(), json!(signature));
    }
    payload
}

fn append_json_string_field(payload: &mut Map<String, Value>, key: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    match payload.get_mut(key) {
        Some(Value::String(existing)) => existing.push_str(value),
        _ => {
            payload.insert(key.to_string(), json!(value));
        }
    }
}

fn insert_provider_reasoning_blocks(
    content_blocks: &mut BTreeMap<usize, ResponseContent>,
    provider_reasoning_blocks: BTreeMap<usize, Map<String, Value>>,
) {
    for (index, payload) in provider_reasoning_blocks {
        content_blocks.insert(
            index,
            ResponseContent::ProviderReasoning {
                provider: "anthropic".to_string(),
                payload: Value::Object(payload),
            },
        );
    }
}

fn parse_stop_reason(value: &str) -> StopReason {
    match value {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "pause_turn" => StopReason::PauseTurn,
        _ => StopReason::EndTurn,
    }
}

#[cfg(test)]
mod tests;
