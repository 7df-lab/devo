//! Rebuild model prompt [`Message`]s from Native [`Item`] journal rows.
//!
//! Replaces the TurnItem-based `apply_prompt_turn_item` path for first-party
//! resume / compaction snapshot rebuild.

use std::collections::HashMap;
use std::path::PathBuf;

use devo_core::ContentBlock;
use devo_core::Message;
use devo_core::Role;
use devo_protocol::SessionHistoryEntry;
use devo_protocol::native::item::Item;
use devo_protocol::native::item::UserInput;

use crate::persisted_native_item::history_entry_from_native_item;
use crate::persisted_native_item::prompt_visible_native_item;

pub(crate) fn apply_native_item(
    messages: &mut Vec<Message>,
    history_items: &mut Vec<SessionHistoryEntry>,
    tool_names_by_id: &mut HashMap<String, String>,
    item: Item,
) {
    remember_tool_name(tool_names_by_id, &item);
    if let Some(history_item) = history_entry_from_native_item(&item) {
        history_items.push(history_item);
    }
    if prompt_visible_native_item(&item) {
        apply_prompt_native_item(messages, tool_names_by_id, item);
    }
}

pub(crate) fn apply_prompt_native_item(
    messages: &mut Vec<Message>,
    tool_names_by_id: &mut HashMap<String, String>,
    item: Item,
) {
    remember_tool_name(tool_names_by_id, &item);
    match item {
        Item::UserMessage { content, .. } => {
            let (text, images) = split_user_content(&content);
            messages.push(user_message_with_local_images(text, &images));
        }
        Item::AssistantMessage { text, .. } if text.trim().is_empty() => {}
        Item::AssistantMessage { text, .. }
        | Item::ContextCompaction {
            summary: Some(text),
            ..
        } => {
            messages.push(Message::assistant_text(text));
        }
        Item::ContextCompaction { summary: None, .. } => {
            messages.push(Message::assistant_text(String::new()));
        }
        Item::Plan { entries, .. } => {
            let text = entries
                .iter()
                .map(|entry| entry.step.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            messages.push(Message::assistant_text(text));
        }
        Item::HostedToolCall {
            tool_name, output, ..
        } => {
            let text = output
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let _ = tool_name;
            messages.push(Message::assistant_text(text));
        }
        Item::ToolCall {
            call_id,
            tool_name,
            input,
            ..
        } => {
            let input = input.unwrap_or(serde_json::Value::Null);
            push_tool_use(messages, call_id, tool_name, input);
        }
        Item::ToolResult {
            call_id,
            output,
            is_error,
            ..
        } => {
            let content = match output {
                serde_json::Value::String(text) => text,
                other => other.to_string(),
            };
            push_tool_result(messages, call_id, content, is_error);
        }
        Item::CommandExecution {
            call_id,
            input,
            output,
            is_error,
            ..
        } => {
            let tool_name = tool_names_by_id
                .get(&call_id)
                .cloned()
                .unwrap_or_else(|| "exec_command".into());
            let input = input.unwrap_or(serde_json::Value::Null);
            push_tool_use(messages, call_id.clone(), tool_name, input);
            let content = match output.unwrap_or(serde_json::Value::Null) {
                serde_json::Value::String(text) => text,
                other => other.to_string(),
            };
            messages.push(Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: call_id,
                    content,
                    is_error,
                }],
            });
        }
        Item::Reasoning { text, .. } => match messages.last_mut() {
            Some(message) if message.role == Role::Assistant => {
                message.content.push(ContentBlock::Reasoning { text });
            }
            _ => {
                messages.push(Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Reasoning { text }],
                });
            }
        },
        Item::Approval { .. }
        | Item::FileChange { .. }
        | Item::UserInputRequest { .. }
        | Item::SubAgent { .. }
        | Item::BackgroundTask { .. }
        | Item::GoalProgress { .. }
        | Item::Refinement { .. }
        | Item::Warning { .. }
        | Item::BranchSummary { .. } => {}
    }
}

fn remember_tool_name(tool_names_by_id: &mut HashMap<String, String>, item: &Item) {
    match item {
        Item::ToolCall {
            call_id, tool_name, ..
        } => {
            tool_names_by_id.insert(call_id.clone(), tool_name.clone());
        }
        Item::CommandExecution { call_id, .. } => {
            tool_names_by_id
                .entry(call_id.clone())
                .or_insert_with(|| "exec_command".into());
        }
        _ => {}
    }
}

fn split_user_content(content: &[UserInput]) -> (String, Vec<PathBuf>) {
    let mut text = String::new();
    let mut images = Vec::new();
    for part in content {
        match part {
            UserInput::Text { text: part_text } => {
                if !text.is_empty() && !part_text.is_empty() {
                    text.push('\n');
                }
                text.push_str(part_text);
            }
            UserInput::LocalImage { path, .. } => images.push(path.clone()),
            UserInput::Image { .. }
            | UserInput::Audio { .. }
            | UserInput::Skill { .. }
            | UserInput::Mention { .. } => {}
        }
    }
    (text, images)
}

fn push_tool_use(
    messages: &mut Vec<Message>,
    call_id: String,
    tool_name: String,
    input: serde_json::Value,
) {
    match messages.last_mut() {
        Some(message) if message.role == Role::Assistant => {
            message.content.push(ContentBlock::ToolUse {
                id: call_id,
                name: tool_name,
                input,
            });
        }
        _ => {
            messages.push(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: call_id,
                    name: tool_name,
                    input,
                }],
            });
        }
    }
}

fn push_tool_result(messages: &mut Vec<Message>, call_id: String, content: String, is_error: bool) {
    match messages.last_mut() {
        Some(message)
            if message.role == Role::User
                && message
                    .content
                    .iter()
                    .all(|block| matches!(block, ContentBlock::ToolResult { .. })) =>
        {
            message.content.push(ContentBlock::ToolResult {
                tool_use_id: call_id,
                content,
                is_error,
            });
        }
        _ => {
            messages.push(Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: call_id,
                    content,
                    is_error,
                }],
            });
        }
    }
}

fn user_message_with_local_images(text: String, local_image_paths: &[PathBuf]) -> Message {
    if local_image_paths.is_empty() {
        return Message::user(text);
    }
    let mut content = Vec::new();
    if !text.trim().is_empty() {
        content.push(ContentBlock::Text { text });
    }
    for path in local_image_paths {
        match crate::session_context::read_local_image_part(path) {
            Ok(part) => content.push(ContentBlock::Image {
                mime_type: part.mime_type,
                data_base64: part.data_base64,
            }),
            Err(error) => {
                let label = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("image");
                content.push(ContentBlock::Text {
                    text: format!("[image:{label} (unreadable: {error})]"),
                });
            }
        }
    }
    if content.is_empty() {
        Message::user(String::new())
    } else {
        Message {
            role: Role::User,
            content,
        }
    }
}
