use std::collections::HashSet;

use crate::RequestContent;
use crate::RequestMessage;
use crate::Role;

pub fn normalize_tool_result_messages(messages: &mut Vec<RequestMessage>) {
    let capacity = messages.len();
    let previous = std::mem::replace(messages, Vec::with_capacity(capacity));
    let mut index = 0;

    while index < previous.len() {
        let message = reorder_user_tool_results(previous[index].clone());
        let pending_tool_use_ids = tool_use_ids(&message);
        messages.push(message);
        index += 1;

        if pending_tool_use_ids.is_empty() {
            continue;
        }

        if let Some((tool_result_message, consumed)) =
            collect_tool_result_followup(&previous, index, &pending_tool_use_ids)
        {
            messages.push(tool_result_message);
            index += consumed;
        }
    }
}

fn tool_use_ids(message: &RequestMessage) -> Vec<String> {
    if message.role != Role::Assistant.as_str() {
        return Vec::new();
    }

    message
        .content
        .iter()
        .filter_map(|content| match content {
            RequestContent::ToolUse { id, .. } => Some(id.clone()),
            RequestContent::Text { .. }
            | RequestContent::Reasoning { .. }
            | RequestContent::ProviderReasoning { .. }
            | RequestContent::HostedToolUse { .. }
            | RequestContent::ToolResult { .. } => None,
        })
        .collect()
}

fn collect_tool_result_followup(
    messages: &[RequestMessage],
    start: usize,
    pending_tool_use_ids: &[String],
) -> Option<(RequestMessage, usize)> {
    let pending_ids = pending_tool_use_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut found_ids = HashSet::with_capacity(pending_ids.len());
    let mut tool_results = Vec::new();
    let mut other_content = Vec::new();
    let mut consumed = 0;

    for message in messages.iter().skip(start) {
        if message.role != Role::User.as_str() {
            break;
        }

        for content in &message.content {
            match content {
                RequestContent::ToolResult { tool_use_id, .. } => {
                    if pending_ids.contains(tool_use_id.as_str()) {
                        found_ids.insert(tool_use_id.as_str());
                    }
                    tool_results.push(content.clone());
                }
                RequestContent::Text { .. }
                | RequestContent::Reasoning { .. }
                | RequestContent::ProviderReasoning { .. }
                | RequestContent::HostedToolUse { .. }
                | RequestContent::ToolUse { .. } => other_content.push(content.clone()),
            }
        }

        consumed += 1;
        if pending_tool_use_ids
            .iter()
            .all(|id| found_ids.contains(id.as_str()))
        {
            break;
        }
    }

    if consumed == 0
        || !pending_tool_use_ids
            .iter()
            .all(|id| found_ids.contains(id.as_str()))
    {
        return None;
    }

    tool_results.extend(other_content);
    Some((
        RequestMessage {
            role: Role::User.as_str().to_string(),
            content: tool_results,
        },
        consumed,
    ))
}

fn reorder_user_tool_results(mut message: RequestMessage) -> RequestMessage {
    if message.role != Role::User.as_str()
        || !message
            .content
            .iter()
            .any(|content| matches!(content, RequestContent::ToolResult { .. }))
    {
        return message;
    }

    let mut tool_results = Vec::new();
    let mut other_content = Vec::new();
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
