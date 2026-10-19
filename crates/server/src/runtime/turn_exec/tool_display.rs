use std::collections::HashMap;
use std::path::PathBuf;

use devo_core::tools::tool_spec::ToolPreparationFeedback;
use devo_protocol::native::item::{ExecOrigin, ExecutionMode, Item, PlanEntry, ToolSource};

use super::types::{PendingToolCall, ToolDisplayKind, ToolStartItem};

pub(super) fn is_unified_exec_tool(name: &str) -> bool {
    matches!(name, "exec_command" | "write_stdin")
}

pub(super) fn is_file_change_tool(name: &str) -> bool {
    matches!(name, "apply_patch" | "write" | "edit")
}

pub(super) fn is_plan_tool(name: &str) -> bool {
    matches!(name, "update_plan")
}

fn tool_start_native_item(
    tool_call_id: &str,
    tool_name: &str,
    command: &str,
    input: &serde_json::Value,
    display_kind: ToolDisplayKind,
    preparation_feedback: ToolPreparationFeedback,
) -> Item {
    if preparation_feedback == ToolPreparationFeedback::LiveOnly {
        return Item::ToolCall {
            call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            source: ToolSource::Builtin,
            server_name: None,
            input: Some(input.clone()),
        };
    }
    if is_file_change_tool(tool_name) {
        // Live row needs path/edits in ToolCall args. FileChange items with empty
        // `changes` hide the running editor call from the TUI; emit ToolCall and
        // let completion produce FileChange when diffs exist.
        return Item::ToolCall {
            call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            source: ToolSource::Builtin,
            server_name: None,
            input: Some(input.clone()),
        };
    }
    if display_kind.is_command_execution() {
        return Item::CommandExecution {
            call_id: tool_call_id.to_string(),
            command: command.to_string(),
            argv: None,
            cwd: PathBuf::new(),
            input: Some(input.clone()),
            output: None,
            exit_code: None,
            execution_handle: None,
            is_error: false,
            execution_mode: ExecutionMode::Foreground,
            origin: ExecOrigin::AgentTool,
            sandbox: None,
        };
    }
    if is_plan_tool(tool_name) {
        return Item::Plan {
            call_id: Some(tool_call_id.to_string()),
            entries: Vec::<PlanEntry>::new(),
        };
    }
    Item::ToolCall {
        call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        source: ToolSource::Builtin,
        server_name: None,
        input: Some(input.clone()),
    }
}

pub(super) fn tool_start_item_from_input(
    tool_call_id: &str,
    tool_name: &str,
    command: &str,
    input: &serde_json::Value,
    display_kind: ToolDisplayKind,
    preparation_feedback: ToolPreparationFeedback,
) -> ToolStartItem {
    ToolStartItem {
        native_item: tool_start_native_item(
            tool_call_id,
            tool_name,
            command,
            input,
            display_kind,
            preparation_feedback,
        ),
    }
}

pub(super) fn tool_start_item_from_result(
    tool_call_id: &str,
    tool_name: &str,
    command: &str,
    input: &serde_json::Value,
    display_kind: ToolDisplayKind,
    preparation_feedback: ToolPreparationFeedback,
    _summary: &str,
) -> ToolStartItem {
    ToolStartItem {
        native_item: tool_start_native_item(
            tool_call_id,
            tool_name,
            command,
            input,
            display_kind,
            preparation_feedback,
        ),
    }
}

pub(super) fn command_display_from_input(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "exec_command" => input
            .get("cmd")
            .or_else(|| input.get("command"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        "write_stdin" => {
            let process_id = input
                .get("process_id")
                .or_else(|| input.get("session_id"))
                .and_then(serde_json::Value::as_i64)
                .map(|id| id.to_string())
                .unwrap_or_else(|| "?".to_string());
            let chars = input
                .get("chars")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if chars.is_empty() {
                format!("poll process {process_id}")
            } else {
                format!("write_stdin process {process_id}")
            }
        }
        "read" => {
            let path = input
                .get("filePath")
                .or_else(|| input.get("path"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            format!("read {path}")
        }
        "find" | "glob" => {
            let pattern = input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let command_name = if tool_name == "find" { "find" } else { "glob" };
            if path.is_empty() {
                format!("{command_name} {pattern}")
            } else {
                format!("{command_name} {pattern} in {path}")
            }
        }
        "grep" => {
            let pattern = input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if path.is_empty() {
                format!("grep {pattern}")
            } else {
                format!("grep {pattern} in {path}")
            }
        }
        _ => String::new(),
    }
}

pub(super) fn command_execution_item_id_for_progress(
    pending_tool_calls: &HashMap<String, PendingToolCall>,
    tool_use_id: &str,
) -> Option<devo_protocol::native::ids::ItemId> {
    pending_tool_calls
        .get(tool_use_id)
        .and_then(|pending| pending.item_id)
}

const AGENT_COORDINATION_TOOL_NAMES: &[&str] = &[
    "spawn_agent",
    "send_message",
    "await_task",
    "list_tasks",
    "cancel_task",
    "wait_agent",
    "list_agents",
    "close_agent",
];

pub(super) fn without_agent_coordination_tools(
    registry: &devo_core::tools::ToolRegistry,
) -> devo_core::tools::ToolRegistry {
    let names = registry
        .tool_definitions()
        .into_iter()
        .map(|tool| tool.name)
        .filter(|name| {
            !AGENT_COORDINATION_TOOL_NAMES
                .iter()
                .any(|hidden_name| *hidden_name == name)
        })
        .collect::<Vec<_>>();
    let names = names.iter().map(String::as_str).collect::<Vec<_>>();
    registry.restricted_to_specs(&names)
}

#[cfg(test)]
mod tests {
    use devo_core::{ToolCallItem, TurnItem};
    use devo_protocol::SessionHistoryEntry;
    use devo_protocol::native::item::Item;
    use pretty_assertions::assert_eq;

    use super::command_display_from_input;
    use crate::projection::history_entry_from_turn_item;

    #[test]
    fn history_entry_preserves_native_tool_call_input() {
        let input = serde_json::json!({
            "filePath": "crates/server/src/projection.rs",
            "offset": 20,
            "limit": 10
        });
        let projected = history_entry_from_turn_item(&TurnItem::ToolCall(ToolCallItem {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            input: input.clone(),
        }))
        .expect("history entry");
        assert_eq!(
            projected,
            SessionHistoryEntry::item(Item::ToolCall {
                call_id: "call-1".to_string(),
                tool_name: "read".to_string(),
                source: devo_protocol::native::item::ToolSource::Builtin,
                server_name: None,
                input: Some(input),
            })
        );
    }

    #[test]
    fn write_stdin_display_prefers_process_id_and_reads_legacy_session_id() {
        assert_eq!(
            command_display_from_input(
                "write_stdin",
                &serde_json::json!({
                    "process_id": 42,
                    "session_id": 99,
                    "chars": "hello"
                }),
            ),
            "write_stdin process 42"
        );
        assert_eq!(
            command_display_from_input(
                "write_stdin",
                &serde_json::json!({ "session_id": 99, "chars": "" }),
            ),
            "poll process 99"
        );
    }
}
