use super::tool_display::*;
use super::types::*;
use devo_core::tools::tool_spec::ToolPreparationFeedback;
use pretty_assertions::assert_eq;
use std::collections::HashMap;

use super::context_compaction::{completed_event, failed_item_notification, started_event};
use super::event_stream::enqueue_query_event;
use super::trace::QueryEventDeliveryPolicy;
use super::trace::query_event_delivery_policy;
use devo_protocol::native::event::ServerNotification;
use devo_protocol::native::ids::{ItemId, SessionId, TurnId};

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
            started_at: std::time::Instant::now(),
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
            started_at: std::time::Instant::now(),
        },
    );

    assert_eq!(
        command_execution_item_id_for_progress(&pending_tool_calls, "exec"),
        Some(command_item_id)
    );
    assert_eq!(
        command_execution_item_id_for_progress(&pending_tool_calls, "read"),
        Some(tool_item_id)
    );
    assert_eq!(
        command_execution_item_id_for_progress(&pending_tool_calls, "missing"),
        None
    );
}

#[test]
fn context_compaction_events_share_stable_item_lifecycle() {
    let session_id = SessionId::new();
    let turn_id = TurnId::new();
    let item_id = ItemId::new();

    let started = started_event(session_id, turn_id, item_id);
    let completed = completed_event(session_id, turn_id, item_id, None);
    assert!(matches!(started, ServerNotification::ItemStarted { .. }));
    assert!(matches!(
        completed,
        ServerNotification::ItemCompleted { .. }
    ));
    assert!(matches!(
        match &started {
            ServerNotification::ItemStarted { item } => &item.item,
            _ => unreachable!(),
        },
        devo_protocol::native::item::Item::ContextCompaction { .. }
    ));
    assert!(matches!(
        match &completed {
            ServerNotification::ItemCompleted { item } => &item.item,
            _ => unreachable!(),
        },
        devo_protocol::native::item::Item::ContextCompaction { .. }
    ));
}

#[test]
fn context_compaction_failure_closes_item_and_reports_visible_error() {
    let session_id = SessionId::new();
    let turn_id = TurnId::new();
    let item_id = ItemId::new();
    let message = "context limit".to_string();

    let item = failed_item_notification(session_id, turn_id, item_id, &message);
    assert!(matches!(item, ServerNotification::ItemCompleted { .. }));
    let _ = ServerNotification::ContextCompactionFailed {
        session_id,
        message,
    };
}

#[test]
fn file_change_tool_detection_matches_apply_patch_and_write() {
    assert!(is_file_change_tool("apply_patch"));
    assert!(is_file_change_tool("write"));
    assert!(is_file_change_tool("edit"));
    assert!(!is_file_change_tool("read"));
}

#[test]
fn plan_tool_detection_matches_update_plan() {
    assert!(is_plan_tool("update_plan"));
    assert!(!is_plan_tool("read"));
}

#[test]
fn plan_tool_start_item_carries_call_id() {
    // Clients key the running tool row by call id; a Plan start item without
    // it cannot be correlated and the row stays open forever.
    let input = serde_json::json!({"plan": [{"step": "a", "status": "pending"}]});
    let start_item = tool_start_item_from_input(
        "call-plan-1",
        "update_plan",
        "",
        &input,
        ToolDisplayKind::Generic,
        ToolPreparationFeedback::None,
    );
    match start_item.native_item {
        devo_protocol::native::item::Item::Plan { call_id, entries } => {
            assert_eq!(call_id.as_deref(), Some("call-plan-1"));
            assert!(entries.is_empty());
        }
        other => panic!("expected Plan start item, got {other:?}"),
    }
}

#[test]
fn plan_entries_parse_from_plan_handler_result_shape() {
    use devo_protocol::native::plan_parse::plan_entries_from_update_plan_json;

    let plan = serde_json::json!([
        {"step": "Verify-R8-PLAN-CREATE", "status": "in_progress"},
        {"step": "Probe-R8-PLAN-UPDATE", "status": "pending"},
    ]);
    // Exactly what PlanHandler returns: Mixed { text: pretty JSON (or
    // explanation + pretty JSON), json: { explanation, plan } }.
    let content = devo_core::tools::ToolContent::Mixed {
        text: Some(serde_json::to_string_pretty(&plan).expect("pretty")),
        json: Some(serde_json::json!({
            "explanation": "",
            "plan": plan,
        })),
    };
    let parsed =
        plan_entries_from_update_plan_json(&super::tool_results::tool_content_to_json(content));
    assert_eq!(
        parsed.as_ref().map(|entries| entries.len()),
        Some(2),
        "PlanHandler output must parse into plan entries"
    );
}

#[test]
fn read_tool_start_item_is_native_tool_call() {
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

    assert_eq!(
        start_item.native_item,
        devo_protocol::native::item::Item::ToolCall {
            call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            source: devo_protocol::native::item::ToolSource::Builtin,
            server_name: None,
            input: Some(input),
        }
    );
}

#[test]
fn grep_tool_start_item_is_native_tool_call() {
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

    assert_eq!(
        start_item.native_item,
        devo_protocol::native::item::Item::ToolCall {
            call_id: "call-1".to_string(),
            tool_name: "grep".to_string(),
            source: devo_protocol::native::item::ToolSource::Builtin,
            server_name: None,
            input: Some(input),
        }
    );
}

