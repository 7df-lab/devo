use std::pin::Pin;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use reqwest::header::AUTHORIZATION;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use tracing::debug;
mod stream;
use devo_protocol::ModelProfileKey;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ProviderWireApi;
use devo_protocol::RequestContent;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseExtra;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;

use super::capabilities::OpenAIReasoningMode;
use super::capabilities::OpenAITransport;
use super::capabilities::resolve_request_profile;
use super::shared::OpenAICompletionUsage;
use super::shared::completion_usage_to_usage;
use super::shared::deserialize_null_vec;
use super::shared::reasoning_value;
use super::shared::request_role;
use super::shared::tool_definitions;
use crate::ModelProviderSDK;
use crate::ProviderAdapter;
use crate::ProviderCapabilities;
use crate::ProviderHttpOptions;
use crate::dsml::DsmlToolCallHealer;
use crate::hosted_tools::apply_openai_chat_completions_hosted_tools;
use crate::http::invalid_status_error;
use crate::merge_extra_body;
use crate::text_normalization::split_tagged_text;

/// OpenAI chat-completion provider backed by the official HTTP API.
/// <https://developers.openai.com/api/reference/chat-completions/overview>
/// Works with OpenAI chat-completion servers by changing the base URL.
pub struct OpenAIProvider {
    client: Client,
    streaming_client: Client,
    base_url: String,
    api_key: Option<String>,
    http_options: ProviderHttpOptions,
}

