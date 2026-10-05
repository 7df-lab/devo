//! Google Generative Language (`generateContent`) provider adapter.

use std::pin::Pin;

use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::{
    ModelRequest, ModelResponse, ProviderWireApi, RequestContent, ResponseContent,
    ResponseMetadata, StopReason, StreamEvent, Usage,
};
use futures::Stream;
use reqwest::Client;
use serde_json::{Value, json};

use crate::http::invalid_status_error;
use crate::{
    ModelProviderSDK, ProviderAdapter, ProviderCapabilities, ProviderHttpOptions, merge_extra_body,
};

pub const DEFAULT_GOOGLE_GENERATIVE_AI_BASE_URL: &str =
    "https://generativelanguage.googleapis.com/v1beta";

pub struct GoogleGenerativeAiProvider {
    client: Client,
    base_url: String,
    api_key: String,
    http_options: ProviderHttpOptions,
}

impl GoogleGenerativeAiProvider {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        let http_options = ProviderHttpOptions::default();
        Self {
            client: http_options
                .build_request_client()
                .unwrap_or_else(|_| Client::new()),
            base_url: base_url.into(),
            api_key: api_key.into(),
            http_options,
        }
    }

    pub fn with_http_options(mut self, http_options: ProviderHttpOptions) -> Result<Self> {
        self.client = http_options.build_request_client()?;
        self.http_options = http_options;
        Ok(self)
    }

    fn endpoint(&self, model: &str) -> String {
        format!(
            "{}/models/{}:generateContent",
            self.base_url.trim_end_matches('/'),
            model
        )
    }

    fn request_builder(&self, request: &ModelRequest, body: &Value) -> reqwest::RequestBuilder {
        let builder = self
            .client
            .post(self.endpoint(&request.model))
            .header("x-goog-api-key", &self.api_key);
        self.http_options
            .apply_request_headers(
                self.http_options.apply_custom_headers(builder),
                &crate::request_headers(request.extra_body.as_ref()),
            )
            .json(body)
    }
}

#[async_trait]
impl ModelProviderSDK for GoogleGenerativeAiProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        let body = build_request(&request);
        let response = self
            .request_builder(&request, &body)
            .send()
            .await
            .context("failed to send Google Generative AI request")?;
        let response = match response.error_for_status_ref() {
            Ok(_) => response,
            Err(_) => {
                let status = response.status();
                return Err(invalid_status_error(
                    "google",
                    &request.model,
                    "request",
                    status,
                    response,
                    &body,
                )
                .await);
            }
        };
        let value = response
            .json()
            .await
            .context("failed to decode Google Generative AI response")?;
        parse_response(value)
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        // The normalized stream contract remains useful even when a compatible
        // endpoint only supports generateContent: surface the completed
        // response as text/tool events followed by MessageDone.
        let response = self.completion(request).await?;
        let stream_response = response.clone();
        let stream = async_stream::try_stream! {
            for (index, content) in stream_response.content.iter().enumerate() {
                match content {
                    ResponseContent::Text(text) => {
                        yield StreamEvent::TextStart { index };
                        yield StreamEvent::TextDelta { index, text: text.clone() };
                    }
                    ResponseContent::ToolUse { id, name, input } => {
                        yield StreamEvent::ToolCallStart {
                            index,
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        };
                    }
                    ResponseContent::HostedToolUse { .. }
                    | ResponseContent::ProviderReasoning { .. } => {}
                }
            }
            yield StreamEvent::UsageDelta(stream_response.usage.clone());
            yield StreamEvent::MessageDone { response: stream_response };
        };
        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "google-generative-ai"
    }
}

#[async_trait]
impl ProviderAdapter for GoogleGenerativeAiProvider {
    fn family(&self) -> ProviderWireApi {
        ProviderWireApi::GoogleGenerativeAi
    }

    fn capabilities(&self, _model_slug: &str) -> ProviderCapabilities {
        ProviderCapabilities {
            supported_roles: vec![
                devo_protocol::RequestRole::System,
                devo_protocol::RequestRole::User,
                devo_protocol::RequestRole::Assistant,
                devo_protocol::RequestRole::Tool,
            ],
            supports_reasoning_effort: false,
            supports_temperature: true,
            supports_top_p: true,
            supports_top_k: true,
            supports_tool_calls: true,
            require_reasoning_content: false,
        }
    }
}