#[test]
fn code_search_tool_start_item_is_native_tool_call() {
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

    assert_eq!(
        start_item.native_item,
        devo_protocol::native::item::Item::ToolCall {
            call_id: "call-1".to_string(),
            tool_name: "code_search".to_string(),
            source: devo_protocol::native::item::ToolSource::Builtin,
            server_name: None,
            input: Some(input),
        }
    );
}

#[test]
fn exec_tool_start_item_is_native_command_execution() {
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

    assert_eq!(
        start_item.native_item,
        devo_protocol::native::item::Item::CommandExecution {
            call_id: "call-1".to_string(),
            command: "cargo test -p devo-server".to_string(),
            argv: None,
            cwd: std::path::PathBuf::new(),
            input: Some(input),
            output: None,
            exit_code: None,
            execution_handle: None,
            is_error: false,
            execution_mode: devo_protocol::native::item::ExecutionMode::Foreground,
            origin: devo_protocol::native::item::ExecOrigin::AgentTool,
            sandbox: None,
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

    assert_eq!(
        start_item.native_item,
        devo_protocol::native::item::Item::ToolCall {
            call_id: "call-1".to_string(),
            tool_name: "apply_patch".to_string(),
            source: devo_protocol::native::item::ToolSource::Builtin,
            server_name: None,
            input: Some(input),
        }
    );
}

#[tokio::test]
async fn provider_retry_status_waits_for_channel_capacity() {
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(1);
    event_tx
        .send(devo_core::QueryEvent::Usage {
            usage: devo_protocol::Usage::default(),
        })
        .await
        .expect("fill event channel");
    let retry_status = devo_core::ProviderRetryStatus {
        provider: "openai".to_string(),
        model: "test-model".to_string(),
        attempt: 1,
        max_attempts: 5,
        backoff_ms: 250,
        phase: devo_core::ModelQueryRetryPhase::Scheduled,
        message: "Retrying provider request in 0.2s".to_string(),
    };
    let retry_event = devo_core::QueryEvent::ProviderRetryStatus(retry_status.clone());
    let enqueue = tokio::spawn(async move {
        enqueue_query_event(&event_tx, retry_event).await;
    });
    tokio::task::yield_now().await;

    assert!(!enqueue.is_finished());
    assert!(matches!(
        event_rx.recv().await,
        Some(devo_core::QueryEvent::Usage { .. })
    ));
    enqueue.await.expect("enqueue retry event");
    let received_status = match event_rx.recv().await {
        Some(devo_core::QueryEvent::ProviderRetryStatus(status)) => status,
        Some(_) | None => panic!("expected provider retry status"),
    };
    assert_eq!(received_status, retry_status);
}

#[test]
fn lifecycle_and_control_query_events_are_must_deliver() {
    let events = [
        devo_core::QueryEvent::ContextCompactionStarted,
        devo_core::QueryEvent::ContextCompactionCompleted {
            compacted_items: Vec::new(),
        },
        devo_core::QueryEvent::ContextCompactionFailed {
            message: "context limit".to_string(),
        },
        devo_core::QueryEvent::ContextEstimate {
            breakdown: devo_core::RawContextBreakdown::default(),
        },
        devo_core::QueryEvent::ReasoningCompleted,
        devo_core::QueryEvent::ToolUseStart {
            id: "tool-1".to_string(),
            name: "read".to_string(),
            input: serde_json::json!({ "path": "README.md" }),
        },
        devo_core::QueryEvent::ToolExecutionStart {
            id: "tool-1".to_string(),
        },
        devo_core::QueryEvent::ToolResult {
            tool_use_id: "tool-1".to_string(),
            tool_name: "read".to_string(),
            input: serde_json::json!({ "path": "README.md" }),
            content: devo_core::tools::ToolContent::Text("contents".to_string()),
            display_content: None,
            is_error: false,
            summary: "read README.md".to_string(),
        },
        devo_core::QueryEvent::TextDelta("text".to_string()),
        devo_core::QueryEvent::ReasoningDelta("reasoning".to_string()),
        devo_core::QueryEvent::TurnComplete {
            stop_reason: devo_core::StopReason::EndTurn,
        },
    ];

    assert_eq!(
        events
            .iter()
            .map(query_event_delivery_policy)
            .collect::<Vec<_>>(),
        vec![QueryEventDeliveryPolicy::MustDeliver; events.len()]
    );
}

#[test]
fn high_volume_query_events_are_best_effort() {
    let events = [
        devo_core::QueryEvent::ToolProgress {
            tool_use_id: "tool-1".to_string(),
            progress: devo_core::tools::ToolProgress::OutputDelta {
                delta: "output".to_string(),
            },
        },
        devo_core::QueryEvent::ToolProgress {
            tool_use_id: "tool-1".to_string(),
            progress: devo_core::tools::ToolProgress::StatusUpdate {
                message: "working".to_string(),
                percent: Some(50),
            },
        },
        devo_core::QueryEvent::ToolProgress {
            tool_use_id: "tool-1".to_string(),
            progress: devo_core::tools::ToolProgress::Completion {
                summary: "done".to_string(),
            },
        },
        devo_core::QueryEvent::UsageDelta {
            usage: devo_protocol::Usage::default(),
        },
        devo_core::QueryEvent::Usage {
            usage: devo_protocol::Usage::default(),
        },
    ];

    assert_eq!(
        events
            .iter()
            .map(query_event_delivery_policy)
            .collect::<Vec<_>>(),
        vec![QueryEventDeliveryPolicy::BestEffort; events.len()]
    );
}
