use super::tool_display::*;
use super::types::*;
use crate::*;
use devo_core::ItemId;
use devo_core::tools::tool_spec::ToolPreparationFeedback;
use pretty_assertions::assert_eq;
use std::collections::HashMap;

#[test]
fn command_progress_uses_command_execution_item_id() {
    let command_item_id = ItemId::new();
    let tool_item_id = ItemId::new();
    let mut pending_tool_calls = HashMap::new();
    pending_tool_calls.insert(
        "exec".to_string(),
        PendingToolCall {
            item_id: Some(command_item_id),
            item_seq: Some(1),
            input: serde_json::json!({}),
            display_kind: ToolDisplayKind::CommandExecution,
            command: "cargo test".to_string(),
        },
    );
    pending_tool_calls.insert(
        "read".to_string(),
        PendingToolCall {
            item_id: Some(tool_item_id),
            item_seq: Some(2),
            input: serde_json::json!({}),
            display_kind: ToolDisplayKind::Generic,
            command: String::new(),
        },
    );

    assert_eq!(
        command_execution_item_id_for_progress(&pending_tool_calls, "exec"),
        Some(command_item_id)
    );
    assert_eq!(
        command_execution_item_id_for_progress(&pending_tool_calls, "read"),
        None
    );
    assert_eq!(
        command_execution_item_id_for_progress(&pending_tool_calls, "missing"),
        None
    );
}

#[test]
fn file_change_tool_detection_matches_apply_patch_and_write() {
    assert!(is_file_change_tool("apply_patch"));
    assert!(is_file_change_tool("write"));
    assert!(!is_file_change_tool("read"));
}

#[test]
fn plan_tool_detection_matches_update_plan() {
    assert!(is_plan_tool("update_plan"));
    assert!(!is_plan_tool("read"));
}

#[test]
fn read_tool_start_item_contains_live_read_action() {
    let input = serde_json::json!({
        "path": "crates/tui/src/mod.rs"
    });
    let start_item = tool_start_item_from_input(
        "call-1",
        "read",
        "read crates/tui/src/mod.rs",
        &input,
        ToolDisplayKind::Generic,
        ToolPreparationFeedback::None,
    );

    let payload: ToolCallPayload =
        serde_json::from_value(start_item.payload).expect("tool call payload");

    assert_eq!(start_item.item_kind, ItemKind::ToolCall);
    assert_eq!(
        payload,
        ToolCallPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            parameters: input,
            command_actions: vec![devo_protocol::parse_command::ParsedCommand::Read {
                cmd: "read crates/tui/src/mod.rs".to_string(),
                name: "mod.rs".to_string(),
                path: std::path::PathBuf::from("crates/tui/src/mod.rs"),
            }],
        }
    );
}

#[test]
fn grep_tool_start_item_contains_live_search_action() {
    let input = serde_json::json!({
        "pattern": "ToolUseStart",
        "path": "crates/server/src"
    });
    let start_item = tool_start_item_from_input(
        "call-1",
        "grep",
        "grep ToolUseStart in crates/server/src",
        &input,
        ToolDisplayKind::Generic,
        ToolPreparationFeedback::None,
    );

    let payload: ToolCallPayload =
        serde_json::from_value(start_item.payload).expect("tool call payload");

    assert_eq!(start_item.item_kind, ItemKind::ToolCall);
    assert_eq!(
        payload,
        ToolCallPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "grep".to_string(),
            parameters: input,
            command_actions: vec![devo_protocol::parse_command::ParsedCommand::Search {
                cmd: "grep ToolUseStart in crates/server/src".to_string(),
                query: Some("ToolUseStart".to_string()),
                path: Some("crates/server/src".to_string()),
            }],
        }
    );
}

#[test]
fn code_search_tool_start_item_contains_live_search_action() {
    let input = serde_json::json!({
        "operation": "search",
        "query": "live tool feedback",
        "path": "crates"
    });
    let start_item = tool_start_item_from_input(
        "call-1",
        "code_search",
        "code_search live tool feedback in crates",
        &input,
        ToolDisplayKind::Generic,
        ToolPreparationFeedback::None,
    );

    let payload: ToolCallPayload =
        serde_json::from_value(start_item.payload).expect("tool call payload");

    assert_eq!(start_item.item_kind, ItemKind::ToolCall);
    assert_eq!(
        payload,
        ToolCallPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "code_search".to_string(),
            parameters: input,
            command_actions: vec![devo_protocol::parse_command::ParsedCommand::Search {
                cmd: "code_search live tool feedback in crates".to_string(),
                query: Some("live tool feedback".to_string()),
                path: Some("crates".to_string()),
            }],
        }
    );
}

#[test]
fn exec_tool_start_item_uses_command_execution_payload() {
    let input = serde_json::json!({
        "cmd": "cargo test -p devo-server"
    });
    let start_item = tool_start_item_from_input(
        "call-1",
        "exec_command",
        "cargo test -p devo-server",
        &input,
        ToolDisplayKind::CommandExecution,
        ToolPreparationFeedback::None,
    );

    let payload: CommandExecutionPayload =
        serde_json::from_value(start_item.payload).expect("command execution payload");

    assert_eq!(start_item.item_kind, ItemKind::CommandExecution);
    assert_eq!(
        payload,
        CommandExecutionPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "exec_command".to_string(),
            command: "cargo test -p devo-server".to_string(),
            input: Some(input),
            source: devo_protocol::protocol::ExecCommandSource::Agent,
            command_actions: Vec::new(),
            output: None,
            is_error: false,
        }
    );
}

