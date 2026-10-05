use std::collections::BTreeMap;
use std::collections::HashMap;
use std::pin::Pin;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningEffort;
use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseExtra;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
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
mod stream;

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
    WebSearchToolResult {
        tool_use_id: String,
        content: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    #[serde(rename = "web_fetch_tool_result")]
    WebFetchToolResult {
        tool_use_id: String,
        content: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
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
    input_tokens: usize,
    output_tokens: usize,
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
        let body = build_request(&request, false);
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
fn build_request(request: &ModelRequest, stream: bool) -> Value {
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

    merge_extra_body(&mut root, request.extra_body.as_ref());

    root
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
            status,
        } => Some(build_hosted_content_block(id, name, input, output, status)),
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
    status: &Option<String>,
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
            status: status.clone(),
        },
        Some(content) => AnthropicInputContentBlock::WebSearchToolResult {
            tool_use_id: id.to_string(),
            content: content.clone(),
            status: status.clone(),
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

fn map_usage(usage: &AnthropicUsage) -> Usage {
    let cache_creation_input_tokens = usage.cache_creation_input_tokens.unwrap_or(0);
    let cache_read_input_tokens = usage.cache_read_input_tokens.unwrap_or(0);
    Usage {
        input_tokens: usage
            .input_tokens
            .saturating_add(cache_creation_input_tokens)
            .saturating_add(cache_read_input_tokens),
        output_tokens: usage.output_tokens,
        cache_creation_input_tokens: usage.cache_creation_input_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens,
        reasoning_output_tokens: None,
        total_tokens: None,
    }
}

fn build_provider_specific_response_payload(response: &AnthropicMessageResponse) -> Option<Value> {
    let mut payload = serde_json::Map::new();

    if let Some(container) = &response.container {
        payload.insert("container".to_string(), json!(container));
    }
    if !response.content.is_empty() {
        payload.insert("content".to_string(), json!(response.content));
    }
    if let Some(model) = &response.model {
        payload.insert("model".to_string(), json!(model));
    }
    if let Some(role) = &response.role {
        payload.insert("role".to_string(), json!(role));
    }
    if let Some(stop_details) = &response.stop_details {
        payload.insert("stop_details".to_string(), json!(stop_details));
    }
    if let Some(stop_sequence) = &response.stop_sequence {
        payload.insert("stop_sequence".to_string(), json!(stop_sequence));
    }
    if let Some(kind) = &response.kind {
        payload.insert("type".to_string(), json!(kind));
    }
    if let Some(usage) = &response.usage {
        payload.insert("usage".to_string(), json!(usage));
    }

    if payload.is_empty() {
        None
    } else {
        Some(Value::Object(payload))
    }
}

fn parse_stop_reason(value: &str) -> StopReason {
    match value {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        _ => StopReason::EndTurn,
    }
}

fn build_thinking(level: &str) -> Option<AnthropicThinkingConfig> {
    match level.trim().to_ascii_lowercase().as_str() {
        "" | "default" => None,
        "disabled" => Some(AnthropicThinkingConfig { r#type: "disabled" }),
        "enabled" | "low" | "medium" | "high" | "xhigh" | "max" => {
            Some(AnthropicThinkingConfig { r#type: "enabled" })
        }
        _ => None,
    }
}

fn build_output_config(
    thinking: Option<&str>,
    reasoning_effort: Option<ReasoningEffort>,
) -> Option<AnthropicOutputConfig> {
    if matches!(
        thinking
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "" | "default" | "disabled"
    ) {
        return None;
    }

    let effort = match reasoning_effort {
        Some(ReasoningEffort::Low) => "low",
        Some(ReasoningEffort::Medium) => "medium",
        Some(ReasoningEffort::High) => "high",
        Some(ReasoningEffort::XHigh) => "xhigh",
        Some(ReasoningEffort::Max) => "max",
        Some(ReasoningEffort::None | ReasoningEffort::Minimal) => return None,
        None => match thinking?.trim().to_ascii_lowercase().as_str() {
            "enabled" => "high",
            "low" => "low",
            "medium" => "medium",
            "high" => "high",
            "xhigh" => "xhigh",
            "max" => "max",
            _ => return None,
        },
    };

    Some(AnthropicOutputConfig { effort })
}

#[cfg(test)]
mod tests {
    use crate::dsml::DsmlToolCallHealer;
    use devo_protocol::ModelRequest;
    use devo_protocol::ReasoningEffort;
    use devo_protocol::RequestContent;
    use devo_protocol::RequestMessage;
    use devo_protocol::SamplingControls;
    use devo_protocol::ToolDefinition;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::build_request;
    use super::parse_response;
    use super::parse_stop_reason;
    use devo_protocol::ResponseContent;
    use devo_protocol::ResponseExtra;
    use devo_protocol::StopReason;

    #[test]
    fn build_request_includes_sampling_tools_and_thinking() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "claude-sonnet-4-6".to_string(),
            system: Some("You are helpful.".to_string()),
            messages: vec![
                RequestMessage {
                    role: "assistant".to_string(),
                    content: vec![
                        RequestContent::Text {
                            text: "Calling tool".to_string(),
                        },
                        RequestContent::ToolUse {
                            id: "toolu_123".to_string(),
                            name: "get_weather".to_string(),
                            input: json!({"city": "Boston"}),
                        },
                    ],
                },
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![RequestContent::ToolResult {
                        tool_use_id: "toolu_123".to_string(),
                        content: "{\"temp\":72}".to_string(),
                        is_error: Some(false),
                    }],
                },
            ],
            max_tokens: 1024,
            tools: Some(vec![ToolDefinition {
                name: "get_weather".to_string(),
                description: "Get weather by city".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "city": { "type": "string" } },
                    "required": ["city"]
                }),
                output_schema: None,
            }]),
            hosted_tools: Vec::new(),
            sampling: SamplingControls {
                temperature: Some(0.2),
                top_p: Some(0.9),
                top_k: Some(32),
            },
            request_thinking: Some("medium".to_string()),
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(body["model"], json!("claude-sonnet-4-6"));
        assert_eq!(body["max_tokens"], json!(1024));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["system"], json!("You are helpful."));
        assert_eq!(body["temperature"], json!(0.2));
        assert_eq!(body["top_p"], json!(0.9));
        assert_eq!(body["top_k"], json!(32));
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["output_config"]["effort"], json!("medium"));
        assert_eq!(body["messages"][0]["role"], json!("assistant"));
        assert_eq!(body["messages"][0]["content"][1]["type"], json!("tool_use"));
        assert_eq!(
            body["messages"][1]["content"][0]["type"],
            json!("tool_result")
        );
        assert_eq!(body["tools"][0]["name"], json!("get_weather"));
    }

    #[test]
    fn build_request_skips_unsigned_reasoning_blocks() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "deepseek-v4-flash".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "assistant".to_string(),
                content: vec![
                    RequestContent::Reasoning {
                        text: "Need to inspect the file first.".to_string(),
                    },
                    RequestContent::Text {
                        text: "I'll read the README.".to_string(),
                    },
                    RequestContent::ToolUse {
                        id: "toolu_123".to_string(),
                        name: "read".to_string(),
                        input: json!({"path": "README.md"}),
                    },
                ],
            }],
            max_tokens: 1024,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: Some("enabled".to_string()),
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(
            body["messages"][0]["content"],
            json!([
                {
                    "type": "text",
                    "text": "I'll read the README."
                },
                {
                    "type": "tool_use",
                    "id": "toolu_123",
                    "name": "read",
                    "input": { "path": "README.md" }
                }
            ])
        );
    }

    #[test]
    fn build_request_omits_messages_with_no_anthropic_content() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "claude-sonnet-4-6".to_string(),
            system: None,
            messages: vec![
                RequestMessage {
                    role: "assistant".to_string(),
                    content: vec![RequestContent::Reasoning {
                        text: "unsigned reasoning".to_string(),
                    }],
                },
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![RequestContent::Text {
                        text: "continue".to_string(),
                    }],
                },
            ],
            max_tokens: 1024,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, false);

        assert_eq!(body["messages"].as_array().map(Vec::len), Some(1));
        assert_eq!(body["messages"][0]["role"], json!("user"));
        assert_eq!(body["messages"][0]["content"][0]["text"], json!("continue"));
    }

    #[test]
    fn build_request_serializes_provider_reasoning_with_signature() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "deepseek-v4-pro".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "assistant".to_string(),
                content: vec![RequestContent::ProviderReasoning {
                    provider: "anthropic".to_string(),
                    payload: json!({
                        "type": "thinking",
                        "thinking": "Need to inspect the file first.",
                        "signature": "sig_123"
                    }),
                }],
            }],
            max_tokens: 1024,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: Some("enabled".to_string()),
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(
            body["messages"][0]["content"],
            json!([{
                "type": "thinking",
                "thinking": "Need to inspect the file first.",
                "signature": "sig_123"
            }])
        );
    }

    #[test]
    fn build_request_serializes_hosted_tool_use_blocks() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "deepseek-v4-pro".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "assistant".to_string(),
                content: vec![
                    RequestContent::HostedToolUse {
                        id: "srvtool_1".to_string(),
                        name: "web_search".to_string(),
                        input: json!({"query": "Rust docs"}),
                        output: None,
                        status: None,
                    },
                    RequestContent::HostedToolUse {
                        id: "srvtool_1".to_string(),
                        name: "web_search".to_string(),
                        input: json!({"query": "Rust docs"}),
                        output: Some(json!([{
                            "title": "Rust documentation",
                            "url": "https://example.test/rust"
                        }])),
                        status: Some("completed".to_string()),
                    },
                ],
            }],
            max_tokens: 1024,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(
            body["messages"][0]["content"],
            json!([
                {
                    "type": "server_tool_use",
                    "id": "srvtool_1",
                    "name": "web_search",
                    "input": { "query": "Rust docs" }
                },
                {
                    "type": "web_search_tool_result",
                    "tool_use_id": "srvtool_1",
                    "content": [{
                        "title": "Rust documentation",
                        "url": "https://example.test/rust"
                    }],
                    "status": "completed"
                }
            ])
        );
    }

    #[test]
    fn build_request_sends_disabled_thinking_without_output_config() {
        let mut request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "deepseek-v4-flash".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "Reply with OK only.".to_string(),
                }],
            }],
            max_tokens: 1024,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: Some("disabled".to_string()),
            reasoning_effort: None,
            extra_body: None,
        };

        let disabled = build_request(&request, true);
        request.request_thinking = Some("bogus".to_string());
        let unknown = build_request(&request, true);

        assert_eq!(disabled["thinking"]["type"], json!("disabled"));
        assert_eq!(disabled.get("output_config"), None);
        assert_eq!(unknown.get("thinking"), None);
        assert_eq!(unknown.get("output_config"), None);
    }

    #[test]
    fn build_request_maps_max_thinking_to_output_config_effort() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "deepseek-v4-pro".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "Reply with OK only.".to_string(),
                }],
            }],
            max_tokens: 1024,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: Some("max".to_string()),
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["output_config"]["effort"], json!("max"));
    }

    #[test]
    fn build_request_prefers_reasoning_effort_for_output_config_effort() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "deepseek-v4-pro".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "Reply with OK only.".to_string(),
                }],
            }],
            max_tokens: 1024,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: Some("enabled".to_string()),
            reasoning_effort: Some(ReasoningEffort::Max),
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["output_config"]["effort"], json!("max"));
    }

    #[test]
    fn parse_response_extracts_text_tool_use_reasoning_and_usage() {
        let response = parse_response(
            json!({
                "id": "msg_123",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-6",
                "content": [
                    {
                        "type": "thinking",
                        "thinking": "Need to call the weather tool first.",
                        "signature": "sig_123"
                    },
                    {
                        "type": "text",
                        "text": "Let me check that."
                    },
                    {
                        "type": "server_tool_use",
                        "id": "srvtool_1",
                        "name": "web_search",
                        "input": { "query": "Boston weather" },
                        "caller": { "type": "direct" }
                    }
                ],
                "stop_reason": "tool_use",
                "usage": {
                    "input_tokens": 11,
                    "output_tokens": 7,
                    "cache_creation_input_tokens": 3,
                    "cache_read_input_tokens": 5,
                    "service_tier": "standard",
                    "inference_geo": "us"
                }
            }),
            &DsmlToolCallHealer::for_model("claude-sonnet-4-6"),
        )
        .expect("parse response");

        assert_eq!(response.id, "msg_123");
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(response.usage.input_tokens, 19);
        assert_eq!(response.usage.output_tokens, 7);
        assert_eq!(response.usage.cache_creation_input_tokens, Some(3));
        assert_eq!(response.usage.cache_read_input_tokens, Some(5));
        assert_eq!(response.content.len(), 3);
        match &response.content[0] {
            ResponseContent::ProviderReasoning { provider, payload } => {
                assert_eq!(provider, "anthropic");
                assert_eq!(
                    payload,
                    &json!({
                        "type": "thinking",
                        "thinking": "Need to call the weather tool first.",
                        "signature": "sig_123"
                    })
                );
            }
            other => panic!("expected provider reasoning block, got {other:?}"),
        }
        match &response.content[1] {
            ResponseContent::Text(text) => {
                assert_eq!(text, "Let me check that.");
            }
            other => panic!("expected text block, got {other:?}"),
        }
        match &response.content[2] {
            ResponseContent::HostedToolUse {
                id,
                name,
                input,
                output,
                status,
            } => {
                assert_eq!(id, "srvtool_1");
                assert_eq!(name, "web_search");
                assert_eq!(input, &json!({"query": "Boston weather"}));
                assert_eq!(output, &None);
                assert_eq!(status, &None);
            }
            other => panic!("expected hosted tool use block, got {other:?}"),
        }
        assert!(response.metadata.extras.iter().any(|extra| matches!(
            extra,
            ResponseExtra::ReasoningText { text }
            if text == "Need to call the weather tool first."
        )));
        assert!(response.metadata.extras.iter().any(|extra| matches!(
            extra,
            ResponseExtra::ProviderSpecific { provider, .. } if provider == "anthropic"
        )));
    }

    #[test]
    fn parse_response_heals_deepseek_v4_dsml_text_tool_calls() {
        let response = parse_response(
            json!({
                "id": "msg_dsml",
                "type": "message",
                "role": "assistant",
                "model": "deepseek-v4-pro",
                "content": [
                    {
                        "type": "text",
                        "text": "<｜｜DSML｜｜tool_calls>\n<｜｜DSML｜｜invoke name=\"web_search\">\n<｜｜DSML｜｜parameter name=\"query\" string=\"true\">electron-vite npm package 2026</｜｜DSML｜｜parameter>\n<｜｜DSML｜｜parameter name=\"limit\" string=\"false\">3</｜｜DSML｜｜parameter>\n</｜｜DSML｜｜invoke>\n</｜｜DSML｜｜tool_calls>"
                    }
                ],
                "stop_reason": "tool_use",
                "usage": {
                    "input_tokens": 4,
                    "output_tokens": 2
                }
            }),
            &DsmlToolCallHealer::for_model("deepseek-v4-pro"),
        )
        .expect("parse response");

        assert_eq!(
            response.content,
            vec![ResponseContent::ToolUse {
                id: "dsml_0_0".to_string(),
                name: "web_search".to_string(),
                input: json!({
                    "query": "electron-vite npm package 2026",
                    "limit": 3
                }),
            }]
        );
    }

    #[test]
    fn parse_response_extracts_web_search_tool_result_as_hosted_completion() {
        let response = parse_response(
            json!({
                "id": "msg_456",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-6",
                "content": [
                    {
                        "type": "web_search_tool_result",
                        "tool_use_id": "srvtool_1",
                        "content": [
                            {
                                "type": "web_search_result",
                                "title": "Boston weather",
                                "url": "https://example.test/weather"
                            }
                        ],
                        "status": "completed"
                    }
                ],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 4,
                    "output_tokens": 2
                }
            }),
            &DsmlToolCallHealer::for_model("claude-sonnet-4-6"),
        )
        .expect("parse response");

        assert_eq!(response.content.len(), 1);
        match &response.content[0] {
            ResponseContent::HostedToolUse {
                id,
                name,
                input,
                output,
                status,
            } => {
                assert_eq!(id, "srvtool_1");
                assert_eq!(name, "web_search");
                assert_eq!(input, &json!({}));
                assert_eq!(
                    output,
                    &Some(json!([
                        {
                            "type": "web_search_result",
                            "title": "Boston weather",
                            "url": "https://example.test/weather"
                        }
                    ]))
                );
                assert_eq!(status.as_deref(), Some("completed"));
            }
            other => panic!("expected hosted tool completion, got {other:?}"),
        }
    }

    #[test]
    fn parse_response_carries_hosted_web_search_input_into_completion() {
        let response = parse_response(
            json!({
                "id": "msg_789",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-6",
                "content": [
                    {
                        "type": "server_tool_use",
                        "id": "srvtool_1",
                        "name": "web_search",
                        "input": { "query": "DeepSeek official website" }
                    },
                    {
                        "type": "web_search_tool_result",
                        "tool_use_id": "srvtool_1",
                        "content": [
                            {
                                "type": "web_search_result",
                                "title": "DeepSeek",
                                "url": "https://www.deepseek.com/"
                            }
                        ],
                        "status": "completed"
                    }
                ],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 4,
                    "output_tokens": 2
                }
            }),
            &DsmlToolCallHealer::for_model("claude-sonnet-4-6"),
        )
        .expect("parse response");

        assert_eq!(
            response.content,
            vec![
                ResponseContent::HostedToolUse {
                    id: "srvtool_1".to_string(),
                    name: "web_search".to_string(),
                    input: json!({"query": "DeepSeek official website"}),
                    output: None,
                    status: None,
                },
                ResponseContent::HostedToolUse {
                    id: "srvtool_1".to_string(),
                    name: "web_search".to_string(),
                    input: json!({"query": "DeepSeek official website"}),
                    output: Some(json!([
                        {
                            "type": "web_search_result",
                            "title": "DeepSeek",
                            "url": "https://www.deepseek.com/"
                        }
                    ])),
                    status: Some("completed".to_string()),
                },
            ]
        );
    }

    #[test]
    fn parse_response_extracts_web_fetch_tool_result_as_hosted_completion() {
        let response = parse_response(
            json!({
                "id": "msg_fetch",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-6",
                "content": [
                    {
                        "type": "server_tool_use",
                        "id": "srvtool_fetch",
                        "name": "web_fetch",
                        "input": { "url": "https://example.test/docs" }
                    },
                    {
                        "type": "web_fetch_tool_result",
                        "tool_use_id": "srvtool_fetch",
                        "content": {
                            "type": "web_fetch_result",
                            "url": "https://example.test/docs",
                            "title": "Docs"
                        },
                        "status": "completed"
                    }
                ],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 4,
                    "output_tokens": 2
                }
            }),
            &DsmlToolCallHealer::for_model("claude-sonnet-4-6"),
        )
        .expect("parse response");

        assert_eq!(
            response.content,
            vec![
                ResponseContent::HostedToolUse {
                    id: "srvtool_fetch".to_string(),
                    name: "web_fetch".to_string(),
                    input: json!({"url": "https://example.test/docs"}),
                    output: None,
                    status: None,
                },
                ResponseContent::HostedToolUse {
                    id: "srvtool_fetch".to_string(),
                    name: "web_fetch".to_string(),
                    input: json!({"url": "https://example.test/docs"}),
                    output: Some(json!({
                        "type": "web_fetch_result",
                        "url": "https://example.test/docs",
                        "title": "Docs"
                    })),
                    status: Some("completed".to_string()),
                },
            ]
        );
    }

    #[test]
    fn parse_stop_reason_matches_messages_contract() {
        assert_eq!(parse_stop_reason("end_turn"), StopReason::EndTurn);
        assert_eq!(parse_stop_reason("tool_use"), StopReason::ToolUse);
        assert_eq!(parse_stop_reason("max_tokens"), StopReason::MaxTokens);
        assert_eq!(parse_stop_reason("stop_sequence"), StopReason::StopSequence);
        assert_eq!(parse_stop_reason("pause_turn"), StopReason::EndTurn);
        assert_eq!(parse_stop_reason("refusal"), StopReason::EndTurn);
    }
}
