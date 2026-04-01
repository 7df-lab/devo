use std::collections::HashMap;
use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use tracing::{debug, warn};

use crate::{
    ModelProvider, ModelRequest, ModelResponse, RequestContent, ResponseContent, StopReason,
    StreamEvent, Usage,
};

/// OpenAI-compatible provider that works with Ollama, vLLM, LM Studio, etc.
pub struct OpenAICompatProvider {
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl OpenAICompatProvider {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: None,
            client: reqwest::Client::new(),
        }
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Convert our internal ModelRequest to OpenAI chat completions format.
    fn build_body(&self, request: &ModelRequest, stream: bool) -> serde_json::Value {
        let mut messages = Vec::new();

        if let Some(ref system) = request.system {
            messages.push(serde_json::json!({
                "role": "system",
                "content": system
            }));
        }

        for msg in &request.messages {
            messages.extend(convert_request_message(msg));
        }

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": messages,
            "max_tokens": request.max_tokens,
            "stream": stream,
        });

        if let Some(ref tools) = request.tools {
            if !tools.is_empty() {
                let openai_tools: Vec<serde_json::Value> = tools
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "type": "function",
                            "function": {
                                "name": t.name,
                                "description": t.description,
                                "parameters": t.input_schema
                            }
                        })
                    })
                    .collect();
                body["tools"] = serde_json::json!(openai_tools);
            }
        }

        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        body
    }

    fn build_request(&self, body: &serde_json::Value) -> reqwest::RequestBuilder {
        let mut req = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("content-type", "application/json");

        if let Some(ref key) = self.api_key {
            req = req.header("authorization", format!("Bearer {}", key));
        }

        req.json(body)
    }
}

/// Convert an internal RequestMessage into one or more OpenAI messages.
///
/// Anthropic puts tool_result blocks in "user" messages.
/// OpenAI expects separate "tool" role messages for each result,
/// and "assistant" messages with tool_calls for tool_use blocks.
fn convert_request_message(msg: &crate::RequestMessage) -> Vec<serde_json::Value> {
    let role = &msg.role;
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut tool_results = Vec::new();

    for block in &msg.content {
        match block {
            RequestContent::Text { text } => {
                text_parts.push(text.clone());
            }
            RequestContent::ToolUse { id, name, input } => {
                tool_calls.push(serde_json::json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": serde_json::to_string(input).unwrap_or_default()
                    }
                }));
            }
            RequestContent::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                tool_results.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": content
                }));
            }
        }
    }

    let mut out = Vec::new();

    if !tool_calls.is_empty() {
        let mut msg = serde_json::json!({ "role": role });
        let combined = text_parts.join("");
        if !combined.is_empty() {
            msg["content"] = serde_json::json!(combined);
        }
        msg["tool_calls"] = serde_json::json!(tool_calls);
        out.push(msg);
    } else if !text_parts.is_empty() {
        out.push(serde_json::json!({
            "role": role,
            "content": text_parts.join("")
        }));
    }

    out.extend(tool_results);
    out
}

#[async_trait]
impl ModelProvider for OpenAICompatProvider {
    async fn complete(&self, request: ModelRequest) -> anyhow::Result<ModelResponse> {
        let body = self.build_body(&request, false);
        debug!(model = %request.model, "openai-compat complete request");

        let resp = self.build_request(&body).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI-compat API error {}: {}", status, text);
        }

        let raw: serde_json::Value = resp.json().await?;
        parse_complete_response(&raw)
    }

    async fn stream(
        &self,
        request: ModelRequest,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>> {
        let body = self.build_body(&request, true);
        debug!(model = %request.model, "openai-compat stream request");

        let resp = self.build_request(&body).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI-compat API error {}: {}", status, text);
        }

        let (tx, rx) = tokio::sync::mpsc::channel::<anyhow::Result<StreamEvent>>(64);
        let byte_stream = resp.bytes_stream();

        tokio::spawn(async move {
            if let Err(e) = process_sse_stream(byte_stream, &tx).await {
                let _ = tx.send(Err(e)).await;
            }
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "openai-compat"
    }
}

// ---------------------------------------------------------------------------
// SSE stream processing (OpenAI format)
// ---------------------------------------------------------------------------

struct StreamState {
    message_id: String,
    text: String,
    // index → (id, name, arguments_accum)
    tool_calls: HashMap<usize, (String, String, String)>,
    finish_reason: Option<String>,
    content_block_started: bool,
    tool_blocks_started: HashMap<usize, bool>,
}

impl StreamState {
    fn new() -> Self {
        Self {
            message_id: String::new(),
            text: String::new(),
            tool_calls: HashMap::new(),
            finish_reason: None,
            content_block_started: false,
            tool_blocks_started: HashMap::new(),
        }
    }
}

async fn process_sse_stream(
    mut byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
    tx: &tokio::sync::mpsc::Sender<anyhow::Result<StreamEvent>>,
) -> anyhow::Result<()> {
    let mut buffer = String::new();
    let mut state = StreamState::new();

    while let Some(chunk) = byte_stream.next().await {
        let bytes = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));

        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim_end_matches('\r').to_string();
            buffer = buffer[pos + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            if let Some(data) = line.strip_prefix("data: ") {
                if data.trim() == "[DONE]" {
                    emit_message_done(&state, tx).await;
                    return Ok(());
                }

                let json: serde_json::Value = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("failed to parse SSE chunk: {}", e);
                        continue;
                    }
                };

                if let Some(id) = json["id"].as_str() {
                    state.message_id = id.to_string();
                }

                let events = handle_chunk(&json, &mut state);
                for evt in events {
                    if tx.send(evt).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }

    // Stream ended without [DONE] — emit what we have
    emit_message_done(&state, tx).await;
    Ok(())
}

