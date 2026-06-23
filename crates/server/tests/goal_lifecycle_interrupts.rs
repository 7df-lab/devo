use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use devo_core::DurableRecord;
use devo_protocol::Usage;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[path = "support/goal_continuation.rs"]
mod support;

use support::BudgetWrapupPendingProvider;
use support::PendingProvider;
use support::build_runtime;
use support::collect_until_turn_completed;
use support::initialize_connection;
use support::start_session;
use support::wait_for_notification;
use support::wait_for_request_count;

#[tokio::test]
async fn goal_clear_interrupts_active_hidden_continuation_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    start_created_goal(
        &runtime,
        connection_id,
        session_id,
        "clear should stop hidden turn",
        /*replace_existing*/ false,
    )
    .await?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let turn_id = notification_turn_id(&turn_started).context("hidden turn id")?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 120,
                "method": "_devo/goal/clear",
                "params": {
                    "sessionId": session_id
                }
            }),
        )
        .await
        .context("goal/clear response")?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert_turn_interrupted(&notifications, &turn_id);
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn goal_complete_interrupts_active_hidden_continuation_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    start_created_goal(
        &runtime,
        connection_id,
        session_id,
        "complete should stop hidden turn",
        /*replace_existing*/ false,
    )
    .await?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let turn_id = notification_turn_id(&turn_started).context("hidden turn id")?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 121,
                "method": "_devo/goal/complete",
                "params": {
                    "sessionId": session_id,
                    "status": "complete"
                }
            }),
        )
        .await
        .context("goal/complete response")?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert_turn_interrupted(&notifications, &turn_id);
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn goal_cancel_interrupts_active_hidden_continuation_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    start_created_goal(
        &runtime,
        connection_id,
        session_id,
        "cancel should stop hidden turn",
        /*replace_existing*/ false,
    )
    .await?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let turn_id = notification_turn_id(&turn_started).context("hidden turn id")?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;
    let goal_id = persisted_goal_id(data_root.path(), session_id)?;

    let cancel_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 126,
                "method": "_devo/goal/cancel",
                "params": {
                    "session_id": session_id,
                    "goal_id": goal_id
                }
            }),
        )
        .await
        .context("goal/cancel response")?;
    let _response: devo_server::SuccessResponse<devo_protocol::GoalSetStatusResult> =
        serde_json::from_value(cancel_response)?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert_turn_interrupted(&notifications, &turn_id);
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn replacing_goal_interrupts_old_hidden_turn_and_starts_new_goal_cleanly() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    start_created_goal(
        &runtime,
        connection_id,
        session_id,
        "old hidden goal",
        /*replace_existing*/ false,
    )
    .await?;
    let first_turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let first_turn_id =
        notification_turn_id(&first_turn_started).context("first hidden turn id")?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;

    start_created_goal(
        &runtime,
        connection_id,
        session_id,
        "new replacement goal",
        /*replace_existing*/ true,
    )
    .await?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert_turn_interrupted(&notifications, &first_turn_id);
    wait_for_request_count(&provider.requests, /*expected*/ 2).await?;

    let status_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 122,
                "method": "_devo/goal/status",
                "params": {
                    "sessionId": session_id
                }
            }),
        )
        .await
        .context("goal/status response")?;
    let response: devo_server::SuccessResponse<devo_protocol::GoalStatusResult> =
        serde_json::from_value(status_response)?;
    assert_eq!(
        response.result.goal.map(|goal| {
            (
                goal.objective,
                goal.status,
                goal.tokens_used,
                goal.time_used_seconds,
            )
        }),
        Some((
            "new replacement goal".to_string(),
            devo_protocol::ThreadGoalStatus::Active,
            0,
            0,
        ))
    );
    assert_eq!(provider.requests.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn pausing_budget_limited_wrapup_preserves_budget_limited_status() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(BudgetWrapupPendingProvider {
        requests: std::sync::atomic::AtomicUsize::new(0),
        captured_requests: Mutex::new(Vec::new()),
        usage: Usage {
            input_tokens: 120,
            output_tokens: 30,
            cache_creation_input_tokens: Some(40),
            cache_read_input_tokens: Some(70),
            reasoning_output_tokens: None,
            total_tokens: None,
        },
    });
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 123,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "objective": "preserve budget-limited status",
                    "status": "active",
                    "tokenBudget": 80
                }
            }),
        )
        .await
        .context("goal/set response")?;
    collect_until_turn_completed(&mut notifications_rx).await?;
    wait_for_request_count(&provider.requests, /*expected*/ 2).await?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let turn_id = notification_turn_id(&turn_started).context("budget wrap-up turn id")?;
    wait_for_notification(&mut notifications_rx, "item/agentMessage/delta").await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;

    let pause_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 124,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "status": "paused"
                }
            }),
        )
        .await
        .context("goal pause response")?;
    let response: devo_server::SuccessResponse<devo_protocol::GoalSetResult> =
        serde_json::from_value(pause_response)?;
    assert_eq!(
        response.result.goal.status,
        devo_protocol::ThreadGoalStatus::BudgetLimited
    );

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert_turn_interrupted(&notifications, &turn_id);
    Ok(())
}

async fn start_created_goal(
    runtime: &Arc<devo_server::ServerRuntime>,
    connection_id: u64,
    session_id: devo_protocol::SessionId,
    objective: &str,
    replace_existing: bool,
) -> Result<()> {
    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 125,
                "method": "_devo/goal/create",
                "params": {
                    "sessionId": session_id,
                    "objective": objective,
                    "replaceExisting": replace_existing
                }
            }),
        )
        .await
        .context("goal/create response")?;
    Ok(())
}

fn notification_turn_id(value: &serde_json::Value) -> Option<serde_json::Value> {
    value
        .get("params")
        .and_then(|params| params.get("turn"))
        .and_then(|turn| turn.get("turn_id"))
        .cloned()
}

fn assert_turn_interrupted(notifications: &[serde_json::Value], turn_id: &serde_json::Value) {
    assert!(
        notifications.iter().any(|value| {
            value.get("method") == Some(&serde_json::json!("turn/interrupted"))
                && value
                    .get("params")
                    .and_then(|params| params.get("turn"))
                    .and_then(|turn| turn.get("turn_id"))
                    == Some(turn_id)
        }),
        "expected turn/interrupted for {turn_id}"
    );
}

fn persisted_goal_id(
    data_root: &std::path::Path,
    session_id: devo_protocol::SessionId,
) -> Result<String> {
    let path = data_root
        .join("goal-records")
        .join("sessions")
        .join(format!("{session_id}.jsonl"));
    let contents = std::fs::read_to_string(path).context("read durable goal records")?;
    for line in contents.lines().rev() {
        let record: DurableRecord = serde_json::from_str(line).context("parse durable record")?;
        if let DurableRecord::GoalCreated(record) = record {
            return Ok(format!("goal-{}", record.goal_id.0));
        }
    }
    anyhow::bail!("goal id not found in durable records")
}
