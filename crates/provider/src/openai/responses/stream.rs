//! Streaming Responses decoding. Terminal failures must never fall through to success.

use anyhow::Result;
use devo_protocol::{
    ModelRequest, ModelResponse, ResponseContent, ResponseExtra, ResponseMetadata, StopReason,
    StreamEvent, Usage,
};
use futures::{Stream, StreamExt};
use serde_json::Value;
use std::{collections::HashMap, pin::Pin};

use super::response::{
    function_call_arguments_json, parse_output_item, parse_response, parse_usage,
};
use super::stream_tools::{
    ResponsesStreamHostedToolCall, ResponsesStreamToolCall, responses_stream_tool_content,
};
use crate::error::stream_error;
use crate::text_normalization::{TaggedTextFragment, TaggedTextParser};

pub(super) fn decode_response_stream(
    response: reqwest::Response,
    request: ModelRequest,
) -> Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> {
    let messages = crate::sse::decode_sse(response.bytes_stream());
    let stream = async_stream::try_stream! {
        let mut text_buf = String::new();
        let mut reasoning_buf = String::new();
        let mut text_parser = TaggedTextParser::default();
        let mut response_id = String::new();
        let mut tool_calls: HashMap<String, ResponsesStreamToolCall> = HashMap::new();
        let mut tool_call_keys_by_item_id: HashMap<String, String> = HashMap::new();
        let mut hosted_tool_calls: HashMap<String, ResponsesStreamHostedToolCall> = HashMap::new();
        let mut usage: Option<Usage> = None;
        let mut reasoning_started = false;
        // Item id of the reasoning block currently being streamed. The
        // boundary between summary parts must only fire *within* one
        // reasoning item: backends that run hosted tools mid-stream emit
        // further reasoning items afterwards, and a cross-item boundary
        // would surface as an orphan leading "\n\n" on the next durable
        // reasoning item (the previous one was already closed when the
        // tool call started).
        let mut reasoning_item_id: Option<String> = None;
        // True while the most recently emitted stream event is reasoning
        // text. A summary-part boundary must glue two parts of the same
        // paragraph together, but must never lead a fresh reasoning block
        // that follows a tool call or assistant text — the consumer closes
        // the reasoning item there, so a leading "\n\n" would orphan-start
        // the next block.
        let mut reasoning_active = false;
        let mut text_started = false;

        futures::pin_mut!(messages);
        loop {
            let message = match messages.next().await {
                Some(Ok(message)) => message,
                Some(Err(error)) => Err(stream_error(format!(
                    "openai responses stream error for model {}: {error}",
                    request.model
                )))?,
                None => break,
            };

            {
                if message.data == "[DONE]" {
                    break;
                }

                let chunk: Value = serde_json::from_str(&message.data)
                    .map_err(|error| anyhow::anyhow!("failed to parse openai responses stream chunk: {error}"))?;

                // HTTP 200 only establishes the SSE transport. A terminal failure
                // in that stream must bypass the successful EOF fallback below.
                let event = match message.event.as_str() {
                    "" | "message" => chunk.get("type").and_then(Value::as_str).unwrap_or("message"),
                    event => event,
                };
                if let Some(error) = super::stream_error::provider_error_from_event(event, &chunk, &request) {
                    Err(error)?;
                }

                if response_id.is_empty() {
                    response_id = chunk
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                }

                if let Some(parsed_usage) = chunk.get("usage").and_then(parse_usage) {
                    usage = Some(parsed_usage.clone());
                    yield StreamEvent::UsageDelta(parsed_usage);
                }

                match event {
                        "response.output_text.delta" => {
                            let delta = chunk
                                .get("delta")
                                .and_then(Value::as_str)
                                .or_else(|| chunk.get("text").and_then(Value::as_str))
                                .unwrap_or_default();
                            if !delta.is_empty() {
                                for fragment in text_parser.consume(delta) {
                                    match fragment {
                                        TaggedTextFragment::Text(text) => {
                                            if text.is_empty() {
                                                continue;
                                            }
                                            if !text_started {
                                                text_started = true;
                                                yield StreamEvent::TextStart { index: 0 };
                                            }
                                            text_buf.push_str(&text);
                                            reasoning_active = false;
                                            yield StreamEvent::TextDelta { index: 0, text };
                                        }
                                        TaggedTextFragment::Reasoning(text) => {
                                            if text.is_empty() {
                                                continue;
                                            }
                                            if !reasoning_started {
                                                reasoning_started = true;
                                                yield StreamEvent::ReasoningStart { index: 1 };
                                            }
                                            reasoning_buf.push_str(&text);
                                            reasoning_active = true;
                                            yield StreamEvent::ReasoningDelta { index: 1, text };
                                        }
                                    }
                                }
                            }
                        }
                        "response.reasoning_summary_part.added" => {
                            // OpenAI Responses streams each thinking summary
                            // as its own part. Without an explicit boundary
                            // consecutive parts fuse into one run-on
                            // paragraph ("…words****Inserting…" in the
                            // journal), so re-emit the boundary as a delta:
                            // every consumer — live TUI stream and the
                            // durable reasoning item — then renders the
                            // parts as separate paragraphs. The boundary
                            // only applies within the same reasoning item:
                            // a part with a new item id starts a fresh
                            // block, not a continuation of the previous one.
                            let part_item_id =
                                chunk.get("item_id").and_then(Value::as_str).map(str::to_string);
                            let same_item = match (&part_item_id, &reasoning_item_id) {
                                (Some(incoming), Some(tracked)) => incoming == tracked,
                                (None, None) => true,
                                _ => false,
                            };
                            if reasoning_active && same_item {
                                reasoning_buf.push_str("\n\n");
                                yield StreamEvent::ReasoningDelta {
                                    index: 1,
                                    text: "\n\n".to_string(),
                                };
                            }
                            reasoning_item_id = part_item_id;
                        }
                        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                            // Codex-style backends stream thinking as
                            // reasoning summary deltas; without this arm the
                            // adapter silently drops them and the UI shows
                            // no thinking content.
                            let delta = chunk
                                .get("delta")
                                .and_then(Value::as_str)
                                .or_else(|| chunk.get("text").and_then(Value::as_str))
                                .unwrap_or_default();
                            if !delta.is_empty() {
                                if !reasoning_started {
                                    reasoning_started = true;
                                    yield StreamEvent::ReasoningStart { index: 1 };
                                }
                                reasoning_buf.push_str(delta);
                                reasoning_active = true;
                                yield StreamEvent::ReasoningDelta {
                                    index: 1,
                                    text: delta.to_string(),
                                };
                            }
                        }
                        "response.output_item.added" => {
                            if let Some(item) = chunk.get("item") {
                                if let Some(reasoning_content) =
                                    item.get("reasoning_content").and_then(Value::as_str)
                                    && !reasoning_content.is_empty() {
                                        if !reasoning_started {
                                            reasoning_started = true;
                                            yield StreamEvent::ReasoningStart { index: 1 };
                                        }
                                        reasoning_buf.push_str(reasoning_content);
                                        reasoning_active = true;
                                        yield StreamEvent::ReasoningDelta {
                                            index: 1,
                                            text: reasoning_content.to_string(),
                                        };
                                    }
                                if let Some(content) = parse_output_item(item).into_iter().next() {
                                    match content {
                                        ResponseContent::ToolUse { id, name, input } => {
                                            let key = id.clone();
                                            let item_id = item
                                                .get("id")
                                                .and_then(Value::as_str)
                                                .unwrap_or_default();
                                            let index = tool_calls.len() + hosted_tool_calls.len() + 1;
                                            let arguments_json = function_call_arguments_json(item);
                                            tool_calls.insert(
                                                key.clone(),
                                                ResponsesStreamToolCall {
                                                    index,
                                                    id: id.clone(),
                                                    name: name.clone(),
                                                    arguments_json,
                                                },
                                            );
                                            if !item_id.is_empty() {
                                                tool_call_keys_by_item_id
                                                    .insert(item_id.to_string(), key);
                                            }
                                            yield StreamEvent::ToolCallStart {
                                                index,
                                                id,
                                                name,
                                                input,
                                            };
                                            reasoning_active = false;
                                        }
                                        ResponseContent::HostedToolUse { id, name, input, output, status } => {
                                            let index = tool_calls.len() + hosted_tool_calls.len() + 1;
                                            let key = id.clone();
                                            hosted_tool_calls.insert(
                                                key,
                                                (index, id.clone(), name.clone(), input.clone(), output.clone(), status.clone()),
                                            );
                                            yield StreamEvent::HostedToolCallStart {
                                                index,
                                                id: id.clone(),
                                                name: name.clone(),
                                                input: input.clone(),
                                            };
                                            reasoning_active = false;
                                            if output.is_some() {
                                                yield StreamEvent::HostedToolCallDone {
                                                    index,
                                                    id,
                                                    name,
                                                    input,
                                                    output,
                                                    status,
                                                };
                                            }
                                        }
                                        ResponseContent::Text(_)
                                        | ResponseContent::ProviderReasoning { .. } => {}
                                    }
                                }
                            }
                        }
                        "response.output_item.done" => {
                            if let Some(item) = chunk.get("item")
                                && let Some(ResponseContent::HostedToolUse { id, name, input, output, status }) =
                                    parse_output_item(item).into_iter().next()
                            {
                                let key = id.clone();
                                let index = if let Some((
                                    index,
                                    stored_id,
                                    stored_name,
                                    stored_input,
                                    stored_output,
                                    stored_status,
                                )) = hosted_tool_calls.get_mut(&key) {
                                    *stored_id = id.clone();
                                    *stored_name = name.clone();
                                    *stored_input = input.clone();
                                    if output.is_some() {
                                        *stored_output = output.clone();
                                    }
                                    if status.is_some() {
                                        *stored_status = status.clone();
                                    }
                                    *index
                                } else {
                                    let index = tool_calls.len() + hosted_tool_calls.len() + 1;
                                    hosted_tool_calls.insert(
                                        key,
                                        (index, id.clone(), name.clone(), input.clone(), output.clone(), status.clone()),
                                    );
                                    index
                                };
                                yield StreamEvent::HostedToolCallDone {
                                    index,
                                    id,
                                    name,
                                    input,
                                    output,
                                    status,
                                };
                            }
                        }
                        "response.function_call_arguments.delta" | "response.output_item.delta" => {
                            let partial_json = chunk
                                .get("delta")
                                .or_else(|| chunk.get("arguments_delta"))
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            let call_id = chunk
                                .get("call_id")
                                .and_then(Value::as_str)
                                .map(ToOwned::to_owned)
                                .or_else(|| {
                                    chunk
                                        .get("item_id")
                                        .and_then(Value::as_str)
                                        .and_then(|item_id| {
                                            tool_call_keys_by_item_id.get(item_id).cloned()
                                        })
                                });
                            if !partial_json.is_empty()
                                && let Some(call_id) = call_id
                                && let Some(entry) = tool_calls.get_mut(&call_id)
                            {
                                entry.arguments_json.push_str(partial_json);
                                yield StreamEvent::ToolCallInputDelta {
                                    index: entry.index,
                                    partial_json: partial_json.to_string(),
                                };
                            }
                        }
                        "response.completed" | "response.done" => {
                            for fragment in text_parser.finish() {
                                match fragment {
                                    TaggedTextFragment::Text(text) => {
                                        if text.is_empty() {
                                            continue;
                                        }
                                        if !text_started {
                                            text_started = true;
                                            yield StreamEvent::TextStart { index: 0 };
                                        }
                                        text_buf.push_str(&text);
                                        yield StreamEvent::TextDelta { index: 0, text };
                                    }
                                    TaggedTextFragment::Reasoning(text) => {
                                        if text.is_empty() {
                                            continue;
                                        }
                                        if !reasoning_started {
                                            reasoning_started = true;
                                            yield StreamEvent::ReasoningStart { index: 1 };
                                        }
                                        reasoning_buf.push_str(&text);
                                        yield StreamEvent::ReasoningDelta { index: 1, text };
                                    }
                                }
                            }
                            if reasoning_started {
                                yield StreamEvent::ReasoningDone { index: 1 };
                            }
                            let response = if let Some(parsed) = chunk.get("response") {
                                let mut response = parse_response(parsed.clone())?;
                                if response.content.is_empty() {
                                    // Codex-style backends do not replay the
                                    // output items in the terminal
                                    // `response.completed` event, so the
                                    // streamed deltas are the only record of
                                    // the assistant content; a non-stream
                                    // `completion()` folded from this stream
                                    // would otherwise report no content.
                                    let mut content = Vec::new();
                                    if !text_buf.is_empty() {
                                        content.push(ResponseContent::Text(
                                            text_buf.clone(),
                                        ));
                                    }
                                    content.extend(responses_stream_tool_content(
                                        &tool_calls,
                                        &hosted_tool_calls,
                                    ));
                                    response.content = content;
                                }
                                if !reasoning_buf.is_empty()
                                    && !response.metadata.extras.iter().any(|extra| {
                                        matches!(
                                            extra,
                                            ResponseExtra::ReasoningText { text }
                                                if text == &reasoning_buf
                                        )
                                    })
                                {
                                    response.metadata.extras.push(ResponseExtra::ReasoningText {
                                        text: reasoning_buf.clone(),
                                    });
                                }
                                response
                            } else {
                                ModelResponse {
                                    id: response_id.clone(),
                                    content: {
                                        let mut content = Vec::new();
                                        if !text_buf.is_empty() {
                                            content.push(ResponseContent::Text(text_buf.clone()));
                                        }
                                        content.extend(responses_stream_tool_content(
                                            &tool_calls,
                                            &hosted_tool_calls,
                                        ));
                                        content
                                    },
                                    stop_reason: Some(StopReason::EndTurn),
                                    usage: usage.unwrap_or_default(),
                                    metadata: if reasoning_buf.is_empty() {
                                        ResponseMetadata::default()
                                    } else {
                                        ResponseMetadata {
                                            extras: vec![ResponseExtra::ReasoningText {
                                                text: reasoning_buf.clone(),
                                            }],
                                        }
                                    },
                                }
                            };
                            yield StreamEvent::MessageDone { response };
                            return;
                        }
                        _ => {}
                    }
            }
        }

        for fragment in text_parser.finish() {
            match fragment {
                TaggedTextFragment::Text(text) => {
                    if text.is_empty() {
                        continue;
                    }
                    if !text_started {
                        text_started = true;
                        yield StreamEvent::TextStart { index: 0 };
                    }
                    text_buf.push_str(&text);
                    yield StreamEvent::TextDelta { index: 0, text };
                }
                TaggedTextFragment::Reasoning(text) => {
                    if text.is_empty() {
                        continue;
                    }
                    if !reasoning_started {
                        reasoning_started = true;
                        yield StreamEvent::ReasoningStart { index: 1 };
                    }
                    reasoning_buf.push_str(&text);
                    yield StreamEvent::ReasoningDelta { index: 1, text };
                }
            }
        }

        if reasoning_started {
            yield StreamEvent::ReasoningDone { index: 1 };
        }

        let response = ModelResponse {
            id: response_id,
            content: {
                let mut content = Vec::new();
                if !text_buf.is_empty() {
                    content.push(ResponseContent::Text(text_buf));
                }
                content.extend(responses_stream_tool_content(
                    &tool_calls,
                    &hosted_tool_calls,
                ));
                content
            },
            stop_reason: Some(StopReason::EndTurn),
            usage: usage.unwrap_or_default(),
            metadata: if reasoning_buf.is_empty() {
                ResponseMetadata::default()
            } else {
                ResponseMetadata {
                    extras: vec![ResponseExtra::ReasoningText {
                        text: reasoning_buf,
                    }],
                }
            },
        };
        yield StreamEvent::MessageDone { response };
    };

    Box::pin(stream)
}
