use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Instant;

use anyhow::{Context, Result};
use devo_protocol::{ModelRequest, StopReason, StreamEvent};
use futures::{Stream, StreamExt};
use reqwest_eventsource::{Event, EventSource};
use serde::Deserialize;
use serde_json::Value;

use super::{OpenAIChoiceLogprobs, OpenAIProvider, build_request};
use crate::error::stream_error;
use crate::http::invalid_status_error;
use crate::openai::error_payload::provider_error_from_payload;
use crate::openai::shared::{OpenAICompletionUsage, deserialize_null_vec};

pub(super) async fn completion_stream(
    provider: &OpenAIProvider,
    request: ModelRequest,
) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
    let body = build_request(&request, true);
    tracing::debug!(
        provider = "openai",
        api_base = %provider.base_url,
        model = %request.model,
        messages = request.messages.len(),
        tools = request.tools.as_ref().map_or(0, Vec::len),
        max_tokens = request.max_tokens,
        http_body = %body,
        "sending openai streaming request"
    );

    let event_source = EventSource::new(
        provider
            .streaming_request_builder(&body, &crate::request_headers(request.extra_body.as_ref())),
    )
    .context("failed to create openai event source")?;
    let stream = async_stream::try_stream! {
        let mut state = ChatCompletionStreamState::for_request(&request);

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
                        "openai",
                        &request.model,
                        "stream",
                        status,
                        response,
                        &body,
                    )
                    .await)?
                }
                Err(error) => Err(stream_error(format!(
                    "openai stream error for model {}: {error}",
                    request.model
                )))?,
            };

            match event {
                Event::Open => {}
                Event::Message(message) => {
                    tracing::debug!(
                        stream_elapsed_ms = stream_trace_elapsed_ms(),
                        event = %message.event,
                        data_len = message.data.len(),
                        "openai chat completions stream chunk received"
                    );
                    tracing::trace!(
                        event = %message.event,
                        data_len = message.data.len(),
                        data = %message.data,
                        "openai chat completions raw stream event"
                    );
                    if message.data == "[DONE]" {
                        break;
                    }

                    let value: Value = serde_json::from_str(&message.data)
                        .map_err(|error| {
                            anyhow::anyhow!("failed to parse openai stream chunk: {error}")
                        })?;
                    if let Some(error) = provider_error_from_payload(&value, &request) {
                        Err(error)?;
                    }
                    let chunk: ChatCompletionStreamChunk = serde_json::from_value(value)
                        .map_err(|error| {
                            anyhow::anyhow!("failed to deserialize openai stream chunk: {error}")
                        })?;

                    for stream_event in state.apply_chunk(chunk) {
                        if let StreamEvent::TextDelta { index, text } = &stream_event {
                            if let Some(assistant_token_text) = assistant_token_log_preview(text) {
                                tracing::debug!(
                                    stream_elapsed_ms = stream_trace_elapsed_ms(),
                                    index,
                                    delta_len = text.len(),
                                    assistant_token_text = %assistant_token_text,
                                    "openai chat completions text delta emitted"
                                );
                            } else {
                                tracing::debug!(
                                    stream_elapsed_ms = stream_trace_elapsed_ms(),
                                    index,
                                    delta_len = text.len(),
                                    "openai chat completions text delta emitted"
                                );
                            }
                        }
                        yield stream_event;
                    }
                }
            }
        }

        for stream_event in state.finish_pending_content() {
            yield stream_event;
        }
        yield StreamEvent::MessageDone {
            response: state.into_response(),
        };
    };

    Ok(Box::pin(stream))
}

fn stream_trace_elapsed_ms() -> u128 {
    static STREAM_TRACE_START: OnceLock<Instant> = OnceLock::new();
    STREAM_TRACE_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}

fn assistant_token_log_preview(text: &str) -> Option<String> {
    assistant_token_logging_enabled()
        .then(|| format_assistant_token_log_preview(text, assistant_token_log_max_chars()))
}

fn assistant_token_logging_enabled() -> bool {
    static ASSISTANT_TOKEN_LOGGING_ENABLED: OnceLock<bool> = OnceLock::new();
    *ASSISTANT_TOKEN_LOGGING_ENABLED.get_or_init(|| {
        std::env::var("DEVO_LOG_ASSISTANT_TOKEN_TEXT")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.as_str(),
                    "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
                )
            })
    })
}