#[test]
fn user_shell_exec_input_uses_pty_backed_exec_command() {
    let cwd = std::path::PathBuf::from("workspace");

    let input = user_shell_exec_input("pwd", cwd.clone());

    assert_eq!(
        input,
        serde_json::json!({
            "cmd": "pwd",
            "workdir": cwd,
            "login": true,
            "tty": true,
        })
    );
}

#[test]
fn user_shell_command_payload_uses_user_shell_source() {
    let output = serde_json::json!({ "output": "done" });

    let input = user_shell_exec_input("pwd", std::path::PathBuf::from("workspace"));
    let payload = user_shell_command_payload(
        "call-1",
        "pwd",
        input.clone(),
        Vec::new(),
        Some(output.clone()),
        false,
    );

    assert_eq!(
        payload,
        CommandExecutionPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "exec_command".to_string(),
            command: "pwd".to_string(),
            input: Some(input),
            source: devo_protocol::protocol::ExecCommandSource::UserShell,
            command_actions: Vec::new(),
            output: Some(output),
            is_error: false,
        }
    );
}

#[test]
fn live_only_apply_patch_start_item_stays_tool_call() {
    let input = serde_json::json!({
        "patch": "*** Begin Patch\n*** End Patch"
    });
    let start_item = tool_start_item_from_input(
        "call-1",
        "apply_patch",
        "apply_patch",
        &input,
        ToolDisplayKind::Generic,
        ToolPreparationFeedback::LiveOnly,
    );

    let payload: ToolCallPayload =
        serde_json::from_value(start_item.payload).expect("tool call payload");

    assert_eq!(start_item.item_kind, ItemKind::ToolCall);
    assert_eq!(
        payload,
        ToolCallPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "apply_patch".to_string(),
            parameters: input,
            command_actions: Vec::new(),
        }
    );
}

#[test]
fn command_actions_from_read_tool_input_builds_read_action() {
    let actions = command_actions_from_tool_input(
        "read",
        "read crates/tui/src/mod.rs",
        &serde_json::json!({
            "filePath": "crates/tui/src/mod.rs"
        }),
    );
    assert_eq!(
        actions,
        vec![devo_protocol::parse_command::ParsedCommand::Read {
            cmd: "read crates/tui/src/mod.rs".to_string(),
            name: "mod.rs".to_string(),
            path: std::path::PathBuf::from("crates/tui/src/mod.rs"),
        }]
    );
}

#[test]
fn command_actions_from_read_tool_input_without_path_is_empty() {
    let actions =
        command_actions_from_tool_input("read", "read", &serde_json::json!({ "limit": 10 }));
    assert_eq!(actions, Vec::new());
}

#[test]
fn command_actions_from_read_tool_result_summary_recovers_final_path() {
    let actions = command_actions_from_tool_result(
        "read",
        "read ",
        &serde_json::json!({}),
        "read: crates/tui/src/mod.rs",
    );
    assert_eq!(
        actions,
        vec![devo_protocol::parse_command::ParsedCommand::Read {
            cmd: "read crates/tui/src/mod.rs".to_string(),
            name: "mod.rs".to_string(),
            path: std::path::PathBuf::from("crates/tui/src/mod.rs"),
        }]
    );
}

#[test]
fn command_actions_from_grep_tool_input_builds_search_action() {
    let actions = command_actions_from_tool_input(
        "grep",
        "grep rebuild_restored_session in crates/tui/src",
        &serde_json::json!({
            "pattern": "rebuild_restored_session",
            "path": "crates/tui/src"
        }),
    );
    assert_eq!(actions.len(), 1);
    assert!(matches!(
        &actions[0],
        devo_protocol::parse_command::ParsedCommand::Search { query, path, .. }
        if query.as_deref() == Some("rebuild_restored_session")
            && path.as_deref() == Some("crates/tui/src")
    ));
}

#[test]
fn command_actions_from_glob_tool_input_include_pattern_and_path() {
    let actions = command_actions_from_tool_input(
        "glob",
        "glob **/Cargo.toml in crates",
        &serde_json::json!({
            "pattern": "**/Cargo.toml",
            "path": "crates"
        }),
    );
    assert_eq!(
        actions,
        vec![devo_protocol::parse_command::ParsedCommand::ListFiles {
            cmd: "glob **/Cargo.toml in crates".to_string(),
            path: Some("**/Cargo.toml in crates".to_string()),
        }]
    );
}

#[test]
fn command_actions_from_find_tool_input_include_pattern_and_path() {
    let actions = command_actions_from_tool_input(
        "find",
        "find **/Cargo.toml in crates",
        &serde_json::json!({
            "pattern": "**/Cargo.toml",
            "path": "crates"
        }),
    );
    assert_eq!(
        actions,
        vec![devo_protocol::parse_command::ParsedCommand::ListFiles {
            cmd: "find **/Cargo.toml in crates".to_string(),
            path: Some("**/Cargo.toml in crates".to_string()),
        }]
    );
}