impl OpenAIProvider {
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
            http_options,
        }
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn with_http_options(mut self, http_options: ProviderHttpOptions) -> Result<Self> {
        self.client = http_options.build_request_client()?;
        self.streaming_client = http_options.build_streaming_client()?;
        self.http_options = http_options;
        Ok(self)
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    fn post_builder(
        &self,
        client: &Client,
        body: &Value,
        headers: &std::collections::BTreeMap<String, String>,
    ) -> reqwest::RequestBuilder {
        let builder = client
            .post(self.endpoint())
            .header(CONTENT_TYPE, "application/json");
        let builder = self
            .http_options
            .apply_request_headers(self.http_options.apply_custom_headers(builder), headers);
        let builder = if let Some(api_key) = &self.api_key {
            builder.header(AUTHORIZATION, format!("Bearer {api_key}"))
        } else {
            builder
        };
        builder.json(body)
    }

    fn request_builder(
        &self,
        body: &Value,
        headers: &std::collections::BTreeMap<String, String>,
    ) -> reqwest::RequestBuilder {
        self.post_builder(&self.client, body, headers)
    }

    pub(super) fn streaming_request_builder(
        &self,
        body: &Value,
        headers: &std::collections::BTreeMap<String, String>,
    ) -> reqwest::RequestBuilder {
        self.post_builder(&self.streaming_client, body, headers)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAIChatCompletionResponse {
    id: String,
    #[serde(default, deserialize_with = "deserialize_null_vec")]
    choices: Vec<OpenAIChatCompletionChoice>,
    #[serde(default)]
    created: Option<u64>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    object: Option<String>,
    #[serde(default)]
    service_tier: Option<String>,
    #[serde(default)]
    system_fingerprint: Option<String>,
    #[serde(default)]
    usage: Option<OpenAICompletionUsage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAIChatCompletionChoice {
    #[serde(default)]
    finish_reason: Option<String>,
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    logprobs: Option<OpenAIChoiceLogprobs>,
    #[serde(default)]
    message: Option<OpenAIChatCompletionMessage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAIChoiceLogprobs {
    #[serde(default)]
    content: Vec<OpenAIChatCompletionTokenLogprob>,
    #[serde(default)]
    refusal: Vec<OpenAIChatCompletionTokenLogprob>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAIChatCompletionTokenLogprob {
    token: String,
    #[serde(default)]
    bytes: Option<Vec<u8>>,
    logprob: f64,
    #[serde(default)]
    top_logprobs: Vec<OpenAIChatCompletionTopLogprob>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAIChatCompletionTopLogprob {
    token: String,
    #[serde(default)]
    bytes: Option<Vec<u8>>,
    logprob: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAIChatCompletionMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    refusal: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_vec")]
    annotations: Vec<OpenAIChatCompletionAnnotation>,
    #[serde(default)]
    audio: Option<OpenAIChatCompletionAudio>,
    #[serde(default)]
    function_call: Option<OpenAIChatCompletionFunctionCall>,
    #[serde(default, deserialize_with = "deserialize_null_vec")]
    tool_calls: Vec<OpenAIChatCompletionMessageToolCall>,
    /// DeepSeek / vLLM use `reasoning_content`; Ollama's OpenAI-compat layer
    /// currently emits the same payload under `reasoning`.
    #[serde(default, alias = "reasoning")]
    reasoning_content: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAIChatCompletionAnnotation {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    url_citation: Option<OpenAIChatCompletionUrlCitation>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAIChatCompletionUrlCitation {
    #[serde(default)]
    end_index: Option<u64>,
    #[serde(default)]
    start_index: Option<u64>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAIChatCompletionAudio {
    id: String,
    data: String,
    expires_at: u64,
    transcript: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAIChatCompletionFunctionCall {
    arguments: String,
    name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAIChatCompletionMessageToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    function: Option<OpenAIChatCompletionFunctionCall>,
    #[serde(default)]
    custom: Option<OpenAIChatCompletionCustomToolCall>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct OpenAIChatCompletionCustomToolCall {
    input: String,
    name: String,
}

/// Chat completions request body. See <https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create>.
/// Emits a user message whose content came from multimodal parts.
///
/// Text-only parts collapse to a plain string so strict OpenAI-compatible
/// servers keep receiving the widely supported string form; images force the
/// content-parts array form.
fn push_user_content_message(
    messages: &mut Vec<Value>,
    role: super::OpenAIRole,
    parts: Vec<Value>,
) {
    let only_text = parts
        .iter()
        .all(|part| part.get("type").and_then(Value::as_str) == Some("text"));
    let content = if only_text {
        let text = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
        Value::String(text)
    } else {
        Value::Array(parts)
    };
    messages.push(json!({ "role": role, "content": content }));
}

fn build_request(request: &ModelRequest, stream: bool) -> Value {
    let profile = resolve_request_profile(&request.model_slug, OpenAITransport::ChatCompletions);
    let include_empty_reasoning_content = profile.require_reasoning_content
        && !matches!(
            request
                .request_thinking
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str(),
            "disabled" | "none"
        );
    let mut messages = Vec::new();
    if let Some(system) = &request.system {
        messages.push(json!({ "role": super::OpenAIRole::System, "content": system }));
    }

    for message in &request.messages {
        match request_role(&message.role) {
            super::OpenAIRole::Assistant => {
                let mut text_parts = Vec::new();
                let mut reasoning_parts = Vec::new();
                let mut tool_calls = Vec::new();
                for block in &message.content {
                    match block {
                        RequestContent::Text { text } => text_parts.push(text.clone()),
                        RequestContent::Reasoning { text } => reasoning_parts.push(text.clone()),
                        RequestContent::ProviderReasoning { .. } => {}
                        RequestContent::ToolUse { id, name, input } => tool_calls.push(json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": input.to_string(),
                            }
                        })),
                        RequestContent::HostedToolUse { .. } => {}
                        RequestContent::ToolResult { .. } => {}
                        RequestContent::Image { .. } => {}
                    }
                }
                if text_parts.is_empty() && reasoning_parts.is_empty() && tool_calls.is_empty() {
                    continue;
                }
                let mut entry = json!({ "role": super::OpenAIRole::Assistant });
                entry["content"] = if text_parts.is_empty() {
                    Value::String(String::new())
                } else {
                    Value::String(text_parts.join(""))
                };
                if include_empty_reasoning_content || !reasoning_parts.is_empty() {
                    entry["reasoning_content"] = Value::String(reasoning_parts.join(""));
                }
                if !tool_calls.is_empty() {
                    entry["tool_calls"] = Value::Array(tool_calls);
                }
                messages.push(entry);
            }
            role => {
                let mut multimodal_parts = Vec::new();
                for block in &message.content {
                    match block {
                        RequestContent::Text { text } if role == super::OpenAIRole::User => {
                            multimodal_parts.push(json!({ "type": "text", "text": text }));
                        }
                        RequestContent::Image {
                            mime_type,
                            data_base64,
                        } if role == super::OpenAIRole::User => {
                            multimodal_parts.push(json!({
                                "type": "image_url",
                                "image_url": {
                                    "url": format!("data:{mime_type};base64,{data_base64}")
                                }
                            }));
                        }
                        RequestContent::Text { text } => {
                            if !multimodal_parts.is_empty() {
                                push_user_content_message(&mut messages, role, multimodal_parts);
                                multimodal_parts = Vec::new();
                            }
                            messages.push(json!({ "role": role, "content": text }));
                        }
                        RequestContent::Image { .. } => {}
                        RequestContent::Reasoning { .. } => {}
                        RequestContent::ProviderReasoning { .. } => {}
                        RequestContent::HostedToolUse { .. } => {}
                        RequestContent::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            if !multimodal_parts.is_empty() {
                                push_user_content_message(&mut messages, role, multimodal_parts);
                                multimodal_parts = Vec::new();
                            }
                            messages.push(json!({
                                "role": super::OpenAIRole::Tool,
                                "tool_call_id": tool_use_id,
                                "content": content,
                            }));
                        }
                        RequestContent::ToolUse { .. } => {}
                    }
                }
                if !multimodal_parts.is_empty() {
                    push_user_content_message(&mut messages, role, multimodal_parts);
                }
            }
        }
    }

    let mut root = json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": request.max_tokens,
        "stream": stream,
    });

    if let Some(tools) = &request.tools {
        root["tools"] = tool_definitions(tools);
    }

    if profile.supports_temperature
        && let Some(temperature) = request.sampling.temperature
    {
        root["temperature"] = json!(temperature);
    }

    if profile.supports_top_p
        && let Some(top_p) = request.sampling.top_p
    {
        root["top_p"] = json!(top_p);
    }

    if profile.supports_top_k
        && let Some(top_k) = request.sampling.top_k
    {
        root["top_k"] = json!(top_k);
    }

    if let Some(payload) = reasoning_value(
        profile,
        request.request_thinking.as_deref(),
        request.reasoning_effort,
    ) {
        match payload {
            super::shared::OpenAIReasoningValue::Effort(effort) => {
                root["reasoning_effort"] = json!(effort);
            }
            super::shared::OpenAIReasoningValue::Thinking { enabled } => {
                root["thinking"] = json!({
                    "type": if enabled { "enabled" } else { "disabled" },
                });
            }
            super::shared::OpenAIReasoningValue::ThinkingWithEffort { enabled, effort } => {
                root["thinking"] = json!({
                    "type": if enabled { "enabled" } else { "disabled" },
                });
                if let Some(effort) = effort {
                    root["reasoning_effort"] = json!(effort);
                }
            }
        }
    }

    if stream {
        root["stream_options"] = json!({ "include_usage": true });
    }

    apply_openai_chat_completions_hosted_tools(&mut root, &request.hosted_tools);

    merge_extra_body(&mut root, request.extra_body.as_ref());

    root
}

/// Chat completions response body. See <https://developers.openai.com/api/reference/resources/chat/subresources/completions>.
fn parse_response(value: Value, dsml_healer: &DsmlToolCallHealer) -> Result<ModelResponse> {
    let response: OpenAIChatCompletionResponse = serde_json::from_value(value.clone())
        .context("failed to deserialize openai chat-completion response")?;
    let mut content = Vec::new();
    let mut stop_reason = None;
    let mut metadata = ResponseMetadata::default();

    if let Some(choice) = response.choices.first() {
        if let Some(message) = &choice.message {
            if let Some(reasoning_content) = &message.reasoning_content {
                metadata.extras.push(ResponseExtra::ReasoningText {
                    text: reasoning_content.clone(),
                });
            }
            if let Some(text) = &message.content {
                let (assistant_text, reasoning) = split_tagged_text(text);
                for text in reasoning {
                    if !text.is_empty() {
                        metadata.extras.push(ResponseExtra::ReasoningText { text });
                    }
                }
                if !assistant_text.is_empty() {
                    content.push(ResponseContent::Text(assistant_text));
                }
            }
            for tool_call in &message.tool_calls {
                if let Some(parsed) = parse_tool_use(tool_call) {
                    content.push(parsed);
                }
            }
        }
        if let Some(reason) = &choice.finish_reason {
            stop_reason = Some(parse_finish_reason(reason));
        }
    }
    let content = dsml_healer.heal_response_content(content);

    let usage = response
        .usage
        .as_ref()
        .map(completion_usage_to_usage)
        .unwrap_or_default();

    if let Some(provider_payload) = build_provider_specific_response_payload(&response) {
        metadata.extras.push(ResponseExtra::ProviderSpecific {
            provider: "openai".to_string(),
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

pub(super) fn parse_tool_use(
    value: &OpenAIChatCompletionMessageToolCall,
) -> Option<ResponseContent> {
    match value.kind.as_str() {
        "function" => {
            let function = value.function.as_ref()?;
            let input = serde_json::from_str(&function.arguments)
                .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
            Some(ResponseContent::ToolUse {
                id: value.id.clone(),
                name: function.name.clone(),
                input,
            })
        }
        "custom" => {
            let custom = value.custom.as_ref()?;
            Some(ResponseContent::ToolUse {
                id: value.id.clone(),
                name: custom.name.clone(),
                input: Value::String(custom.input.clone()),
            })
        }
        _ => None,
    }
}

pub(super) fn build_provider_specific_response_payload(
    response: &OpenAIChatCompletionResponse,
) -> Option<Value> {
    let mut payload = serde_json::Map::new();

    if let Some(created) = response.created {
        payload.insert("created".to_string(), json!(created));
    }
    if let Some(model) = &response.model {
        payload.insert("model".to_string(), json!(model));
    }
    if let Some(object) = &response.object {
        payload.insert("object".to_string(), json!(object));
    }
    if let Some(service_tier) = &response.service_tier {
        payload.insert("service_tier".to_string(), json!(service_tier));
    }
    if let Some(system_fingerprint) = &response.system_fingerprint {
        payload.insert("system_fingerprint".to_string(), json!(system_fingerprint));
    }
    if let Some(usage) = &response.usage {
        payload.insert("usage".to_string(), json!(usage));
    }

    let choices = response
        .choices
        .iter()
        .filter_map(build_provider_specific_choice_payload)
        .collect::<Vec<_>>();
    if !choices.is_empty() {
        payload.insert("choices".to_string(), Value::Array(choices));
    }

    if payload.is_empty() {
        None
    } else {
        Some(Value::Object(payload))
    }
}

fn build_provider_specific_choice_payload(choice: &OpenAIChatCompletionChoice) -> Option<Value> {
    let mut payload = serde_json::Map::new();

    if let Some(index) = choice.index {
        payload.insert("index".to_string(), json!(index));
    }
    if let Some(logprobs) = &choice.logprobs {
        payload.insert("logprobs".to_string(), json!(logprobs));
    }
    if let Some(message) = &choice.message
        && let Some(message_payload) = build_provider_specific_message_payload(message)
    {
        payload.insert("message".to_string(), message_payload);
    }

    if payload.is_empty() {
        None
    } else {
        Some(Value::Object(payload))
    }
}

fn build_provider_specific_message_payload(message: &OpenAIChatCompletionMessage) -> Option<Value> {
    let mut payload = serde_json::Map::new();

    if let Some(role) = &message.role {
        payload.insert("role".to_string(), json!(role));
    }
    if let Some(refusal) = &message.refusal {
        payload.insert("refusal".to_string(), json!(refusal));
    }
    if !message.annotations.is_empty() {
        payload.insert("annotations".to_string(), json!(message.annotations));
    }
    if let Some(audio) = &message.audio {
        payload.insert("audio".to_string(), json!(audio));
    }
    if let Some(function_call) = &message.function_call {
        payload.insert("function_call".to_string(), json!(function_call));
    }
    let custom_tool_calls = message
        .tool_calls
        .iter()
        .filter(|tool_call| tool_call.kind == "custom")
        .cloned()
        .collect::<Vec<_>>();
    if !custom_tool_calls.is_empty() {
        payload.insert("tool_calls".to_string(), json!(custom_tool_calls));
    }

    if payload.is_empty() {
        None
    } else {
        Some(Value::Object(payload))
    }
}

fn parse_finish_reason(value: &str) -> StopReason {
    match value {
        "tool_calls" => StopReason::ToolUse,
        "function_call" => StopReason::ToolUse,
        "length" => StopReason::MaxTokens,
        "stop" => StopReason::EndTurn,
        "content_filter" => StopReason::StopSequence,
        _ => StopReason::EndTurn,
    }
}

#[async_trait]
impl ModelProviderSDK for OpenAIProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        let body = build_request(&request, false);
        debug!(
            provider = "openai",
            api_base = %self.base_url,
            model = %request.model,
            messages = request.messages.len(),
            tools = request.tools.as_ref().map_or(0, Vec::len),
            max_tokens = request.max_tokens,
            "sending openai completion request"
        );

        let response = self
            .request_builder(&body, &crate::request_headers(request.extra_body.as_ref()))
            .send()
            .await
            .context("failed to send openai request")?;
        let response = match response.error_for_status_ref() {
            Ok(_) => response,
            Err(_) => {
                let status = response.status();
                return Err(invalid_status_error(
                    "openai",
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
            .context("failed to decode openai response")?;
        parse_response(value, &DsmlToolCallHealer::for_request(&request))
    }

    /// --------- Here is an example of stream response ------------------------
    /// ```text
    /// {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1694268190,"model":"gpt-4o-mini", "system_fingerprint": "fp_44709d6fcb", "choices":[{"index":0,"delta":{"role":"assistant","content":""},"logprobs":null,"finish_reason":null}]}
    /// {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1694268190,"model":"gpt-4o-mini", "system_fingerprint": "fp_44709d6fcb", "choices":[{"index":0,"delta":{"content":"Hello"},"logprobs":null,"finish_reason":null}]}
    /// ....
    /// {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1694268190,"model":"gpt-4o-mini", "system_fingerprint": "fp_44709d6fcb", "choices":[{"index":0,"delta":{},"logprobs":null,"finish_reason":"stop"}]}
    /// ```
    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        stream::completion_stream(self, request).await
    }

    fn name(&self) -> &str {
        "openai"
    }
}

#[async_trait]
impl ProviderAdapter for OpenAIProvider {
    fn family(&self) -> ProviderWireApi {
        ProviderWireApi::OpenAIChatCompletions
    }

    fn capabilities(&self, model_slug: &str) -> ProviderCapabilities {
        let profile = resolve_request_profile(
            &ModelProfileKey::CatalogSlug(model_slug.to_string()),
            OpenAITransport::ChatCompletions,
        );
        let mut capabilities = ProviderCapabilities::openai();
        capabilities.supports_temperature = profile.supports_temperature;
        capabilities.supports_top_p = profile.supports_top_p;
        capabilities.supports_reasoning_effort = matches!(
            profile.reasoning_mode,
            OpenAIReasoningMode::Effort | OpenAIReasoningMode::ThinkingWithEffort
        );
        capabilities.supports_top_k = profile.supports_top_k;
        capabilities.require_reasoning_content = profile.require_reasoning_content;
        capabilities.supported_roles = profile.supported_roles.to_vec();
        capabilities
    }
}

#[cfg(test)]
mod tests {
    use crate::dsml::DsmlToolCallHealer;
    use devo_protocol::ModelProfileKey;
    use devo_protocol::ModelRequest;
    use devo_protocol::RequestContent;
    use devo_protocol::RequestMessage;
    use devo_protocol::SamplingControls;
    use devo_protocol::ToolDefinition;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::parse_finish_reason;
    use super::parse_response;
    use devo_protocol::ResponseContent;
    use devo_protocol::ResponseExtra;
    use devo_protocol::StopReason;

    use crate::openai::chat_completions::build_request;

    #[test]
    fn debug_request_body_includes_tools_and_reasoning_effort() {
        let request = ModelRequest {
            model_slug: ModelProfileKey::CatalogSlug("gpt-4o-mini".to_string()),
            model: "gpt-4o-mini".to_string(),
            system: Some("You are helpful.".to_string()),
            messages: vec![
                RequestMessage {
                    role: "assistant".to_string(),
                    content: vec![
                        RequestContent::Reasoning {
                            text: "Need to inspect weather data first.".to_string(),
                        },
                        RequestContent::Text {
                            text: "Calling tool".to_string(),
                        },
                        RequestContent::ToolUse {
                            id: "call_123".to_string(),
                            name: "get_weather".to_string(),
                            input: json!({"city": "Boston"}),
                        },
                    ],
                },
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![RequestContent::ToolResult {
                        tool_use_id: "call_123".to_string(),
                        content: "{\"temp\":72}".to_string(),
                        is_error: Some(false),
                    }],
                },
            ],
            max_tokens: 256,
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
                ..SamplingControls::default()
            },
            request_thinking: Some("medium".to_string()),
            reasoning_effort: Some(devo_protocol::ReasoningEffort::Medium),
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(body["model"], json!("gpt-4o-mini"));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["max_tokens"], json!(256));
        assert_eq!(body["reasoning_effort"], json!("medium"));
        assert_eq!(body["temperature"], json!(0.2));
        assert_eq!(body["tools"][0]["type"], json!("function"));
        assert_eq!(body["messages"][1]["role"], json!("assistant"));
        assert_eq!(
            body["messages"][1]["reasoning_content"],
            json!("Need to inspect weather data first.")
        );
        assert_eq!(
            body["messages"][1]["tool_calls"][0]["function"]["arguments"],
            json!("{\"city\":\"Boston\"}")
        );
        assert_eq!(body["messages"][1]["content"], json!("Calling tool"));
        assert_eq!(body["messages"][2]["role"], json!("tool"));
        assert_eq!(body["messages"][2]["tool_call_id"], json!("call_123"));
    }

    #[test]
    fn debug_request_body_uses_thinking_and_replays_reasoning_for_zai_models() {
        let request = ModelRequest {
            model_slug: ModelProfileKey::CatalogSlug("glm-4.5".to_string()),
            model: "renamed-provider-model".to_string(),
            system: None,
            messages: vec![
                RequestMessage {
                    role: "assistant".to_string(),
                    content: vec![
                        RequestContent::Reasoning {
                            text: "prior plan".to_string(),
                        },
                        RequestContent::Text {
                            text: "prior answer".to_string(),
                        },
                    ],
                },
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![RequestContent::Text {
                        text: "hi".to_string(),
                    }],
                },
            ],
            max_tokens: 64,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: Some("enabled".to_string()),
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, false);

        assert_eq!(body["model"], json!("renamed-provider-model"));
        assert_eq!(body["thinking"], json!({ "type": "enabled" }));
        assert_eq!(body["messages"][0]["role"], json!("assistant"));
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            json!("prior plan")
        );
        assert_eq!(body["messages"][0]["content"], json!("prior answer"));
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn debug_request_body_includes_sampling_controls_for_capable_models() {
        let request = ModelRequest {
            model_slug: ModelProfileKey::CatalogSlug("glm-4.5".to_string()),
            model: "renamed-provider-model".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "hi".to_string(),
                }],
            }],
            max_tokens: 64,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls {
                temperature: Some(0.3),
                top_p: Some(0.9),
                top_k: Some(40),
            },
            request_thinking: Some("enabled".to_string()),
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, false);

        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["temperature"], json!(0.3));
        assert_eq!(body["top_p"], json!(0.9));
        assert_eq!(body["top_k"], json!(40));
    }

    #[test]
    fn debug_request_body_preserves_top_p_precision() {
        let request = ModelRequest {
            model_slug: ModelProfileKey::CatalogSlug("glm-5.1".to_string()),
            model: "glm-5.1".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "Reply with OK only.".to_string(),
                }],
            }],
            max_tokens: 8192,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls {
                temperature: Some(1.0),
                top_p: Some(0.95),
                top_k: None,
            },
            request_thinking: Some("enabled".to_string()),
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(body["top_p"], json!(0.95));
    }

    #[test]
    fn parse_response_extracts_text_tool_calls_and_usage() {
        let response = parse_response(
            json!({
                "id": "chatcmpl-123",
                "object": "chat.completion",
                "created": 1741569952,
                "model": "gpt-5.4",
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [
                                {
                                    "id": "call_abc123",
                                    "type": "function",
                                    "function": {
                                        "name": "get_weather",
                                        "arguments": "{\"location\":\"Boston, MA\"}"
                                    }
                                }
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }
                ],
                "usage": {
                    "prompt_tokens": 82,
                    "completion_tokens": 17,
                    "total_tokens": 99,
                    "prompt_tokens_details": {
                        "cached_tokens": 12,
                        "audio_tokens": 0
                    },
                    "completion_tokens_details": {
                        "reasoning_tokens": 4,
                        "audio_tokens": 0
                    }
                },
                "service_tier": "default"
            }),
            &DsmlToolCallHealer::for_model("gpt-5.4"),
        )
        .expect("parse response");

        assert_eq!(response.id, "chatcmpl-123");
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(response.usage.input_tokens, 82);
        assert_eq!(response.usage.output_tokens, 17);
        assert_eq!(response.usage.cache_read_input_tokens, Some(12));
        assert_eq!(response.usage.reasoning_output_tokens, Some(4));
        assert_eq!(response.usage.total_tokens, Some(99));
        assert_eq!(response.content.len(), 1);
        match &response.content[0] {
            ResponseContent::ToolUse { id, name, input } => {
                assert_eq!(id, "call_abc123");
                assert_eq!(name, "get_weather");
                assert_eq!(input, &json!({"location": "Boston, MA"}));
            }
            other => panic!("expected tool use, got {other:?}"),
        }
        assert!(response.metadata.extras.iter().any(|extra| matches!(
            extra,
            ResponseExtra::ProviderSpecific { provider, .. } if provider == "openai"
        )));
    }

    #[test]
    fn parse_response_preserves_text_content() {
        let response = parse_response(
            json!({
                "id": "chatcmpl-456",
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "Hello! How can I assist you today?"
                        },
                        "finish_reason": "stop"
                    }
                ],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 8
                }
            }),
            &DsmlToolCallHealer::for_model("gpt-5.4"),
        )
        .expect("parse response");

        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(response.content.len(), 1);
        match &response.content[0] {
            ResponseContent::Text(text) => {
                assert_eq!(text, "Hello! How can I assist you today?");
            }
            other => panic!("expected text response, got {other:?}"),
        }
    }

    #[test]
    fn parse_response_reads_ollama_reasoning_field_alias() {
        let response = parse_response(
            json!({
                "id": "chatcmpl-ollama",
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "Hello!",
                            "reasoning": "plan via ollama"
                        },
                        "finish_reason": "stop"
                    }
                ]
            }),
            &DsmlToolCallHealer::for_model("qwen3"),
        )
        .expect("parse response");

        assert_eq!(
            response.content,
            vec![ResponseContent::Text("Hello!".to_string())]
        );
        assert!(response.metadata.extras.iter().any(|extra| matches!(
            extra,
            ResponseExtra::ReasoningText { text } if text == "plan via ollama"
        )));
    }

    #[test]
    fn parse_response_heals_deepseek_v4_dsml_text_tool_calls() {
        let response = parse_response(
            json!({
                "id": "chatcmpl-dsml",
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "<｜DSML｜tool_calls><｜DSML｜invoke name=\"web_search\"><｜DSML｜parameter name=\"query\" string=\"true\">DeepSeek V4 DSML</｜DSML｜parameter></｜DSML｜invoke></｜DSML｜tool_calls>"
                        },
                        "finish_reason": "tool_calls"
                    }
                ],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 8
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
                input: json!({"query": "DeepSeek V4 DSML"}),
            }]
        );
    }

    #[test]
    fn parse_finish_reason_matches_chat_completion_contract() {
        assert_eq!(parse_finish_reason("tool_calls"), StopReason::ToolUse);
        assert_eq!(parse_finish_reason("length"), StopReason::MaxTokens);
        assert_eq!(parse_finish_reason("stop"), StopReason::EndTurn);
        assert_eq!(
            parse_finish_reason("content_filter"),
            StopReason::StopSequence
        );
        assert_eq!(parse_finish_reason("function_call"), StopReason::ToolUse);
    }

    #[test]
    fn parse_response_preserves_provider_specific_response_fields() {
        let response = parse_response(
            json!({
                "id": "chatcmpl-789",
                "object": "chat.completion",
                "created": 1741569952,
                "model": "gpt-5.4",
                "service_tier": "default",
                "system_fingerprint": "fp_123",
                "choices": [
                    {
                        "index": 0,
                        "logprobs": {
                            "content": [],
                            "refusal": []
                        },
                        "message": {
                            "role": "assistant",
                            "content": "hello",
                            "refusal": "none",
                            "annotations": [
                                {
                                    "type": "url_citation",
                                    "url_citation": {
                                        "start_index": 0,
                                        "end_index": 5,
                                        "title": "Example",
                                        "url": "https://example.com"
                                    }
                                }
                            ],
                            "audio": {
                                "id": "aud_1",
                                "data": "Zm9v",
                                "expires_at": 1741569999u64,
                                "transcript": "hello"
                            }
                        },
                        "finish_reason": "stop"
                    }
                ],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 8,
                    "total_tokens": 18
                }
            }),
            &DsmlToolCallHealer::for_model("gpt-5.4"),
        )
        .expect("parse response");

        let provider_payload = response
            .metadata
            .extras
            .iter()
            .find_map(|extra| match extra {
                ResponseExtra::ProviderSpecific { provider, payload } if provider == "openai" => {
                    Some(payload)
                }
                _ => None,
            })
            .expect("provider-specific metadata");

        assert_eq!(provider_payload["object"], json!("chat.completion"));
        assert_eq!(provider_payload["model"], json!("gpt-5.4"));
        assert_eq!(provider_payload["service_tier"], json!("default"));
        assert_eq!(provider_payload["system_fingerprint"], json!("fp_123"));
        assert_eq!(
            provider_payload["choices"][0]["message"]["annotations"][0]["type"],
            json!("url_citation")
        );
    }

    #[test]
    fn user_text_only_content_serializes_as_string_and_images_as_parts() {
        let request = ModelRequest {
            model_slug: ModelProfileKey::CatalogSlug("gpt-4o-mini".to_string()),
            model: "gpt-4o-mini".to_string(),
            system: None,
            messages: vec![
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![RequestContent::Text {
                        text: "text-only prompt".to_string(),
                    }],
                },
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![
                        RequestContent::Text {
                            text: "caption".to_string(),
                        },
                        RequestContent::Image {
                            mime_type: "image/png".to_string(),
                            data_base64: "aGVsbG8=".to_string(),
                        },
                    ],
                },
            ],
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
            max_tokens: 0,
        };

        let body = build_request(&request, false);

        assert_eq!(body["messages"][0]["role"], json!("user"));
        assert_eq!(body["messages"][0]["content"], json!("text-only prompt"));
        assert_eq!(body["messages"][1]["role"], json!("user"));
        assert_eq!(
            body["messages"][1]["content"],
            json!([
                { "type": "text", "text": "caption" },
                {
                    "type": "image_url",
                    "image_url": { "url": "data:image/png;base64,aGVsbG8=" }
                }
            ])
        );
    }

    #[test]
    fn build_request_omits_unsupported_hosted_tool_history() {
        let request = ModelRequest {
            model_slug: ModelProfileKey::CatalogSlug("gpt-4o-mini".to_string()),
            model: "gpt-4o-mini".to_string(),
            system: None,
            messages: vec![
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![RequestContent::Text {
                        text: "before".to_string(),
                    }],
                },
                RequestMessage {
                    role: "assistant".to_string(),
                    content: vec![
                        RequestContent::HostedToolUse {
                            id: "hosted_ws_1".to_string(),
                            name: "web_search".to_string(),
                            input: json!({"query": "Rust docs"}),
                            output: None,
                            status: None,
                        },
                        RequestContent::HostedToolUse {
                            id: "hosted_ws_1".to_string(),
                            name: "web_search".to_string(),
                            input: json!({"query": "Rust docs"}),
                            output: Some(json!([{
                                "title": "Rust documentation",
                                "url": "https://example.test/rust"
                            }])),
                            status: Some("completed".to_string()),
                        },
                    ],
                },
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![RequestContent::Text {
                        text: "after".to_string(),
                    }],
                },
            ],
            max_tokens: 256,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, false);

        assert_eq!(body["messages"].as_array().map(Vec::len), Some(2));
        assert_eq!(body["messages"][0]["role"], json!("user"));
        assert_eq!(body["messages"][1]["role"], json!("user"));
        let serialized = serde_json::to_string(&body).expect("serialize request body");
        assert!(!serialized.contains("hosted_tool_use"));
        assert!(!serialized.contains("web_search_tool_result"));
    }

    #[test]
    fn debug_request_body_uses_explicit_reasoning_effort_field() {
        let request = ModelRequest {
            model_slug: ModelProfileKey::CatalogSlug("deepseek-v4".to_string()),
            model: "deepseek-v4".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "hi".to_string(),
                }],
            }],
            max_tokens: 64,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: Some("enabled".to_string()),
            reasoning_effort: Some(devo_protocol::ReasoningEffort::Max),
            extra_body: None,
        };

        let body = build_request(&request, false);

        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["reasoning_effort"], json!("max"));
    }

    #[test]
    fn debug_request_body_includes_empty_reasoning_content_for_deepseek_assistant_messages() {
        let request = ModelRequest {
            model_slug: ModelProfileKey::CatalogSlug("deepseek-v4".to_string()),
            model: "deepseek-v4".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "assistant".to_string(),
                content: vec![RequestContent::Text {
                    text: "Prior assistant reply".to_string(),
                }],
            }],
            max_tokens: 64,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: Some("enabled".to_string()),
            reasoning_effort: Some(devo_protocol::ReasoningEffort::High),
            extra_body: None,
        };

        let body = build_request(&request, false);

        assert_eq!(body["messages"][0]["role"], json!("assistant"));
        assert_eq!(
            body["messages"][0]["content"],
            json!("Prior assistant reply")
        );
        assert_eq!(body["messages"][0]["reasoning_content"], json!(""));
    }
}
