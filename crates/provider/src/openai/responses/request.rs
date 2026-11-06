//! OpenAI Responses request encoding.

use devo_protocol::{ModelRequest, RequestContent};
use serde_json::{Value, json};

use super::super::capabilities::{OpenAITransport, resolve_request_profile};
use super::super::{
    OpenAIRole,
    shared::{request_role, responses_tool_definitions},
};
use crate::hosted_tools::append_openai_responses_hosted_tools;
use crate::merge_extra_body;

/// Builds the exact OpenAI Responses request body used by this provider.
pub(super) fn build_request(request: &ModelRequest, stream: bool) -> Value {
    let profile = resolve_request_profile(&request.model_slug, OpenAITransport::Responses);
    // `max_output_tokens` is intentionally omitted: codex-style backends
    // reject the parameter outright, and the Responses API already treats it
    // as optional (the model default applies when unset).
    let mut root = json!({
        "model": request.model,
        "input": build_input(request),
        "stream": stream,
    });

    if let Some(system) = &request.system {
        // The Responses wire carries the system prompt in `instructions`;
        // codex-style backends reject system-role input items outright.
        root["instructions"] = json!(system);
    }

    if let Some(tools) = &request.tools {
        root["tools"] = responses_tool_definitions(tools);
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

    if let Some(reasoning) = request.reasoning_effort {
        // "auto" keeps codex-style backends streaming reasoning summaries;
        // it is the documented default elsewhere, so it is safe to always set.
        root["reasoning"] = json!({ "effort": reasoning, "summary": "auto" });
    }

    // Responses backends derive usage from the terminal event; the codex
    // backend additionally rejects chat-completions-style `stream_options`
    // and requires stateless requests (no server-side response storage).
    root["store"] = json!(false);

    append_openai_responses_hosted_tools(&mut root, &request.hosted_tools);

    merge_extra_body(&mut root, request.extra_body.as_ref());

    root
}

fn build_input(request: &ModelRequest) -> Vec<Value> {
    let mut input = Vec::with_capacity(request.messages.len());

    for message in &request.messages {
        let role = request_role(&message.role);
        append_input_items(&mut input, role, &message.content);
    }

    input
}

fn append_input_items(input: &mut Vec<Value>, role: OpenAIRole, content: &[RequestContent]) {
    let mut message_blocks = Vec::new();
    let flush_message = |input: &mut Vec<Value>, blocks: &mut Vec<Value>| {
        if blocks.is_empty() {
            return;
        }
        input.push(json!({
            "type": "message",
            "role": role,
            "content": std::mem::take(blocks),
        }));
    };

    for block in content {
        match block {
            RequestContent::Text { text } => {
                // Responses message content: user/system use input_text;
                // assistant replay uses output_text (DeepSeek/OpenAI).
                let block_type = match role {
                    OpenAIRole::Assistant => "output_text",
                    _ => "input_text",
                };
                message_blocks.push(json!({
                    "type": block_type,
                    "text": text,
                }));
            }
            RequestContent::Reasoning { .. } => {
                // Message content only accepts input_text / output_text /
                // input_image / input_file. Nested `type: "reasoning"` causes
                // 400s on DeepSeek (and is not valid OpenAI message content).
                // Prior reasoning is omitted unless carried as a provider
                // reasoning item with a stable id (ProviderReasoning).
            }
            RequestContent::ProviderReasoning { .. } => {}
            RequestContent::Image {
                mime_type,
                data_base64,
            } => {
                message_blocks.push(json!({
                    "type": "input_image",
                    "image_url": format!("data:{mime_type};base64,{data_base64}"),
                }));
            }
            RequestContent::ToolUse {
                id,
                name,
                input: args,
            } => {
                flush_message(input, &mut message_blocks);
                input.push(json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string()),
                }));
            }
            RequestContent::ToolResult {
                tool_use_id,
                content,
                is_error: _,
            } => {
                flush_message(input, &mut message_blocks);
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": tool_use_id,
                    "output": content,
                }));
            }
            RequestContent::HostedToolUse {
                id,
                name,
                input: hosted_input,
                output,
                status,
            } => {
                flush_message(input, &mut message_blocks);
                // DeepSeek / OpenAI Responses expect hosted calls as top-level
                // items that are passed back as-is (not nested message content).
                let item_type = match name.as_str() {
                    "web_search" => "web_search_call",
                    "web_fetch" => "web_fetch_call",
                    other => other,
                };
                let mut item = json!({
                    "type": item_type,
                    "id": id,
                    "status": status.clone().unwrap_or_else(|| "completed".to_string()),
                });
                if name == "web_search" {
                    if let Some(query) = hosted_input.get("query") {
                        item["action"] = json!({
                            "type": "search",
                            "query": query,
                        });
                    } else if let Some(action) = hosted_input.get("action") {
                        item["action"] = action.clone();
                    }
                } else if !hosted_input.is_null() {
                    item["action"] = hosted_input.clone();
                }
                if let Some(output) = output {
                    item["output"] = output.clone();
                }
                input.push(item);
            }
        }
    }

    flush_message(input, &mut message_blocks);
}
