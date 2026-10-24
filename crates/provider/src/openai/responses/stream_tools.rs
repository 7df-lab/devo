//! Ordered tool state accumulated across Responses stream events.

use super::response::parse_function_call_arguments_json;
use devo_protocol::ResponseContent;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug)]
pub(super) struct ResponsesStreamToolCall {
    pub(super) index: usize,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) arguments_json: String,
}

pub(super) type ResponsesStreamHostedToolCall =
    (usize, String, String, Value, Option<Value>, Option<String>);

pub(super) fn responses_stream_tool_content(
    tool_calls: &HashMap<String, ResponsesStreamToolCall>,
    hosted_tool_calls: &HashMap<String, ResponsesStreamHostedToolCall>,
) -> Vec<ResponseContent> {
    let mut indexed_content = Vec::new();
    for call in tool_calls.values() {
        indexed_content.push((
            call.index,
            ResponseContent::ToolUse {
                id: call.id.clone(),
                name: call.name.clone(),
                input: parse_function_call_arguments_json(&call.arguments_json),
            },
        ));
    }
    for (index, id, name, input, output, status) in hosted_tool_calls.values() {
        indexed_content.push((
            *index,
            ResponseContent::HostedToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                output: output.clone(),
                status: status.clone(),
            },
        ));
    }
    indexed_content.sort_by_key(|(index, _)| *index);
    indexed_content
        .into_iter()
        .map(|(_, content)| content)
        .collect()
}
