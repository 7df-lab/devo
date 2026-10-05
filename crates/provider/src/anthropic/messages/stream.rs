use std::collections::{BTreeMap, HashMap, HashSet};
use std::pin::Pin;

use anyhow::{Context, Result};
use devo_protocol::{
    ModelRequest, ModelResponse, ResponseContent, ResponseExtra, ResponseMetadata, StopReason,
    StreamEvent,
};
use futures::{Stream, StreamExt};
use reqwest_eventsource::{Event, EventSource};
use serde_json::{Map, Value, json};
use tracing::debug;

use super::{
    AnthropicProvider, AnthropicResponseContentBlock, anthropic_thinking_payload_map,
    append_json_string_field, build_request, hosted_result_tool_name,
    insert_provider_reasoning_blocks, parse_stop_reason,
};
use crate::anthropic::stream_usage::AnthropicStreamUsage;
use crate::dsml::DsmlToolCallHealer;
use crate::error::{format_eventsource_error, stream_error};
use crate::http::invalid_status_error;

pub(super) async fn completion_stream(
    provider: &AnthropicProvider,
    request: ModelRequest,
) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
    let body = build_request(&request, true);
    debug!(
        provider = "anthropic",
        api_base = %provider.base_url,
        model = %request.model,
        messages = request.messages.len(),
        tools = request.tools.as_ref().map_or(0, Vec::len),
        max_tokens = request.max_tokens,
        "sending anthropic streaming request"
    );

    let dsml_healer = DsmlToolCallHealer::for_request(&request);
    let event_source = EventSource::new(
        provider
            .streaming_request_builder(&body, &crate::request_headers(request.extra_body.as_ref())),
    )
    .context("failed to create anthropic event source")?;
    let stream = async_stream::try_stream! {
        let mut message_id = String::new();
        let mut stream_usage = AnthropicStreamUsage::default();
        let mut stop_reason: Option<StopReason> = None;
        let mut content_blocks: BTreeMap<usize, ResponseContent> = BTreeMap::new();
        let mut reasoning_blocks: BTreeMap<usize, String> = BTreeMap::new();
        let mut provider_reasoning_blocks: BTreeMap<usize, Map<String, Value>> = BTreeMap::new();
        let mut completed_reasoning_blocks: HashSet<usize> = HashSet::new();
        let mut tool_json: HashMap<usize, String> = HashMap::new();
        let mut hosted_tool_inputs: HashMap<String, Value> = HashMap::new();
        let mut dsml_text_filters = BTreeMap::new();

        futures::pin_mut!(event_source);
        loop {
            let event = match event_source.next().await {
                Some(event) => event,
                None => break,
            };
            let event = match event {
                Ok(event) => event,
                Err(reqwest_eventsource::Error::InvalidStatusCode(status, response)) => {
                    Err(invalid_status_error(
                        "anthropic",
                        &request.model,
                        "stream",
                        status,
                        response,
                        &body,
                    )
                    .await)?
                }
                Err(error) => Err(stream_error(
                    format!(
                        "anthropic stream error for model {}: {}",
                        request.model,
                        format_eventsource_error(&error)
                    )
                ))?,
            };

            match event {
                Event::Open => {}
                Event::Message(message) => {
                    let data: Value = serde_json::from_str(&message.data)
                        .map_err(|error| anyhow::anyhow!("failed to parse anthropic stream payload: {error}"))?;

                    match message.event.as_str() {
                        "message_start" => {
                            if let Some(id) = data
                                .get("message")
                                .and_then(Value::as_object)
                                .and_then(|message| message.get("id"))
                                .and_then(Value::as_str)
                            {
                                message_id = id.to_string();
                            }
                            if let Some(usage) = stream_usage.update_from_message_start(&data) {
                                yield StreamEvent::UsageDelta(usage);
                            }
                        }
                        "content_block_start" => {
                            let Some(index) = data.get("index").and_then(Value::as_u64) else {
                                continue;
                            };
                            let Some(content_block) = data.get("content_block") else {
                                continue;
                            };
                            let block: AnthropicResponseContentBlock =
                                serde_json::from_value(content_block.clone()).map_err(|error| {
                                    anyhow::anyhow!(
                                        "failed to parse anthropic content block start: {error}"
                                    )
                                })?;
                            match block.kind.as_str() {
                                "text" => {
                                    if let Some(filter) = dsml_healer.text_stream_filter() {
                                        dsml_text_filters.insert(index as usize, filter);
                                    }
                                    content_blocks.insert(
                                        index as usize,
                                        ResponseContent::Text(String::new()),
                                    );
                                    yield StreamEvent::TextStart {
                                        index: index as usize,
                                    };
                                }
                                "tool_use" => {
                                    let Some(id) = block.id.clone() else {
                                        continue;
                                    };
                                    let Some(name) = block.name.clone() else {
                                        continue;
                                    };
                                    let input = block
                                        .input
                                        .clone()
                                        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                                    content_blocks.insert(
                                        index as usize,
                                        ResponseContent::ToolUse {
                                            id: id.clone(),
                                            name: name.clone(),
                                            input: input.clone(),
                                        },
                                    );
                                    tool_json.insert(index as usize, String::new());
                                    yield StreamEvent::ToolCallStart {
                                        index: index as usize,
                                        id,
                                        name,
                                        input,
                                    };
                                }
                                "server_tool_use" => {
                                    let Some(id) = block.id.clone() else {
                                        continue;
                                    };
                                    let name = block
                                        .name
                                        .clone()
                                        .unwrap_or_else(|| "web_search".to_string());
                                    let input = block
                                        .input
                                        .clone()
                                        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                                    content_blocks.insert(
                                        index as usize,
                                        ResponseContent::HostedToolUse {
                                            id: id.clone(),
                                            name: name.clone(),
                                            input: input.clone(),
                                            output: None,
                                            status: None,
                                        },
                                    );
                                    tool_json.insert(index as usize, String::new());
                                }
                                "web_search_tool_result" | "web_fetch_tool_result" => {
                                    let id = block
                                        .tool_use_id
                                        .clone()
                                        .or_else(|| block.id.clone())
                                        .unwrap_or_default();
                                    let name = hosted_result_tool_name(&block.kind);
                                    let input = hosted_tool_inputs
                                        .get(&id)
                                        .cloned()
                                        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                                    let output = block.content.clone();
                                    let status = Some(
                                        block
                                            .status
                                            .clone()
                                            .unwrap_or_else(|| "completed".to_string()),
                                    );
                                    content_blocks.insert(
                                        index as usize,
                                        ResponseContent::HostedToolUse {
                                            id: id.clone(),
                                            name: name.clone(),
                                            input: input.clone(),
                                            output: output.clone(),
                                            status: status.clone(),
                                        },
                                    );
                                    yield StreamEvent::HostedToolCallDone {
                                        index: index as usize,
                                        id,
                                        name,
                                        input,
                                        output,
                                        status,
                                    };
                                }
                                "thinking" => {
                                    reasoning_blocks.insert(index as usize, String::new());
                                    provider_reasoning_blocks.insert(
                                        index as usize,
                                        anthropic_thinking_payload_map(&block),
                                    );
                                    yield StreamEvent::ReasoningStart {
                                        index: index as usize,
                                    };
                                }
                                _ => {}
                            };
                        }
                        "content_block_delta" => {
                            let Some(index) = data.get("index").and_then(Value::as_u64) else {
                                continue;
                            };
                            let Some(delta) = data.get("delta").and_then(Value::as_object)
                            else {
                                continue;
                            };
                            match delta.get("type").and_then(Value::as_str) {
                                Some("text_delta") => {
                                    let text = delta
                                        .get("text")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default();
                                    if let Some(ResponseContent::Text(value)) =
                                        content_blocks.get_mut(&(index as usize))
                                    {
                                        value.push_str(text);
                                    }
                                    let index = index as usize;
                                    let text_chunks = if let Some(filter) =
                                        dsml_text_filters.get_mut(&index)
                                    {
                                        filter.consume(text)
                                    } else {
                                        vec![text.to_string()]
                                    };
                                    for text in text_chunks {
                                        yield StreamEvent::TextDelta {
                                            index,
                                            text,
                                        };
                                    }
                                }
                                Some("thinking_delta") => {
                                    let text = delta
                                        .get("thinking")
                                        .or_else(|| delta.get("text"))
                                        .and_then(Value::as_str)
                                        .unwrap_or_default();
                                    if let Some(value) = reasoning_blocks.get_mut(&(index as usize))
                                    {
                                        value.push_str(text);
                                    }
                                    append_json_string_field(
                                        provider_reasoning_blocks
                                            .entry(index as usize)
                                            .or_insert_with(|| {
                                                let mut payload = Map::new();
                                                payload
                                                    .insert("type".to_string(), json!("thinking"));
                                                payload
                                            }),
                                        "thinking",
                                        text,
                                    );
                                    yield StreamEvent::ReasoningDelta {
                                        index: index as usize,
                                        text: text.to_string(),
                                    };
                                }
                                Some("signature_delta") => {
                                    let signature = delta
                                        .get("signature")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default();
                                    append_json_string_field(
                                        provider_reasoning_blocks
                                            .entry(index as usize)
                                            .or_insert_with(|| {
                                                let mut payload = Map::new();
                                                payload
                                                    .insert("type".to_string(), json!("thinking"));
                                                payload
                                            }),
                                        "signature",
                                        signature,
                                    );
                                }
                                Some("input_json_delta") => {
                                    let partial_json = delta
                                        .get("partial_json")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default();
                                    if let Some(acc) = tool_json.get_mut(&(index as usize)) {
                                        acc.push_str(partial_json);
                                    }
                                    yield StreamEvent::ToolCallInputDelta {
                                        index: index as usize,
                                        partial_json: partial_json.to_string(),
                                    };
                                }
                                _ => {}
                            }
                        }
                        "content_block_stop" => {
                            let index = data.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                            if let Some(mut filter) = dsml_text_filters.remove(&index) {
                                for text in filter.finish() {
                                    yield StreamEvent::TextDelta {
                                        index,
                                        text,
                                    };
                                }
                            }
                            if let Some(json_str) = tool_json.remove(&index)
                                && !json_str.is_empty()
                                && let Ok(parsed) = serde_json::from_str(&json_str)
                                && let Some(block) = content_blocks.get_mut(&index)
                            {
                                match block {
                                    ResponseContent::ToolUse { input, .. }
                                    | ResponseContent::HostedToolUse { input, .. } => {
                                        *input = parsed;
                                    }
                                    ResponseContent::Text(_)
                                    | ResponseContent::ProviderReasoning { .. } => {}
                                }
                            }
                            if let Some(ResponseContent::HostedToolUse {
                                id,
                                name,
                                input,
                                output,
                                status,
                            }) = content_blocks.get(&index)
                                && output.is_none()
                                && status.is_none()
                            {
                                hosted_tool_inputs.insert(id.clone(), input.clone());
                                yield StreamEvent::HostedToolCallStart {
                                    index,
                                    id: id.clone(),
                                    name: name.clone(),
                                    input: input.clone(),
                                };
                            }
                            if reasoning_blocks.contains_key(&index)
                                && completed_reasoning_blocks.insert(index)
                            {
                                yield StreamEvent::ReasoningDone { index };
                            }
                        }
                        "message_delta" => {
                            if let Some(delta) = data.get("delta").and_then(Value::as_object)
                                && let Some(reason) =
                                    delta.get("stop_reason").and_then(Value::as_str)
                                {
                                    stop_reason = Some(parse_stop_reason(reason));
                                }
                            if let Some(usage) = stream_usage.update_from_message_delta(&data) {
                                yield StreamEvent::UsageDelta(usage);
                            }
                        }
                        "message_stop" => {
                            for (index, mut filter) in std::mem::take(&mut dsml_text_filters) {
                                for text in filter.finish() {
                                    yield StreamEvent::TextDelta {
                                        index,
                                        text,
                                    };
                                }
                            }
                            for index in reasoning_blocks.keys().copied().collect::<Vec<_>>() {
                                if completed_reasoning_blocks.insert(index) {
                                    yield StreamEvent::ReasoningDone { index };
                                }
                            }
                            insert_provider_reasoning_blocks(
                                &mut content_blocks,
                                std::mem::take(&mut provider_reasoning_blocks),
                            );
                            let response = ModelResponse {
                                id: message_id.clone(),
                                content: dsml_healer
                                    .heal_response_content(content_blocks.into_values().collect()),
                                stop_reason: stop_reason.clone(),
                                usage: stream_usage.snapshot(),
                                metadata: ResponseMetadata {
                                    extras: reasoning_blocks
                                        .values()
                                        .filter(|text| !text.is_empty())
                                        .cloned()
                                        .map(|text| ResponseExtra::ReasoningText { text })
                                        .collect(),
                                },
                            };
                            yield StreamEvent::MessageDone { response };
                            return;
                        }
                        _ => {}
                    }
                }
            }
        }

        for (index, mut filter) in std::mem::take(&mut dsml_text_filters) {
            for text in filter.finish() {
                yield StreamEvent::TextDelta {
                    index,
                    text,
                };
            }
        }
        for index in reasoning_blocks.keys().copied().collect::<Vec<_>>() {
            if completed_reasoning_blocks.insert(index) {
                yield StreamEvent::ReasoningDone { index };
            }
        }
        insert_provider_reasoning_blocks(
            &mut content_blocks,
            provider_reasoning_blocks,
        );
        let response = ModelResponse {
            id: message_id,
            content: dsml_healer.heal_response_content(content_blocks.into_values().collect()),
            stop_reason,
            usage: stream_usage.snapshot(),
            metadata: ResponseMetadata {
                extras: reasoning_blocks
                    .into_values()
                    .filter(|text| !text.is_empty())
                    .map(|text| ResponseExtra::ReasoningText { text })
                    .collect(),
            },
        };
        yield StreamEvent::MessageDone { response };
    };

    Ok(Box::pin(stream))
}
