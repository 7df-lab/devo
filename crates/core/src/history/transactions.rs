//! Transaction boundaries and lossless prompt reconstruction shared by compaction.

use std::collections::HashSet;

use devo_protocol::{ContentBlock, Message, Role};

use crate::ResponseItem;

/// Offsets after complete transactions. Every suffix starting at one of these
/// offsets contains both sides of any tool batch, including parallel calls.
pub(super) fn completed_boundaries(items: &[ResponseItem]) -> Vec<usize> {
    let mut pending = HashSet::new();
    let mut boundaries = vec![0];
    let mut before_reasoning = None;
    for (index, item) in items.iter().enumerate() {
        match item {
            ResponseItem::ToolCall { id, .. } => {
                // Reasoning (including signed provider records) and the tool use
                // from that assistant message must be replayed together.
                if let Some(count) = before_reasoning.take() {
                    boundaries.truncate(count);
                }
                pending.insert(id);
            }
            ResponseItem::ToolCallOutput { tool_use_id, .. } => {
                pending.remove(tool_use_id);
                before_reasoning = None;
            }
            ResponseItem::Reason { .. } => {
                before_reasoning.get_or_insert(boundaries.len());
            }
            ResponseItem::Message(msg) if msg.role != Role::Assistant => before_reasoning = None,
            ResponseItem::Message(msg) => {
                if msg.content.iter().any(|block| {
                    matches!(
                        block,
                        ContentBlock::ProviderReasoning { .. } | ContentBlock::Reasoning { .. }
                    )
                }) {
                    before_reasoning.get_or_insert(boundaries.len());
                }
            }
        }
        if pending.is_empty() {
            boundaries.push(index + 1);
        }
    }
    boundaries
}

/// Reconstitute all response variants, grouping adjacent assistant/tool messages.
/// This conversion never removes an intent merely because its result is pending.
pub fn response_items_to_messages(items: &[ResponseItem]) -> Vec<Message> {
    let mut messages: Vec<Message> = Vec::new();
    for item in items {
        let message = match item {
            ResponseItem::Message(message) => message.clone(),
            ResponseItem::Reason { text } => Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Reasoning { text: text.clone() }],
            },
            ResponseItem::ToolCall { id, name, input } => Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                }],
            },
            ResponseItem::ToolCallOutput {
                tool_use_id,
                content,
                is_error,
            } => Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                }],
            },
        };
        if let Some(last) = messages.last_mut()
            && last.role == message.role
            && (message.role == Role::Assistant
                || (last
                    .content
                    .iter()
                    .all(|block| matches!(block, ContentBlock::ToolResult { .. }))
                    && message
                        .content
                        .iter()
                        .all(|block| matches!(block, ContentBlock::ToolResult { .. }))))
        {
            last.content.extend(message.content);
        } else {
            messages.push(message);
        }
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn batch() -> Vec<ResponseItem> {
        vec![
            ResponseItem::Message(Message::user("old")),
            ResponseItem::ToolCall {
                id: "a".into(),
                name: "read".into(),
                input: serde_json::json!({}),
            },
            ResponseItem::ToolCall {
                id: "b".into(),
                name: "read".into(),
                input: serde_json::json!({}),
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "b".into(),
                content: "B".into(),
                is_error: false,
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "a".into(),
                content: "A".into(),
                is_error: false,
            },
        ]
    }

    #[test]
    fn parallel_batch_is_indivisible() {
        let items = batch();
        assert_eq!(completed_boundaries(&items), vec![0, 1, 5]);
        assert_eq!(completed_boundaries(&items[..4]), vec![0, 1]);
    }

    #[test]
    fn messages_round_trip_all_tool_items() {
        let items = batch();
        let restored = response_items_to_messages(&items)
            .into_iter()
            .flat_map(crate::response_item::message_to_response_items)
            .collect::<Vec<_>>();
        assert_eq!(restored, items);
    }
}