fn handle_chunk(
    json: &serde_json::Value,
    state: &mut StreamState,
) -> Vec<anyhow::Result<StreamEvent>> {
    let mut out = Vec::new();

    let Some(choices) = json["choices"].as_array() else {
        return out;
    };

    for choice in choices {
        let delta = &choice["delta"];

        // Text content
        if let Some(content) = delta["content"].as_str() {
            if !content.is_empty() {
                if !state.content_block_started {
                    state.content_block_started = true;
                    out.push(Ok(StreamEvent::ContentBlockStart {
                        index: 0,
                        content: ResponseContent::Text(String::new()),
                    }));
                }
                state.text.push_str(content);
                out.push(Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: content.to_string(),
                }));
            }
        }

        // Tool calls
        if let Some(tool_calls) = delta["tool_calls"].as_array() {
            for tc in tool_calls {
                let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                let content_idx = idx + 1; // offset by 1 since index 0 is text

                let entry = state
                    .tool_calls
                    .entry(idx)
                    .or_insert_with(|| (String::new(), String::new(), String::new()));

                if let Some(id) = tc["id"].as_str() {
                    entry.0 = id.to_string();
                }
                if let Some(func) = tc.get("function") {
                    if let Some(name) = func["name"].as_str() {
                        entry.1 = name.to_string();
                    }
                    if let Some(args) = func["arguments"].as_str() {
                        entry.2.push_str(args);

                        if !state.tool_blocks_started.contains_key(&idx) {
                            state.tool_blocks_started.insert(idx, true);
                            out.push(Ok(StreamEvent::ContentBlockStart {
                                index: content_idx,
                                content: ResponseContent::ToolUse {
                                    id: entry.0.clone(),
                                    name: entry.1.clone(),
                                    input: serde_json::Value::Object(serde_json::Map::new()),
                                },
                            }));
                        }

                        if !args.is_empty() {
                            out.push(Ok(StreamEvent::InputJsonDelta {
                                index: content_idx,
                                partial_json: args.to_string(),
                            }));
                        }
                    }
                }
            }
        }

        if let Some(reason) = choice["finish_reason"].as_str() {
            state.finish_reason = Some(reason.to_string());
        }
    }

    out
}

async fn emit_message_done(
    state: &StreamState,
    tx: &tokio::sync::mpsc::Sender<anyhow::Result<StreamEvent>>,
) {
    let mut content = Vec::new();

    if !state.text.is_empty() {
        content.push(ResponseContent::Text(state.text.clone()));
    }

    let mut sorted_tools: Vec<_> = state.tool_calls.iter().collect();
    sorted_tools.sort_by_key(|(idx, _)| *idx);

    for (_, (id, name, args_json)) in sorted_tools {
        let input = serde_json::from_str(args_json)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        content.push(ResponseContent::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input,
        });
    }

    let stop_reason = state
        .finish_reason
        .as_deref()
        .map(parse_finish_reason);

    let response = ModelResponse {
        id: state.message_id.clone(),
        content,
        stop_reason,
        usage: Usage::default(),
    };

    let _ = tx.send(Ok(StreamEvent::MessageDone { response })).await;
}

fn parse_finish_reason(reason: &str) -> StopReason {
    match reason {
        "stop" => StopReason::EndTurn,
        "tool_calls" => StopReason::ToolUse,
        "length" => StopReason::MaxTokens,
        _ => StopReason::EndTurn,
    }
}

// ---------------------------------------------------------------------------
// Non-streaming response parsing
// ---------------------------------------------------------------------------

fn parse_complete_response(raw: &serde_json::Value) -> anyhow::Result<ModelResponse> {
    let id = raw["id"].as_str().unwrap_or("").to_string();

    let mut content = Vec::new();

    if let Some(choices) = raw["choices"].as_array() {
        if let Some(choice) = choices.first() {
            let message = &choice["message"];

            if let Some(text) = message["content"].as_str() {
                if !text.is_empty() {
                    content.push(ResponseContent::Text(text.to_string()));
                }
            }

            if let Some(tool_calls) = message["tool_calls"].as_array() {
                for tc in tool_calls {
                    let id = tc["id"].as_str().unwrap_or("").to_string();
                    let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                    let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                    let input = serde_json::from_str(args_str)
                        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                    content.push(ResponseContent::ToolUse { id, name, input });
                }
            }
        }
    }

    let stop_reason = raw["choices"]
        .as_array()
        .and_then(|c| c.first())
        .and_then(|c| c["finish_reason"].as_str())
        .map(parse_finish_reason);

    let usage = Usage {
        input_tokens: raw["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as usize,
        output_tokens: raw["usage"]["completion_tokens"].as_u64().unwrap_or(0) as usize,
        ..Default::default()
    };

    Ok(ModelResponse {
        id,
        content,
        stop_reason,
        usage,
    })
}
