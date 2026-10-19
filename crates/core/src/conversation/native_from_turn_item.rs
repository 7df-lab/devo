//! Best-effort TurnItem → Native Item for in-memory history rebuild.
//!
//! Live emit paths should pass the already-built Native [`Item`] directly.
//! This converter exists for resume/replay when only legacy [`TurnItem`]
//! payloads are available (e.g. packed item lines before v2 envelopes).

use std::path::PathBuf;

use devo_protocol::native::item::{
    CompactionTrigger, ContextUsage, ExecOrigin, ExecutionMode, Item, ToolSource, UserInput,
    UserMessageEntry,
};

use super::{CommandExecutionItem, TextItem, ToolCallItem, ToolResultItem, TurnItem};

/// Projects a visible TurnItem into a Native Item for session history.
///
/// Returns `None` for internal-only / non-display payloads (approvals,
/// tool progress, empty assistant text) and for `TurnSummary` (migrate-only
/// packed items — live history uses Native `Turn` status, not a TurnItem).
pub fn native_item_from_turn_item(item: &TurnItem) -> Option<Item> {
    match item {
        TurnItem::UserMessage(text) => Some(Item::UserMessage {
            client_user_message_id: None,
            content: user_message_content(text),
            entry: UserMessageEntry::TurnStart,
        }),
        TurnItem::SteerInput(text) => Some(Item::UserMessage {
            client_user_message_id: None,
            content: user_message_content(text),
            entry: UserMessageEntry::Steer,
        }),
        TurnItem::AgentMessage(TextItem { text, .. }) if text.trim().is_empty() => None,
        TurnItem::AgentMessage(TextItem { text, .. })
        | TurnItem::HookPrompt(TextItem { text, .. }) => {
            Some(Item::AssistantMessage { text: text.clone() })
        }
        TurnItem::WebSearch(TextItem { text, .. }) => Some(Item::HostedToolCall {
            call_id: String::new(),
            tool_name: "web_search".into(),
            input: None,
            output: Some(serde_json::Value::String(text.clone())),
        }),
        TurnItem::ImageGeneration(TextItem { text, .. }) => Some(Item::HostedToolCall {
            call_id: String::new(),
            tool_name: "image_generation".into(),
            input: None,
            output: Some(serde_json::Value::String(text.clone())),
        }),
        TurnItem::Plan(TextItem { text, .. }) => Some(Item::Plan {
            call_id: None,
            entries: devo_protocol::native::plan_parse::plan_entries_from_plan_text_or_single(
                text.clone(),
            ),
        }),
        TurnItem::ContextCompaction(TextItem { text, .. }) => Some(Item::ContextCompaction {
            trigger: CompactionTrigger::AutoThreshold,
            before: ContextUsage {
                measured: false,
                ..ContextUsage::default()
            },
            after: None,
            summary: Some(text.clone()),
        }),
        TurnItem::Reasoning(TextItem { text, .. }) => Some(Item::Reasoning {
            text: text.clone(),
            provider_payload_ref: None,
        }),
        TurnItem::ToolCall(ToolCallItem {
            tool_call_id,
            tool_name,
            input,
        }) => Some(Item::ToolCall {
            call_id: tool_call_id.clone(),
            tool_name: tool_name.clone(),
            source: ToolSource::Builtin,
            server_name: None,
            input: Some(input.clone()),
        }),
        TurnItem::ToolResult(ToolResultItem {
            tool_call_id,
            output,
            display_content,
            is_error,
            ..
        }) => Some(Item::ToolResult {
            call_id: tool_call_id.clone(),
            output: output.clone(),
            display_content: display_content.clone(),
            is_error: *is_error,
            truncated: false,
        }),
        TurnItem::CommandExecution(CommandExecutionItem {
            tool_call_id,
            command,
            input,
            output,
            is_error,
            ..
        }) => Some(Item::CommandExecution {
            call_id: tool_call_id.clone(),
            command: command.clone(),
            argv: None,
            cwd: PathBuf::new(),
            input: Some(input.clone()),
            output: Some(output.clone()),
            exit_code: None,
            execution_handle: None,
            is_error: *is_error,
            execution_mode: ExecutionMode::Foreground,
            origin: ExecOrigin::AgentTool,
            sandbox: None,
        }),
        TurnItem::ToolProgress(_)
        | TurnItem::ApprovalRequest(_)
        | TurnItem::ApprovalDecision(_)
        | TurnItem::TurnSummary(_) => None,
    }
}

fn user_message_content(item: &TextItem) -> Vec<UserInput> {
    let mut content = Vec::with_capacity(1 + item.local_image_paths.len());
    content.push(UserInput::Text {
        text: item.text.clone(),
    });
    for path in &item.local_image_paths {
        content.push(UserInput::LocalImage {
            path: path.clone(),
            detail: None,
        });
    }
    content
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::conversation::TextItem;

    /// Trace: L2-DES-APP-008
    /// Verifies: empty assistant TurnItems are omitted from Native history.
    #[test]
    fn omits_empty_agent_message() {
        assert_eq!(
            native_item_from_turn_item(&TurnItem::AgentMessage(TextItem::text(String::new()))),
            None
        );
        assert_eq!(
            native_item_from_turn_item(&TurnItem::AgentMessage(TextItem::text("  \n\t"))),
            None
        );
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: user TurnItems become Native UserMessage items.
    #[test]
    fn user_message_projects_to_native() {
        let item = native_item_from_turn_item(&TurnItem::UserMessage(TextItem::text("hello")))
            .expect("item");
        assert_eq!(
            item,
            Item::UserMessage {
                client_user_message_id: None,
                content: vec![UserInput::Text {
                    text: "hello".into(),
                }],
                entry: UserMessageEntry::TurnStart,
            }
        );
    }
}
