use devo_protocol::{ContentBlock, Message, Role};

/// Inserts one context-diff message for the next model request without
/// retroactively splitting provider tool-call adjacency.
pub fn insert_context_diff_message(messages: &mut Vec<Message>, diff: Message) {
    let insert_at = if messages.last().is_some_and(is_user_text_message) {
        messages.len().saturating_sub(1)
    } else {
        messages.len()
    };
    messages.insert(insert_at, diff);
}

fn is_user_text_message(message: &Message) -> bool {
    message.role == Role::User
        && message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn context_diff() -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "<context_changes>\nmodel: a -> b\n</context_changes>".into(),
            }],
        }
    }

    #[test]
    fn places_diff_before_current_user_request() {
        let mut messages = vec![
            Message::user("first"),
            Message::assistant_text("reply"),
            Message::user("second"),
        ];

        insert_context_diff_message(&mut messages, context_diff());

        assert_eq!(messages.len(), 4);
        let ContentBlock::Text { text } = &messages[2].content[0] else {
            panic!("expected diff text");
        };
        assert!(text.contains("<context_changes>"));
        assert_eq!(messages[3], Message::user("second"));
    }

    #[test]
    fn appends_after_completed_turn_instead_of_rewriting_old_request() {
        let mut messages = vec![Message::user("first"), Message::assistant_text("reply")];

        insert_context_diff_message(&mut messages, context_diff());

        assert_eq!(messages.len(), 3);
        let ContentBlock::Text { text } = &messages[2].content[0] else {
            panic!("expected diff text");
        };
        assert!(text.contains("<context_changes>"));
    }

    #[test]
    fn does_not_split_assistant_tool_call_from_tool_result() {
        let mut messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call-1".into(),
                    name: "read".into(),
                    input: serde_json::json!({ "file_path": "Cargo.toml" }),
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call-1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            },
            Message::assistant_text("done"),
        ];

        insert_context_diff_message(&mut messages, context_diff());

        assert_eq!(messages.len(), 4);
        assert!(matches!(
            messages[0].content.as_slice(),
            [ContentBlock::ToolUse { id, .. }] if id == "call-1"
        ));
        assert!(matches!(
            messages[1].content.as_slice(),
            [ContentBlock::ToolResult { tool_use_id, .. }] if tool_use_id == "call-1"
        ));
        assert!(matches!(
            messages[3].content.as_slice(),
            [ContentBlock::Text { text }] if text.contains("<context_changes>")
        ));
    }
}
