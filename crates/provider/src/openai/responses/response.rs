//! OpenAI Responses output and usage decoding.

use anyhow::Result;
use devo_protocol::{
    ModelResponse, ResponseContent, ResponseExtra, ResponseMetadata, StopReason, Usage,
};
use serde_json::{Value, json};

use crate::text_normalization::split_tagged_text;

pub(super) fn parse_response(value: Value) -> Result<ModelResponse> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut content = Vec::new();
    let mut metadata = ResponseMetadata::default();

    if let Some(output) = value.get("output").and_then(Value::as_array) {
        for item in output {
            if let Some(reasoning_content) = item.get("reasoning_content").and_then(Value::as_str) {
                metadata.extras.push(ResponseExtra::ReasoningText {
                    text: reasoning_content.to_string(),
                });
            }
            if matches!(item.get("type").and_then(Value::as_str), Some("message")) {
                if let Some(items) = item.get("content").and_then(Value::as_array) {
                    for message_item in items {
                        if let Some(text) = message_item.get("text").and_then(Value::as_str) {
                            let (assistant_text, reasoning) = split_tagged_text(text);
                            for text in reasoning {
                                if !text.is_empty() {
                                    metadata.extras.push(ResponseExtra::ReasoningText { text });
                                }
                            }
                            if !assistant_text.is_empty() {
                                content.push(ResponseContent::Text(assistant_text));
                            }
                            continue;
                        }
                        if let Some(parsed) = parse_message_content(message_item) {
                            content.push(parsed);
                        }
                    }
                }
                continue;
            }
            content.extend(parse_output_item(item));
        }
    }

    let stop_reason = value
        .get("status")
        .and_then(Value::as_str)
        .map(parse_status_reason)
        .or_else(|| {
            value
                .get("incomplete")
                .and_then(|item| item.get("reason"))
                .and_then(Value::as_str)
                .map(parse_status_reason)
        });

    let usage = value.get("usage").and_then(parse_usage).unwrap_or_default();

    Ok(ModelResponse {
        id,
        content,
        stop_reason,
        usage,
        metadata,
    })
}

pub(super) fn parse_output_item(item: &Value) -> Vec<ResponseContent> {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => item
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(parse_message_content)
            .collect(),
        Some("function_call") | Some("tool_call") => {
            let id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let input = parse_function_call_arguments(item);
            vec![ResponseContent::ToolUse { id, name, input }]
        }
        Some("web_search_call") => vec![parse_hosted_web_search_call(item)],
        Some("web_fetch_call") => vec![parse_hosted_web_fetch_call(item)],
        Some("reasoning") => Vec::new(),
        _ => Vec::new(),
    }
}

fn parse_message_content(item: &Value) -> Option<ResponseContent> {
    match item.get("type").and_then(Value::as_str) {
        Some("output_text") | Some("text") | Some("input_text") => {
            let assistant_text =
                split_tagged_text(item.get("text").and_then(Value::as_str).unwrap_or_default()).0;
            if assistant_text.is_empty() {
                None
            } else {
                Some(ResponseContent::Text(assistant_text))
            }
        }
        Some("tool_call") | Some("function_call") => Some(ResponseContent::ToolUse {
            id: item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            input: parse_function_call_arguments(item),
        }),
        Some("web_search_call") => Some(parse_hosted_web_search_call(item)),
        Some("web_fetch_call") => Some(parse_hosted_web_fetch_call(item)),
        _ => None,
    }
}

fn parse_function_call_arguments(item: &Value) -> Value {
    match item.get("arguments").or_else(|| item.get("input")) {
        Some(Value::String(arguments)) => parse_function_call_arguments_json(arguments),
        Some(Value::Null) | None => Value::Object(serde_json::Map::new()),
        Some(value) => value.clone(),
    }
}

