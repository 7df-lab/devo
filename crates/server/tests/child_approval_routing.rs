//! Routing of child (subagent) permission requests to controlling clients.
//!
//! A child session never has client subscribers of its own: approvals must
//! surface through the parent chain even when the parent's turn already
//! ended (spawn-and-return), because the client stays subscribed to the
//! parent session while the child keeps running in the background.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use tempfile::TempDir;
use tokio::time::timeout;

#[path = "support/subagent_lifecycle.rs"]
mod support;

use support::ScriptedProvider;
use support::StreamScript;
use support::initialize_connection;
use support::spawn_child_with;
use support::start_parent_session;
use support::start_turn;
use support::wait_for_session_notification;

fn ipython_tool_call(code: &str) -> StreamScript {
    let input = serde_json::json!({
        "code": code,
        // This fixture simulates unadvertised fields from an adversarial
        // provider; the permission gate must still prompt before execution.
        "sandbox_permissions": "with_additional_permissions",
        "additional_permissions": { "network": { "enabled": true } }
    });
    let tool_call_id = "ipython-approval-call".to_string();
    StreamScript::Events(vec![
        StreamEvent::ToolCallStart {
            index: 0,
            id: tool_call_id.clone(),
            name: "ipython".to_string(),
            input: input.clone(),
        },
        StreamEvent::MessageDone {
            response: ModelResponse {
                id: "ipython-approval-response".into(),
                content: vec![ResponseContent::ToolUse {
                    id: tool_call_id,
                    name: "ipython".to_string(),
                    input,
                }],
                stop_reason: Some(StopReason::ToolUse),
                usage: Usage::default(),
                metadata: ResponseMetadata::default(),
            },
        },
    ])
}

#[tokio::test]
async fn child_approval_routes_to_parent_subscriber_while_parent_idle() -> Result<()> {
    // Regression: the parent turn completed right after spawning (no
    // await_task held it open), so the parent held no live connection
    // attribution when the child's gated tool ran. The approval then fell
    // back to the child's own session, matched no controller (clients only
    // subscribe to the parent), and failed as an instant permission error —
    // the child could never run any approval-gated tool at all.
    let data_root = TempDir::new()?;
    let workspace_root = data_root.path().join("workspace");
    std::fs::create_dir_all(&workspace_root)?;
    let marker = workspace_root.join("child_marker.txt");
    let marker_path = serde_json::to_string(marker.to_string_lossy().as_ref())?;
    let code = format!("from pathlib import Path; Path({marker_path}).write_text('called')");
    let provider = Arc::new(ScriptedProvider::new([
        ScriptedProvider::completed("parent done"),
        ipython_tool_call(&code),
        ScriptedProvider::completed("child done"),
    ]));
    let runtime = devo_server::test_support::TestRuntime::new(provider as _)
        .disabled_skills()
        .db_file("child_approval_routing.db")
        .runtime(data_root.path());
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let parent_session_id = start_parent_session(&runtime, connection_id, &workspace_root).await?;

    // The TUI subscribes with an explicit Session selector (subscription/
    // create); only these selectors count as "controlling" for approvals.
    let subscription_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 20,
                "method": "subscription/create",
                "params": {
                    "selectors": [{ "kind": "session", "sessionId": parent_session_id.to_string() }],
                    "includeSnapshot": false
                }
            }),
        )
        .await
        .context("subscription/create response")?;
    assert!(
        subscription_response.get("error").is_none(),
        "subscription/create failed: {subscription_response}"
    );

    // Parent turn starts and completes immediately (script 1 is plain text):
    // the connection keeps its Session selector subscription, exactly like a
    // TUI client that stays open while the agent idles.
    start_turn(&runtime, connection_id, parent_session_id, "spawn a worker").await?;
    wait_for_session_notification(&mut notifications_rx, "turn/completed", parent_session_id)
        .await
        .map(|_| ())?;
    let mut parent_idle = false;
    for _ in 0..200 {
        let response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 30,
                    "method": "session/read",
                    "params": { "sessionId": parent_session_id }
                }),
            )
            .await
            .context("session/read response")?;
        if response["result"]["session"]["status"] == serde_json::json!("idle") {
            parent_idle = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        parent_idle,
        "parent must be idle before the child requests approval"
    );

    let child = spawn_child_with(
        &runtime,
        connection_id,
        parent_session_id,
        "write the marker file",
        Some("all"),
    )
    .await?;
    wait_for_session_notification(
        &mut notifications_rx,
        "turn/started",
        child.child_session_id,
    )
    .await
    .map(|_| ())?;

    // The child's explicit ipython permission request must reach the
    // connection subscribed to the PARENT even though the parent is idle.
    let approval = timeout(Duration::from_secs(10), async {
        loop {
            let value = notifications_rx
                .recv()
                .await
                .context("notification channel closed before approval request")?;
            if value.get("method").and_then(|method| method.as_str())
                == Some("approval/permission/request")
            {
                return Ok::<_, anyhow::Error>(value);
            }
        }
    })
    .await
    .context("child approval must be routed to the parent's subscriber")?
    .expect("approval request payload");
    assert!(approval.get("id").is_some(), "approval request id");
    assert!(
        !marker.exists(),
        "the gated command must wait for the approval instead of executing"
    );

    // Cleanup: interrupt the parent, which cascades to the parked child.
    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 40,
                "method": "session/interrupt",
                "params": {
                    "scope": { "scope": "session", "sessionId": parent_session_id }
                }
            }),
        )
        .await;
    Ok(())
}