fn build_request(request: &ModelRequest) -> Value {
    let mut contents = Vec::new();
    let tool_names = request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|content| match content {
            RequestContent::ToolUse { id, name, .. } => Some((id.as_str(), name.as_str())),
            RequestContent::Text { .. }
            | RequestContent::Reasoning { .. }
            | RequestContent::ProviderReasoning { .. }
            | RequestContent::HostedToolUse { .. }
            | RequestContent::ToolResult { .. }
            | RequestContent::Image { .. } => None,
        })
        .collect::<std::collections::HashMap<_, _>>();
    for message in &request.messages {
        let role = if message.role == "assistant" {
            "model"
        } else {
            "user"
        };
        let mut parts = Vec::new();
        for content in &message.content {
            match content {
                RequestContent::Text { text } => parts.push(json!({ "text": text })),
                RequestContent::Image {
                    mime_type,
                    data_base64,
                } => parts.push(json!({
                    "inlineData": { "mimeType": mime_type, "data": data_base64 }
                })),
                RequestContent::ToolUse { id, name, input } => parts.push(json!({
                    "functionCall": { "id": id, "name": name, "args": input }
                })),
                RequestContent::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    let response = serde_json::from_str(content)
                        .unwrap_or_else(|_| json!({ "result": content }));
                    parts.push(json!({
                        "functionResponse": {
                            "id": tool_use_id,
                            "name": tool_names.get(tool_use_id.as_str()).copied().unwrap_or(tool_use_id),
                            "response": response
                        }
                    }));
                }
                RequestContent::Reasoning { .. }
                | RequestContent::ProviderReasoning { .. }
                | RequestContent::HostedToolUse { .. } => {}
            }
        }
        if !parts.is_empty() {
            contents.push(json!({ "role": role, "parts": parts }));
        }
    }

    let mut root = json!({
        "contents": contents,
        "generationConfig": {
            "maxOutputTokens": request.max_tokens
        }
    });
    if let Some(system) = &request.system {
        root["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }
    if let Some(temperature) = request.sampling.temperature {
        root["generationConfig"]["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.sampling.top_p {
        root["generationConfig"]["topP"] = json!(top_p);
    }
    if let Some(top_k) = request.sampling.top_k {
        root["generationConfig"]["topK"] = json!(top_k);
    }
    if let Some(tools) = &request.tools {
        root["tools"] = json!([{
            "functionDeclarations": tools.iter().map(|tool| json!({
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.input_schema
            })).collect::<Vec<_>>()
        }]);
    }
    merge_extra_body(&mut root, request.extra_body.as_ref());
    root
}

fn parse_response(value: Value) -> Result<ModelResponse> {
    let candidate = value
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first());
    let mut content = Vec::new();
    if let Some(parts) = candidate
        .and_then(|candidate| candidate.pointer("/content/parts"))
        .and_then(Value::as_array)
    {
        for (index, part) in parts.iter().enumerate() {
            if let Some(text) = part.get("text").and_then(Value::as_str)
                && !text.is_empty()
            {
                content.push(ResponseContent::Text(text.to_string()));
            }
            if let Some(call) = part.get("functionCall") {
                content.push(ResponseContent::ToolUse {
                    id: call
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("google_tool_{index}")),
                    name: call
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    input: call.get("args").cloned().unwrap_or_else(|| json!({})),
                });
            }
        }
    }
    let finish_reason = candidate
        .and_then(|candidate| candidate.get("finishReason"))
        .and_then(Value::as_str);
    let has_tool_use = content
        .iter()
        .any(|item| matches!(item, ResponseContent::ToolUse { .. }));
    let stop_reason = match (finish_reason, has_tool_use) {
        (_, true) => Some(StopReason::ToolUse),
        (Some("MAX_TOKENS"), false) => Some(StopReason::MaxTokens),
        (Some("STOP"), false) => Some(StopReason::EndTurn),
        (Some(_), false) => Some(StopReason::StopSequence),
        (None, false) => None,
    };
    let usage = value.get("usageMetadata");
    let input_tokens = token_count(usage, "promptTokenCount");
    let output_tokens = token_count(usage, "candidatesTokenCount");
    Ok(ModelResponse {
        id: value
            .get("responseId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        content,
        stop_reason,
        usage: Usage {
            input_tokens,
            output_tokens,
            total_tokens: usage
                .and_then(|usage| usage.get("totalTokenCount"))
                .and_then(Value::as_u64)
                .map(|value| value as usize),
            ..Usage::default()
        },
        metadata: ResponseMetadata::default(),
    })
}

fn token_count(usage: Option<&Value>, field: &str) -> usize {
    usage
        .and_then(|usage| usage.get(field))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize
}

#[cfg(test)]
mod tests {
    use devo_protocol::{ModelProfileKey, SamplingControls};
    use pretty_assertions::assert_eq;

    use super::*;

    /// Trace: L2-DES-MODEL-002
    /// Verifies: Google requests use the v1beta model endpoint and API-key header.
    #[test]
    fn google_request_uses_model_url_and_api_key_header() {
        let provider =
            GoogleGenerativeAiProvider::new(DEFAULT_GOOGLE_GENERATIVE_AI_BASE_URL, "google-secret");
        let request = ModelRequest {
            model_slug: ModelProfileKey::Generic,
            model: "gemini-2.5-flash".to_string(),
            system: None,
            messages: Vec::new(),
            max_tokens: 64,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        };
        let built = provider
            .request_builder(&request, &build_request(&request))
            .build()
            .expect("build Google request");

        assert_eq!(
            built.url().as_str(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent"
        );
        assert_eq!(
            built.headers()["x-goog-api-key"].to_str().expect("header"),
            "google-secret"
        );
    }
}
