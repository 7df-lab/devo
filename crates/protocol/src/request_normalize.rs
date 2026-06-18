//! Normalization helpers for provider request messages.
//!
//! Some providers require tool results to appear immediately after the assistant
//! tool-use message that requested them. Devo can inject user-role context
//! messages between those records, so this pass groups the required tool results
//! first while preserving the remaining user content after them.

use std::collections::HashSet;
use std::collections::VecDeque;

use crate::RequestContent;
use crate::RequestMessage;
use crate::Role;

pub fn normalize_tool_result_messages(messages: &mut Vec<RequestMessage>) {
    let capacity = messages.len();
    let mut previous = VecDeque::from(std::mem::replace(messages, Vec::with_capacity(capacity)));
    while let Some(message) = previous.pop_front() {
        let message = reorder_user_tool_results(message);
        let Some(pending_tool_use_ids) = tool_use_ids(&message) else {
            messages.push(message);
            continue;
        };

        let tool_result_message =
            collect_tool_result_followup(&mut previous, &pending_tool_use_ids);
        messages.push(message);
        if let Some(tool_result_message) = tool_result_message {
            messages.push(tool_result_message);
        }
    }
}

fn tool_use_ids(message: &RequestMessage) -> Option<HashSet<&str>> {
    if message.role != Role::Assistant.as_str() {
        return None;
    }

    let mut ids = None::<HashSet<&str>>;
    for content in &message.content {
        match content {
            RequestContent::ToolUse { id, .. } => {
                ids.get_or_insert_with(|| HashSet::with_capacity(message.content.len()))
                    .insert(id.as_str());
            }
            RequestContent::Text { .. }
            | RequestContent::Reasoning { .. }
            | RequestContent::ProviderReasoning { .. }
            | RequestContent::HostedToolUse { .. }
            | RequestContent::ToolResult { .. } => {}
        }
    }
    ids
}

fn collect_tool_result_followup(
    messages: &mut VecDeque<RequestMessage>,
    pending_tool_use_ids: &HashSet<&str>,
) -> Option<RequestMessage> {
    let mut found_ids = HashSet::with_capacity(pending_tool_use_ids.len());
    let mut consumed = 0;

    for message in messages.iter() {
        if message.role != Role::User.as_str() {
            break;
        }

        for content in &message.content {
            match content {
                RequestContent::ToolResult { tool_use_id, .. } => {
                    if pending_tool_use_ids.contains(tool_use_id.as_str()) {
                        found_ids.insert(tool_use_id.as_str());
                    }
                }
                RequestContent::Text { .. }
                | RequestContent::Reasoning { .. }
                | RequestContent::ProviderReasoning { .. }
                | RequestContent::HostedToolUse { .. }
                | RequestContent::ToolUse { .. } => {}
            }
        }

        consumed += 1;
        if found_ids.len() == pending_tool_use_ids.len() {
            break;
        }
    }

    if consumed == 0 || found_ids.len() != pending_tool_use_ids.len() {
        return None;
    }

    let (tool_result_count, other_content_count) = messages
        .iter()
        .take(consumed)
        .flat_map(|message| &message.content)
        .fold((0, 0), |(tool_results, other_content), content| {
            if matches!(content, RequestContent::ToolResult { .. }) {
                (tool_results + 1, other_content)
            } else {
                (tool_results, other_content + 1)
            }
        });
    let mut tool_results = Vec::with_capacity(tool_result_count);
    let mut other_content = Vec::with_capacity(other_content_count);
    for _ in 0..consumed {
        let message = messages
            .pop_front()
            .expect("consumed count only includes queued messages");
        for content in message.content {
            match content {
                RequestContent::ToolResult { .. } => tool_results.push(content),
                RequestContent::Text { .. }
                | RequestContent::Reasoning { .. }
                | RequestContent::ProviderReasoning { .. }
                | RequestContent::HostedToolUse { .. }
                | RequestContent::ToolUse { .. } => other_content.push(content),
            }
        }
    }

    tool_results.extend(other_content);
    Some(RequestMessage {
        role: Role::User.as_str().to_string(),
        content: tool_results,
    })
}