pub(super) fn function_call_arguments_json(item: &Value) -> String {
    match item.get("arguments").or_else(|| item.get("input")) {
        Some(Value::String(arguments)) => arguments.clone(),
        Some(Value::Null) | None => String::new(),
        Some(value) => value.to_string(),
    }
}

pub(super) fn parse_function_call_arguments_json(arguments_json: &str) -> Value {
    if arguments_json.trim().is_empty() {
        return Value::Object(serde_json::Map::new());
    }
    serde_json::from_str(arguments_json).unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
}

fn hosted_tool_call_id(item: &Value) -> String {
    item.get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn hosted_tool_status(item: &Value) -> Option<String> {
    item.get("status")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn hosted_tool_output(item: &Value) -> Option<Value> {
    item.get("output")
        .or_else(|| item.get("results"))
        .or_else(|| item.get("result"))
        .or_else(|| item.get("content"))
        .cloned()
}

fn parse_hosted_tool_call(item: &Value, name: &str, input: Value) -> ResponseContent {
    ResponseContent::HostedToolUse {
        id: hosted_tool_call_id(item),
        name: name.to_string(),
        input,
        output: hosted_tool_output(item),
        status: hosted_tool_status(item),
    }
}

fn parse_hosted_web_search_call(item: &Value) -> ResponseContent {
    parse_hosted_tool_call(item, "web_search", hosted_web_search_input(item))
}

fn hosted_web_search_input(item: &Value) -> Value {
    if let Some(query) = item
        .get("action")
        .and_then(|action| action.get("query"))
        .and_then(Value::as_str)
    {
        return json!({ "query": query });
    }
    if let Some(query) = item.get("query").and_then(Value::as_str) {
        return json!({ "query": query });
    }
    item.get("action")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()))
}

fn parse_hosted_web_fetch_call(item: &Value) -> ResponseContent {
    parse_hosted_tool_call(item, "web_fetch", hosted_web_fetch_input(item))
}

fn hosted_web_fetch_input(item: &Value) -> Value {
    if let Some(url) = item
        .get("action")
        .and_then(|action| action.get("url"))
        .and_then(Value::as_str)
    {
        return json!({ "url": url });
    }
    if let Some(url) = item.get("url").and_then(Value::as_str) {
        return json!({ "url": url });
    }
    if let Some(url) = item
        .get("input")
        .and_then(|input| input.get("url"))
        .and_then(Value::as_str)
    {
        return json!({ "url": url });
    }
    item.get("action")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()))
}

pub(super) fn parse_usage(value: &Value) -> Option<Usage> {
    Some(Usage {
        input_tokens: value
            .get("input_tokens")
            .or_else(|| value.get("prompt_tokens"))
            .and_then(Value::as_u64)? as usize,
        output_tokens: value
            .get("output_tokens")
            .or_else(|| value.get("completion_tokens"))
            .and_then(Value::as_u64)? as usize,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: value
            .pointer("/input_tokens_details/cached_tokens")
            .or_else(|| value.pointer("/prompt_tokens_details/cached_tokens"))
            .or_else(|| value.get("cache_read_input_tokens"))
            .and_then(Value::as_u64)
            .map(|tokens| tokens as usize),
        reasoning_output_tokens: value
            .get("output_tokens_details")
            .or_else(|| value.get("completion_tokens_details"))
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(Value::as_u64)
            .map(|tokens| tokens as usize),
        total_tokens: value
            .get("total_tokens")
            .and_then(Value::as_u64)
            .map(|tokens| tokens as usize),
    })
}

fn parse_status_reason(value: &str) -> StopReason {
    match value {
        "completed" | "stop" | "end_turn" => StopReason::EndTurn,
        "incomplete" | "max_output_tokens" | "length" => StopReason::MaxTokens,
        "tool_use" | "tool_calls" => StopReason::ToolUse,
        "stop_sequence" | "content_filter" => StopReason::StopSequence,
        _ => StopReason::EndTurn,
    }
}
