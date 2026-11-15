//! Google Generative Language (`generateContent`) provider adapter.

use std::pin::Pin;

use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::{
    ModelRequest, ModelResponse, ProviderWireApi, RequestContent, ResponseContent, ResponseExtra,
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
        // response as text/tool events followed by MessageDone. This is a
        // synthetic stream: no content or usage arrives before the full
        // generateContent response completes.
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
            if stream_response.usage.is_reported() {
                yield StreamEvent::UsageDelta(stream_response.usage.clone());
            }
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
                    // Gemini requires functionResponse.response to be a JSON object.
                    let response = match serde_json::from_str::<Value>(content) {
                        Ok(value @ Value::Object(_)) => value,
                        Ok(value) => json!({ "result": value }),
                        Err(_) => json!({ "result": content }),
                    };
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
    let mut metadata = ResponseMetadata::default();
    if let Some(parts) = candidate
        .and_then(|candidate| candidate.pointer("/content/parts"))
        .and_then(Value::as_array)
    {
        for (index, part) in parts.iter().enumerate() {
            if let Some(text) = part.get("text").and_then(Value::as_str)
                && !text.is_empty()
            {
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    metadata.extras.push(ResponseExtra::ReasoningText {
                        text: text.to_string(),
                    });
                } else {
                    content.push(ResponseContent::Text(text.to_string()));
                }
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
    let mut normalized_usage = Usage {
        reported: input_tokens.is_some() || output_tokens.is_some(),
        input_tokens: input_tokens.unwrap_or(0),
        output_tokens: output_tokens.unwrap_or(0),
        cache_read_input_tokens: token_count(usage, "cachedContentTokenCount"),
        reasoning_output_tokens: token_count(usage, "thoughtsTokenCount"),
        total_tokens: usage
            .and_then(|usage| usage.get("totalTokenCount"))
            .and_then(Value::as_u64)
            .map(|value| value as usize),
        ..Usage::default()
    }
    .with_field_coverage(input_tokens, output_tokens);
    // Google candidate tokens exclude thoughts, so input + candidate tokens
    // is only a subtotal when thoughts are reported without a provider total.
    if normalized_usage.total_tokens.is_none()
        && normalized_usage.reasoning_output_tokens.is_some()
        && let Some(unreported) = &mut normalized_usage.unreported_fields
        && !unreported.contains(&devo_protocol::native::usage::UsageField::TotalTokens)
    {
        unreported.push(devo_protocol::native::usage::UsageField::TotalTokens);
    }
    Ok(ModelResponse {
        id: value
            .get("responseId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        content,
        stop_reason,
        usage: normalized_usage,
        metadata,
    })
}

fn token_count(usage: Option<&Value>, field: &str) -> Option<usize> {
    usage
        .and_then(|usage| usage.get(field))
        .and_then(Value::as_u64)
        .map(|tokens| tokens as usize)
}

#[cfg(test)]
mod tests {
    use devo_protocol::{ModelProfileKey, SamplingControls};
    use pretty_assertions::assert_eq;

    use super::*;

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
    /// Verifies: serialized tool responses are objects without losing result values or names.
    #[test]
    fn google_tool_result_response_requires_json_object_contract() {
        let cases = [
            ("42", json!({ "result": 42 })),
            ("-1.5", json!({ "result": -1.5 })),
            ("true", json!({ "result": true })),
            ("false", json!({ "result": false })),
            ("null", json!({ "result": null })),
            ("[1,2]", json!({ "result": [1, 2] })),
            ("[]", json!({ "result": [] })),
            (r#""hello""#, json!({ "result": "hello" })),
            (r#""""#, json!({ "result": "" })),
            ("plain text", json!({ "result": "plain text" })),
            ("", json!({ "result": "" })),
            ("{invalid", json!({ "result": "{invalid" })),
            (r#"{"ok":true}"#, json!({ "ok": true })),
            ("{}", json!({})),
            (
                r#" { "result": [1, null], "error": "failed" } "#,
                json!({ "result": [1, null], "error": "failed" }),
            ),
        ];
        for (output, expected) in cases {
            let request = ModelRequest {
                model_slug: ModelProfileKey::Generic,
                model: "gemini-2.5-flash".into(),
                system: None,
                messages: vec![
                    devo_protocol::RequestMessage {
                        role: "assistant".into(),
                        content: vec![RequestContent::ToolUse {
                            id: "call-1".into(),
                            name: "read_value".into(),
                            input: json!({}),
                        }],
                    },
                    devo_protocol::RequestMessage {
                        role: "user".into(),
                        content: vec![RequestContent::ToolResult {
                            tool_use_id: "call-1".into(),
                            content: output.into(),
                            is_error: None,
                        }],
                    },
                ],
                max_tokens: 64,
                tools: None,
                hosted_tools: Vec::new(),
                sampling: SamplingControls::default(),
                request_thinking: None,
                reasoning_effort: None,
                extra_body: None,
            };
            let provider = GoogleGenerativeAiProvider::new(
                DEFAULT_GOOGLE_GENERATIVE_AI_BASE_URL,
                "fixture-key",
            );
            let built = provider
                .request_builder(&request, &build_request(&request))
                .build()
                .expect("build Google request");
            let body: Value = serde_json::from_slice(
                built
                    .body()
                    .expect("request body")
                    .as_bytes()
                    .expect("JSON body"),
            )
            .expect("parse serialized request");
            assert_eq!(
                body["contents"][1]["parts"][0],
                json!({ "functionResponse": {
                    "id": "call-1", "name": "read_value", "response": expected
                }}),
                "tool output: {output}"
            );
        }
    }
    #[test]
    fn google_thought_summary_is_not_final_answer_text() {
        let response = parse_response(json!({
            "candidates": [{"content": {"parts": [
                {"thought": true, "text": "Reasoning summary"},
                {"text": "Final answer"}
            ]}, "finishReason": "STOP"}]
        }))
        .expect("valid Google response");
        let visible: Vec<_> = response
            .content
            .iter()
            .filter_map(|part| match part {
                ResponseContent::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(visible, vec!["Final answer"]);
        assert_eq!(
            response.metadata.extras,
            vec![ResponseExtra::ReasoningText {
                text: "Reasoning summary".into()
            }]
        );
    }

    #[test]
    fn google_thought_parts_preserve_boundaries_and_tools() {
        let response = parse_response(json!({
            "candidates": [{"content": {"parts": [
                {"thought": true, "text": ""},
                {"thought": true, "text": "First"},
                {"thought": false, "text": "Answer"},
                {"thought": true, "text": "Second"},
                {"text": "More"},
                {"functionCall": {"id": "call", "name": "read", "args": {}}},
                {"thought": true}
            ]}, "finishReason": "STOP"}]
        }))
        .expect("valid response");
        assert_eq!(
            response.content,
            vec![
                ResponseContent::Text("Answer".into()),
                ResponseContent::Text("More".into()),
                ResponseContent::ToolUse {
                    id: "call".into(),
                    name: "read".into(),
                    input: json!({})
                },
            ]
        );
        assert_eq!(
            response.metadata.extras,
            vec![
                ResponseExtra::ReasoningText {
                    text: "First".into()
                },
                ResponseExtra::ReasoningText {
                    text: "Second".into()
                },
            ]
        );
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
        let only_thought = parse_response(json!({
            "candidates": [{"content": {"parts": [{"thought": true, "text": "Summary"}]},
                "finishReason": "MAX_TOKENS"}]
        }))
        .expect("thought-only response");
        assert_eq!(only_thought.content, vec![]);
        assert_eq!(
            only_thought.metadata.extras,
            vec![ResponseExtra::ReasoningText {
                text: "Summary".into()
            },]
        );
        assert_eq!(only_thought.stop_reason, Some(StopReason::MaxTokens));
    }
    /// Verifies: real HTTP completion and synthetic streaming preserve summaries only as metadata.
    #[tokio::test]
    async fn google_thought_metadata_survives_completion_and_stream() {
        use futures::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock");
        let address = listener.local_addr().expect("mock address");
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.expect("accept request");
                let mut request = [0; 4096];
                let bytes_read = socket.read(&mut request).await.expect("read request");
                assert!(bytes_read > 0, "request must not be empty");
                let body = r#"{"candidates":[{"content":{"parts":[{"thought":true,"text":"Summary"},{"text":"Answer"}]},"finishReason":"STOP"}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket
                    .write_all(response.as_bytes())
                    .await
                    .expect("write response");
            }
        });
        let provider = GoogleGenerativeAiProvider::new(format!("http://{address}"), "fixture");
        let request = ModelRequest {
            model_slug: ModelProfileKey::Generic,
            model: "gemini-2.5-flash".into(),
            system: None,
            messages: vec![],
            max_tokens: 64,
            tools: None,
            hosted_tools: vec![],
            sampling: SamplingControls::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: Some(
                json!({"generationConfig": {"thinkingConfig": {"includeThoughts": true}}}),
            ),
        };
        assert_eq!(
            build_request(&request)["generationConfig"]["thinkingConfig"]["includeThoughts"],
            json!(true)
        );
        let response = provider
            .completion(request.clone())
            .await
            .expect("completion");
        assert_eq!(
            response.content,
            vec![ResponseContent::Text("Answer".into())]
        );
        assert_eq!(
            response.metadata.extras,
            vec![ResponseExtra::ReasoningText {
                text: "Summary".into()
            }]
        );
        let events = provider
            .completion_stream(request)
            .await
            .expect("stream")
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()
            .expect("stream events");
        assert_eq!(
            events,
            vec![
                StreamEvent::TextStart { index: 0 },
                StreamEvent::TextDelta {
                    index: 0,
                    text: "Answer".into()
                },
                StreamEvent::MessageDone { response },
            ]
        );
        server.await.expect("mock server");
    }

    /// Verifies: subsequent Google requests do not relabel persisted reasoning as answer text.
    #[test]
    fn google_history_keeps_reasoning_out_of_answer_parts() {
        let request = ModelRequest {
            model_slug: ModelProfileKey::Generic,
            model: "gemini-2.5-flash".into(),
            system: None,
            messages: vec![devo_protocol::RequestMessage {
                role: "assistant".into(),
                content: vec![
                    RequestContent::Reasoning {
                        text: "Summary".into(),
                    },
                    RequestContent::Text {
                        text: "Answer".into(),
                    },
                ],
            }],
            max_tokens: 64,
            tools: None,
            hosted_tools: vec![],
            sampling: SamplingControls::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        };
        assert_eq!(
            build_request(&request)["contents"],
            json!([
                {"role": "model", "parts": [{"text": "Answer"}]}
            ])
        );
    }
}