fn reorder_user_tool_results(mut message: RequestMessage) -> RequestMessage {
    if message.role != Role::User.as_str() {
        return message;
    }
    let tool_result_count = message
        .content
        .iter()
        .filter(|content| matches!(content, RequestContent::ToolResult { .. }))
        .count();
    if tool_result_count == 0 {
        return message;
    }

    let mut tool_results = Vec::with_capacity(tool_result_count);
    let mut other_content = Vec::with_capacity(message.content.len() - tool_result_count);
    for content in message.content {
        match content {
            RequestContent::ToolResult { .. } => tool_results.push(content),
            RequestContent::Text { .. }
            | RequestContent::Reasoning { .. }
            | RequestContent::ProviderReasoning { .. }
            | RequestContent::HostedToolUse { .. }
            | RequestContent::ToolUse { .. } => other_content.push(content),
        }
    }
    tool_results.extend(other_content);
    message.content = tool_results;
    message
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::normalize_tool_result_messages;
    use crate::RequestContent;
    use crate::RequestMessage;
    use crate::Role;

    fn assert_messages_eq(actual: &[RequestMessage], expected: &[RequestMessage]) {
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }

    fn assistant(content: Vec<RequestContent>) -> RequestMessage {
        RequestMessage {
            role: Role::Assistant.as_str().to_string(),
            content,
        }
    }

    fn user(content: Vec<RequestContent>) -> RequestMessage {
        RequestMessage {
            role: Role::User.as_str().to_string(),
            content,
        }
    }

    fn tool_use(id: &str) -> RequestContent {
        RequestContent::ToolUse {
            id: id.to_string(),
            name: "read".to_string(),
            input: json!({ "path": "Cargo.toml" }),
        }
    }

    fn tool_result(id: &str, content: &str) -> RequestContent {
        RequestContent::ToolResult {
            tool_use_id: id.to_string(),
            content: content.to_string(),
            is_error: None,
        }
    }

    fn text(content: &str) -> RequestContent {
        RequestContent::Text {
            text: content.to_string(),
        }
    }

    #[test]
    fn groups_split_parallel_tool_results_after_assistant_tool_use() {
        let mut messages = vec![
            assistant(vec![tool_use("call-1"), tool_use("call-2")]),
            user(vec![tool_result("call-1", "first")]),
            user(vec![tool_result("call-2", "second")]),
            assistant(vec![text("done")]),
        ];

        normalize_tool_result_messages(&mut messages);

        assert_messages_eq(
            &messages,
            &[
                assistant(vec![tool_use("call-1"), tool_use("call-2")]),
                user(vec![
                    tool_result("call-1", "first"),
                    tool_result("call-2", "second"),
                ]),
                assistant(vec![text("done")]),
            ],
        );
    }

    #[test]
    fn moves_intervening_user_text_after_required_tool_results() {
        let mut messages = vec![
            assistant(vec![tool_use("call-1")]),
            user(vec![text(
                "<context_changes>model changed</context_changes>",
            )]),
            user(vec![tool_result("call-1", "ok")]),
            assistant(vec![text("done")]),
        ];

        normalize_tool_result_messages(&mut messages);

        assert_messages_eq(
            &messages,
            &[
                assistant(vec![tool_use("call-1")]),
                user(vec![
                    tool_result("call-1", "ok"),
                    text("<context_changes>model changed</context_changes>"),
                ]),
                assistant(vec![text("done")]),
            ],
        );
    }

    #[test]
    fn puts_tool_results_first_within_user_message() {
        let mut messages = vec![
            assistant(vec![tool_use("call-1")]),
            user(vec![text("result follows"), tool_result("call-1", "ok")]),
        ];

        normalize_tool_result_messages(&mut messages);

        assert_messages_eq(
            &messages,
            &[
                assistant(vec![tool_use("call-1")]),
                user(vec![tool_result("call-1", "ok"), text("result follows")]),
            ],
        );
    }

    #[test]
    fn leaves_text_in_place_when_tool_result_is_missing() {
        let mut messages = vec![
            assistant(vec![tool_use("call-1")]),
            user(vec![text("follow up")]),
            assistant(vec![text("next")]),
        ];
        let expected = messages.clone();

        normalize_tool_result_messages(&mut messages);

        assert_messages_eq(&messages, &expected);
    }
}