fn assistant_token_log_max_chars() -> usize {
    static ASSISTANT_TOKEN_LOG_MAX_CHARS: OnceLock<usize> = OnceLock::new();
    *ASSISTANT_TOKEN_LOG_MAX_CHARS.get_or_init(|| {
        std::env::var("DEVO_ASSISTANT_TOKEN_LOG_MAX_CHARS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(512)
    })
}

fn format_assistant_token_log_preview(text: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(1);
    let mut preview = String::new();
    let mut chars = text.chars();
    for ch in chars.by_ref().take(max_chars) {
        preview.extend(ch.escape_default());
    }
    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
}

#[path = "stream_state.rs"]
mod stream_state;
use stream_state::ChatCompletionStreamState;

#[derive(Debug, Deserialize)]
pub(super) struct ChatCompletionStreamChunk {
    #[serde(default)]
    id: Option<String>,
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
    #[serde(default, deserialize_with = "deserialize_null_vec")]
    choices: Vec<ChatCompletionStreamChoice>,
    #[serde(default)]
    usage: Option<OpenAICompletionUsage>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct ChatCompletionStreamChoice {
    #[serde(default)]
    delta: ChatCompletionStreamDelta,
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    finish_reason: Option<String>,
    #[serde(default)]
    logprobs: Option<OpenAIChoiceLogprobs>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionStreamDelta {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Option<String>,
    /// DeepSeek / vLLM use `reasoning_content`; Ollama's OpenAI-compat layer
    /// currently emits the same payload under `reasoning`.
    #[serde(default, alias = "reasoning")]
    reasoning_content: Option<String>,
    #[serde(default)]
    refusal: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_vec")]
    tool_calls: Vec<ChatCompletionStreamToolCallDelta>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct ChatCompletionStreamToolCallDelta {
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    function: Option<ChatCompletionStreamFunctionDelta>,
    #[serde(default)]
    custom: Option<ChatCompletionStreamCustomDelta>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionStreamFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionStreamCustomDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<String>,
}

pub(super) fn stop_reason_to_finish_reason(reason: &StopReason) -> String {
    match reason {
        StopReason::ToolUse => "tool_calls".to_string(),
        StopReason::MaxTokens => "length".to_string(),
        StopReason::EndTurn => "stop".to_string(),
        StopReason::StopSequence => "content_filter".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use devo_protocol::{ResponseContent, ResponseExtra, StopReason, Usage};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    #[test]
    fn text_chunks_accumulate_into_response() {
        let mut state = ChatCompletionStreamState::default();

        let events = state.apply_chunk(parse_chunk(json!({
            "id": "chatcmpl-123",
            "choices": [
                {
                    "delta": {
                        "role": "assistant",
                        "content": ""
                    }
                }
            ]
        })));
        assert!(events.is_empty());

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "content": "Hello"
                    }
                }
            ]
        })));
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], StreamEvent::TextStart { index: 0 }));
        assert!(matches!(
            &events[1],
            StreamEvent::TextDelta { index: 0, text } if text == "Hello"
        ));

        state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "content": " world"
                    },
                    "finish_reason": "stop"
                }
            ]
        })));

        let response = state.into_response();
        assert_eq!(response.id, "chatcmpl-123");
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(response.usage.input_tokens, 0);
        assert_eq!(response.usage.output_tokens, 0);
        assert_eq!(response.content.len(), 1);
        match &response.content[0] {
            ResponseContent::Text(text) => assert_eq!(text, "Hello world"),
            other => panic!("expected text content, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_block_starts_before_arguments_arrive() {
        let mut state = ChatCompletionStreamState::default();

        let events = state.apply_chunk(parse_chunk(json!({
            "id": "chatcmpl-456",
            "choices": [
                {
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_123",
                                "function": {
                                    "name": "get_weather"
                                }
                            }
                        ]
                    }
                }
            ]
        })));

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::ToolCallStart { index: 1, id, name, input }
            if id == "call_123" && name == "get_weather" && input == &json!({})
        ));

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "function": {
                                    "arguments": "{\"city\":\"Boston\"}"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        })));

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::ToolCallInputDelta {
                index: 1,
                partial_json,
            } if partial_json == "{\"city\":\"Boston\"}"
        ));

        let response = state.into_response();
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(response.content.len(), 1);
        match &response.content[0] {
            ResponseContent::ToolUse { id, name, input } => {
                assert_eq!(id, "call_123");
                assert_eq!(name, "get_weather");
                assert_eq!(input, &json!({"city": "Boston"}));
            }
            other => panic!("expected tool content, got {other:?}"),
        }
    }

    #[test]
    fn usage_chunks_and_malformed_tool_json_are_handled() {
        let mut state = ChatCompletionStreamState::default();

        let events = state.apply_chunk(parse_chunk(json!({
            "id": "chatcmpl-789",
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 7,
                "total_tokens": 18,
                "prompt_tokens_details": {
                    "cached_tokens": 3
                },
                "completion_tokens_details": {
                    "reasoning_tokens": 2
                }
            }
        })));

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::UsageDelta(Usage {
                input_tokens: 11,
                output_tokens: 7,
                cache_read_input_tokens: Some(3),
                ..
            })
        ));

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_bad",
                                "function": {
                                    "name": "broken_tool",
                                    "arguments": "{"
                                }
                            }
                        ]
                    }
                }
            ]
        })));

        assert_eq!(events.len(), 2);

        let response = state.into_response();
        assert_eq!(response.usage.input_tokens, 11);
        assert_eq!(response.usage.output_tokens, 7);
        assert_eq!(response.usage.cache_read_input_tokens, Some(3));
        assert_eq!(response.usage.reasoning_output_tokens, Some(2));
        assert_eq!(response.usage.total_tokens, Some(18));
        assert_eq!(response.content.len(), 1);
        match &response.content[0] {
            ResponseContent::ToolUse { id, name, input } => {
                assert_eq!(id, "call_bad");
                assert_eq!(name, "broken_tool");
                assert_eq!(input, &json!({}));
            }
            other => panic!("expected tool content, got {other:?}"),
        }
        assert!(response.metadata.extras.iter().any(|extra| matches!(
            extra,
            ResponseExtra::ProviderSpecific { provider, payload }
            if provider == "openai" && payload["usage"]["prompt_tokens"] == json!(11)
        )));
    }

    #[test]
    fn chunks_without_choices_are_ignored_safely() {
        let mut state = ChatCompletionStreamState::default();

        let events = state.apply_chunk(parse_chunk(json!({
            "id": "chatcmpl-empty",
            "object": "chat.completion.chunk",
            "created": 1741569952,
            "model": "gpt-5.4"
        })));

        assert!(events.is_empty());

        let response = state.into_response();
        assert_eq!(response.id, "chatcmpl-empty");
        assert!(response.content.is_empty());
        assert!(response.metadata.extras.iter().any(|extra| matches!(
            extra,
            ResponseExtra::ProviderSpecific { provider, payload }
            if provider == "openai"
                && payload["object"] == json!("chat.completion.chunk")
                && payload["model"] == json!("gpt-5.4")
        )));
    }

    #[test]
    fn interleaved_text_and_tool_call_chunks_preserve_event_order() {
        let mut state = ChatCompletionStreamState::default();

        let events = state.apply_chunk(parse_chunk(json!({
            "id": "chatcmpl-mixed",
            "choices": [
                {
                    "delta": {
                        "content": "Let me "
                    }
                }
            ]
        })));
        assert_eq!(events.len(), 2);

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_mix",
                                "type": "function",
                                "function": {
                                    "name": "lookup_weather"
                                }
                            }
                        ]
                    }
                }
            ]
        })));
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::ToolCallStart { index: 1, id, name, .. }
            if id == "call_mix" && name == "lookup_weather"
        ));

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "content": "check that",
                        "tool_calls": [
                            {
                                "index": 0,
                                "function": {
                                    "arguments": "{\"city\":\"Boston\"}"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        })));
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            StreamEvent::TextDelta { index: 0, text } if text == "check that"
        ));
        assert!(matches!(
            &events[1],
            StreamEvent::ToolCallInputDelta { index: 1, partial_json }
            if partial_json == "{\"city\":\"Boston\"}"
        ));

        let response = state.into_response();
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(response.content.len(), 2);
    }

    #[test]
    fn custom_tool_call_stream_uses_string_input() {
        let mut state = ChatCompletionStreamState::default();

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_custom",
                                "type": "custom",
                                "custom": {
                                    "name": "draft_sql",
                                    "input": "select *"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        })));

        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            StreamEvent::ToolCallStart { index: 1, id, name, input }
            if id == "call_custom" && name == "draft_sql" && input == &json!("")
        ));
        assert!(matches!(
            &events[1],
            StreamEvent::ToolCallInputDelta { index: 1, partial_json }
            if partial_json == "select *"
        ));

        let response = state.into_response();
        match &response.content[0] {
            ResponseContent::ToolUse { id, name, input } => {
                assert_eq!(id, "call_custom");
                assert_eq!(name, "draft_sql");
                assert_eq!(input, &json!("select *"));
            }
            other => panic!("expected custom tool content, got {other:?}"),
        }
    }

    #[test]
    fn deepseek_v4_stream_heals_dsml_text_tool_calls() {
        let mut state = ChatCompletionStreamState::for_model("deepseek-v4-pro");

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "content": "<｜DSML｜tool_calls><｜DSML｜invoke name=\"web_search\">"
                    }
                }
            ]
        })));
        assert!(events.is_empty());

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "content": "<｜DSML｜parameter name=\"query\" string=\"true\">DeepSeek V4</｜DSML｜parameter></｜DSML｜invoke></｜DSML｜tool_calls>"
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        })));
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, StreamEvent::TextDelta { .. }))
        );

        let response = state.into_response();
        assert_eq!(
            response.content,
            vec![ResponseContent::ToolUse {
                id: "dsml_0_0".to_string(),
                name: "web_search".to_string(),
                input: json!({"query": "DeepSeek V4"}),
            }]
        );
    }

    #[test]
    fn finish_reason_closes_active_reasoning_content_phase() {
        let mut state = ChatCompletionStreamState::default();

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "reasoning_content": "plan"
                    },
                    "finish_reason": "stop"
                }
            ]
        })));

        assert_eq!(
            events,
            vec![
                StreamEvent::ReasoningStart { index: 1 },
                StreamEvent::ReasoningDelta {
                    index: 1,
                    text: "plan".to_string(),
                },
                StreamEvent::ReasoningDone { index: 1 },
            ]
        );
    }

    #[test]
    fn ollama_reasoning_field_alias_emits_reasoning_events() {
        let mut state = ChatCompletionStreamState::default();

        let events = state.apply_chunk(parse_chunk(json!({
            "id": "chatcmpl-ollama",
            "choices": [
                {
                    "delta": {
                        "reasoning": "plan via ollama",
                        "content": "answer"
                    },
                    "finish_reason": "stop"
                }
            ]
        })));

        assert_eq!(
            events,
            vec![
                StreamEvent::ReasoningStart { index: 1 },
                StreamEvent::ReasoningDelta {
                    index: 1,
                    text: "plan via ollama".to_string(),
                },
                StreamEvent::TextStart { index: 0 },
                StreamEvent::TextDelta {
                    index: 0,
                    text: "answer".to_string(),
                },
                StreamEvent::ReasoningDone { index: 1 },
            ]
        );

        let response = state.into_response();
        assert!(response.metadata.extras.iter().any(|extra| matches!(
            extra,
            ResponseExtra::ReasoningText { text } if text == "plan via ollama"
        )));
        assert_eq!(
            response.content,
            vec![ResponseContent::Text("answer".to_string())]
        );
    }

    #[test]
    fn finish_pending_content_closes_tagged_reasoning_at_stream_end() {
        let mut state = ChatCompletionStreamState::default();

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "content": "<think>plan</think>"
                    }
                }
            ]
        })));
        assert_eq!(
            events,
            vec![
                StreamEvent::ReasoningStart { index: 1 },
                StreamEvent::ReasoningDelta {
                    index: 1,
                    text: "plan".to_string(),
                },
            ]
        );

        let events = state.finish_pending_content();

        assert_eq!(events, vec![StreamEvent::ReasoningDone { index: 1 }]);
    }

    #[test]
    fn finish_pending_content_flushes_buffered_tagged_text_at_stream_end() {
        let mut state = ChatCompletionStreamState::default();

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "content": "answer<th"
                    }
                }
            ]
        })));
        assert_eq!(
            events,
            vec![
                StreamEvent::TextStart { index: 0 },
                StreamEvent::TextDelta {
                    index: 0,
                    text: "answer".to_string(),
                },
            ]
        );

        let events = state.finish_pending_content();

        assert_eq!(
            events,
            vec![StreamEvent::TextDelta {
                index: 0,
                text: "<th".to_string(),
            }]
        );

        let response = state.into_response();
        assert_eq!(
            response.content,
            vec![ResponseContent::Text("answer<th".to_string())]
        );
    }

    fn parse_chunk(value: Value) -> ChatCompletionStreamChunk {
        serde_json::from_value(value).expect("valid stream chunk")
    }

    #[test]
    fn reasoning_content_and_tagged_text_emit_reasoning_events() {
        let mut state = ChatCompletionStreamState::default();

        let events = state.apply_chunk(parse_chunk(json!({
            "id": "chatcmpl-reasoning",
            "choices": [
                {
                    "delta": {
                        "reasoning_content": "internal plan",
                        "content": "<think>hidden</think>visible"
                    },
                    "finish_reason": "stop"
                }
            ]
        })));

        assert_eq!(events.len(), 6);
        assert!(matches!(
            &events[0],
            StreamEvent::ReasoningStart { index: 1 }
        ));
        assert!(matches!(
            &events[1],
            StreamEvent::ReasoningDelta { index: 1, text } if text == "internal plan"
        ));
        assert!(matches!(
            &events[2],
            StreamEvent::ReasoningDelta { index: 1, text } if text == "hidden"
        ));
        assert!(matches!(&events[3], StreamEvent::TextStart { index: 0 }));
        assert!(matches!(
            &events[4],
            StreamEvent::TextDelta { index: 0, text } if text == "visible"
        ));
        assert!(matches!(
            &events[5],
            StreamEvent::ReasoningDone { index: 1 }
        ));

        let response = state.into_response();
        assert_eq!(response.id, "chatcmpl-reasoning");
        assert_eq!(
            response.content,
            vec![ResponseContent::Text("visible".into())]
        );
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(response.usage, Usage::default());
        assert!(response.metadata.extras.iter().any(|extra| matches!(
            extra,
            ResponseExtra::ReasoningText { text } if text == "internal planhidden"
        )));
        assert!(response.metadata.extras.iter().any(|extra| matches!(
            extra,
            ResponseExtra::ProviderSpecific { provider, payload }
            if provider == "openai"
                && payload["choices"][0]["index"] == json!(0)
        )));
    }

    #[test]
    fn reasoning_content_phase_done_before_tool_call_when_field_disappears() {
        let mut state = ChatCompletionStreamState::default();

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "role": "assistant",
                        "content": null,
                        "reasoning_content": ""
                    },
                    "finish_reason": null
                }
            ]
        })));
        assert!(events.is_empty());

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "content": null,
                        "reasoning_content": " wants"
                    },
                    "finish_reason": null
                }
            ]
        })));
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            StreamEvent::ReasoningStart { index: 1 }
        ));
        assert!(matches!(
            &events[1],
            StreamEvent::ReasoningDelta { index: 1, text } if text == " wants"
        ));

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "content": null,
                        "reasoning_content": "."
                    },
                    "finish_reason": null
                }
            ]
        })));
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::ReasoningDelta { index: 1, text } if text == "."
        ));

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_00_600mpUqdusY9jkJm31MM0811",
                                "type": "function",
                                "function": {
                                    "name": "read",
                                    "arguments": ""
                                }
                            }
                        ]
                    },
                    "finish_reason": null
                }
            ]
        })));
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            StreamEvent::ReasoningDone { index: 1 }
        ));
        assert!(matches!(
            &events[1],
            StreamEvent::ToolCallStart { index: 1, id, name, .. }
            if id == "call_00_600mpUqdusY9jkJm31MM0811" && name == "read"
        ));
    }

    #[test]
    fn reasoning_content_null_ends_active_reasoning_phase_once() {
        let mut state = ChatCompletionStreamState::default();

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "reasoning_content": "plan"
                    },
                    "finish_reason": null
                }
            ]
        })));
        assert_eq!(events.len(), 2);

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "reasoning_content": null
                    },
                    "finish_reason": null
                }
            ]
        })));
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StreamEvent::ReasoningDone { index: 1 }
        ));

        let events = state.apply_chunk(parse_chunk(json!({
            "choices": [
                {
                    "delta": {
                        "content": "answer"
                    },
                    "finish_reason": "stop"
                }
            ]
        })));
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], StreamEvent::TextStart { index: 0 }));
        assert!(matches!(
            &events[1],
            StreamEvent::TextDelta { index: 0, text } if text == "answer"
        ));
    }
}
