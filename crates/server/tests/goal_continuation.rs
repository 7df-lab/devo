use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use devo_protocol::Usage;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::Notify;

#[path = "support/goal_continuation.rs"]
mod support;

use support::BudgetWrapupPendingProvider;
use support::CapturingProvider;
use support::FailingProvider;
use support::PendingProvider;
use support::QueuedPriorityProvider;
use support::UsageProvider;
use support::build_runtime;
use support::collect_until_turn_completed;
use support::initialize_connection;
use support::is_user_message_item;
use support::pause_goal_and_interrupt_turn;
use support::request_contains_text;
use support::request_last_message_contains_text;
use support::start_session;
use support::wait_for_captured_request_count;
use support::wait_for_notification;
use support::wait_for_request_count;

#[tokio::test]
async fn goal_token_budget_reached_after_turn_enters_budget_limited() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(UsageProvider {
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

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 9,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "objective": "use budget",
                    "status": "active",
                    "tokenBudget": 80
                }
            }),
        )
        .await
        .context("goal/set response")?;
    collect_until_turn_completed(&mut notifications_rx).await?;
    wait_for_request_count(&provider.requests, /*expected*/ 2).await?;
    collect_until_turn_completed(&mut notifications_rx).await?;

    let status_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 10,
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

    let goal = response.result.goal.expect("goal");
    assert_eq!(goal.status, devo_protocol::ThreadGoalStatus::BudgetLimited);
    assert_eq!(goal.tokens_used, 160);
    assert_eq!(provider.requests.load(Ordering::SeqCst), 2);
    let requests = provider.captured_requests.lock().expect("lock requests");
    assert!(
        request_contains_text(&requests[1], "has reached its token budget")
            && request_contains_text(&requests[1], "do not start new substantive work"),
        "budget-limited goal should receive a wrap-up prompt"
    );
    Ok(())
}

#[tokio::test]
async fn budget_limited_goal_pause_interrupts_pending_wrapup_turn() -> Result<()> {
    // Trace: L2-DES-GOAL-001
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

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 110,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "objective": "pause during budget wrap-up",
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
    let turn_id = turn_started
        .get("params")
        .and_then(|params| params.get("turn"))
        .and_then(|turn| turn.get("turn_id"))
        .cloned()
        .context("turn id in budget wrap-up turn/started")?;
    wait_for_notification(&mut notifications_rx, "item/agentMessage/delta").await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 111,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "status": "paused"
                }
            }),
        )
        .await
        .context("goal pause response")?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert!(
        notifications.iter().any(|value| {
            value.get("method") == Some(&serde_json::json!("turn/interrupted"))
                && value
                    .get("params")
                    .and_then(|params| params.get("turn"))
                    .and_then(|turn| turn.get("turn_id"))
                    == Some(&turn_id)
        }),
        "pausing a budget-limited goal should interrupt the pending wrap-up turn"
    );
    assert!(
        notifications.iter().any(|value| {
            value.get("method") == Some(&serde_json::json!("item/completed"))
                && value
                    .get("params")
                    .and_then(|params| params.get("item"))
                    .and_then(|item| item.get("item_kind"))
                    == Some(&serde_json::json!("agent_message"))
                && value
                    .get("params")
                    .and_then(|params| params.get("item"))
                    .and_then(|item| item.get("payload"))
                    .and_then(|payload| payload.get("text"))
                    == Some(&serde_json::json!("Budget wrap-up started."))
        }),
        "interrupting the wrap-up turn should complete deferred assistant text"
    );
    assert_eq!(provider.requests.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn persisted_paused_goal_replays_without_continuation() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 11,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "objective": "persist paused goal",
                    "status": "paused"
                }
            }),
        )
        .await
        .context("paused goal/set response")?;
    assert_eq!(provider.requests.load(Ordering::SeqCst), 0);

    let replay_provider = Arc::new(PendingProvider::default());
    let replayed_runtime = build_runtime(data_root.path(), replay_provider.clone())?;
    replayed_runtime.load_persisted_sessions().await?;
    let (replayed_connection_id, _replayed_notifications_rx) =
        initialize_connection(&replayed_runtime).await?;

    let status_response = replayed_runtime
        .handle_incoming(
            replayed_connection_id,
            serde_json::json!({
                "id": 12,
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
        response.result.goal.as_ref().map(|goal| goal.status),
        Some(devo_protocol::ThreadGoalStatus::Paused)
    );
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;
    assert_eq!(replay_provider.requests.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn persisted_active_goal_pauses_on_restart_without_continuation() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(CapturingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 13,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "plan before setting goal" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null,
                    "collaboration_mode": "plan"
                }
            }),
        )
        .await
        .context("plan turn/start response")?;
    collect_until_turn_completed(&mut notifications_rx).await?;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 14,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "objective": "persist active goal without restart loop",
                    "status": "active"
                }
            }),
        )
        .await
        .context("active goal/set response")?;
    assert_eq!(provider.requests.lock().expect("lock requests").len(), 1);

    let replay_provider = Arc::new(PendingProvider::default());
    let replayed_runtime = build_runtime(data_root.path(), replay_provider.clone())?;
    replayed_runtime.load_persisted_sessions().await?;
    let (replayed_connection_id, _replayed_notifications_rx) =
        initialize_connection(&replayed_runtime).await?;

    let status_response = replayed_runtime
        .handle_incoming(
            replayed_connection_id,
            serde_json::json!({
                "id": 15,
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
        response.result.goal.as_ref().map(|goal| goal.status),
        Some(devo_protocol::ThreadGoalStatus::Paused)
    );
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;
    assert_eq!(replay_provider.requests.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn goal_pause_interrupts_active_hidden_continuation_turn() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 16,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "objective": "pause an active continuation",
                    "status": "active"
                }
            }),
        )
        .await
        .context("active goal/set response")?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    let turn_id = turn_started
        .get("params")
        .and_then(|params| params.get("turn"))
        .and_then(|turn| turn.get("turn_id"))
        .cloned()
        .context("turn id in turn/started")?;
    wait_for_request_count(&provider.requests, 1).await?;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 17,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "status": "paused"
                }
            }),
        )
        .await
        .context("paused goal/set response")?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert!(
        notifications.iter().any(|value| {
            value.get("method") == Some(&serde_json::json!("turn/interrupted"))
                && value
                    .get("params")
                    .and_then(|params| params.get("turn"))
                    .and_then(|turn| turn.get("turn_id"))
                    == Some(&turn_id)
        }),
        "pausing an active goal should interrupt the hidden continuation turn"
    );
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn provider_400_tool_call_adjacency_failure_pauses_goal_without_looping() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(FailingProvider {
        requests: std::sync::atomic::AtomicUsize::new(0),
        message: "model provider error: openai stream error: Invalid status code: 400 Bad Request; response body: assistant message with 'tool_calls' must be followed by tool messages responding to each 'tool_call_id'".to_string(),
    });
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 13,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "objective": "do not loop after bad request",
                    "status": "active"
                }
            }),
        )
        .await
        .context("goal/set response")?;
    collect_until_turn_completed(&mut notifications_rx).await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;

    let status_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 14,
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
        response.result.goal.as_ref().map(|goal| goal.status),
        Some(devo_protocol::ThreadGoalStatus::Paused)
    );
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn goal_set_starts_hidden_continuation_turn() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(CapturingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 19,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "previous visible prompt" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("prior turn/start response")?;
    collect_until_turn_completed(&mut notifications_rx).await?;

    let goal_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 20,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "objective": "write a benchmark note",
                    "status": "active"
                }
            }),
        )
        .await
        .context("goal/set response")?;
    let response: devo_server::SuccessResponse<devo_protocol::GoalSetResult> =
        serde_json::from_value(goal_response)?;
    assert_eq!(response.result.goal.objective, "write a benchmark note");
    tokio::time::timeout(Duration::from_secs(/*secs*/ 5), async {
        loop {
            if provider.requests.lock().expect("lock requests").len() >= 2 {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;
        }
    })
    .await
    .context("timed out waiting for captured provider request")??;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 21,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "status": "paused"
                }
            }),
        )
        .await
        .context("goal pause response")?;

    let notifications = collect_until_turn_completed(&mut notifications_rx).await?;
    assert!(
        notifications
            .iter()
            .any(|value| value.get("method") == Some(&serde_json::json!("turn/started"))),
        "goal continuation should start a turn"
    );
    assert!(
        !notifications.iter().any(is_user_message_item),
        "goal continuation must not emit a synthetic user message item"
    );

    let requests = provider.requests.lock().expect("lock requests");
    assert!(requests.len() >= 2);
    assert!(
        request_contains_text(&requests[1], "Completion audit:")
            && request_contains_text(&requests[1], "write a benchmark note"),
        "goal continuation request should include hidden goal context"
    );
    assert!(
        request_last_message_contains_text(&requests[1], "Completion audit:"),
        "autonomous goal context should be the latest request message"
    );

    Ok(())
}

#[tokio::test]
async fn goal_set_does_not_start_continuation_while_turn_is_active() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 30,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "keep this turn active" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 31,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "objective": "continue after this turn",
                    "status": "active"
                }
            }),
        )
        .await
        .context("goal/set response")?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 32,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "status": "paused"
                }
            }),
        )
        .await
        .context("goal pause response")?;
    let turn_id = turn_started["params"]["turn"]["turn_id"]
        .as_str()
        .context("turn id")?;
    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 33,
                "method": "_devo/turn/interrupt",
                "params": {
                    "session_id": session_id,
                    "turn_id": turn_id,
                    "reason": "test cleanup"
                }
            }),
        )
        .await
        .context("turn/interrupt response")?;

    Ok(())
}

#[tokio::test]
async fn goal_create_starts_hidden_continuation_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 34,
                "method": "_devo/goal/create",
                "params": {
                    "sessionId": session_id,
                    "objective": "created goal should run",
                    "replaceExisting": false
                }
            }),
        )
        .await
        .context("goal/create response")?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;

    let turn_id: devo_protocol::TurnId =
        serde_json::from_value(turn_started["params"]["turn"]["turn_id"].clone())?;
    pause_goal_and_interrupt_turn(&runtime, connection_id, session_id, turn_id).await?;
    Ok(())
}

#[tokio::test]
async fn goal_resume_starts_hidden_continuation_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(PendingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 35,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "objective": "paused goal should resume",
                    "status": "paused"
                }
            }),
        )
        .await
        .context("paused goal/set response")?;
    assert_eq!(provider.requests.load(Ordering::SeqCst), 0);

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 36,
                "method": "_devo/goal/resume",
                "params": {
                    "sessionId": session_id,
                    "status": "active"
                }
            }),
        )
        .await
        .context("goal/resume response")?;
    let turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    wait_for_request_count(&provider.requests, /*expected*/ 1).await?;

    let turn_id: devo_protocol::TurnId =
        serde_json::from_value(turn_started["params"]["turn"]["turn_id"].clone())?;
    pause_goal_and_interrupt_turn(&runtime, connection_id, session_id, turn_id).await?;
    Ok(())
}

#[tokio::test]
async fn queued_user_turn_runs_before_goal_continuation() -> Result<()> {
    let data_root = TempDir::new()?;
    let release_first = Arc::new(Notify::new());
    let provider = Arc::new(QueuedPriorityProvider {
        requests: Mutex::new(Vec::new()),
        release_first: Arc::clone(&release_first),
    });
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let active_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 40,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "hold the first turn" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("active turn/start response")?;
    let active_result: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(active_response)?;
    let active_turn_id = active_result
        .result
        .turn_id()
        .expect("active turn/start should start a turn");
    wait_for_captured_request_count(&provider.requests, /*expected*/ 1).await?;
    wait_for_notification(&mut notifications_rx, "turn/started").await?;

    let queued_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 41,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "queued user input wins" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("queued turn/start response")?;
    let queued_result: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(queued_response)?;
    let devo_server::TurnStartResult::Queued {
        active_turn_id: queued_active_turn_id,
        queued_input_id,
        status,
        ..
    } = queued_result.result
    else {
        panic!("expected queued turn/start result");
    };
    assert_eq!(queued_active_turn_id, active_turn_id);
    assert_ne!(queued_input_id.to_string(), active_turn_id.to_string());
    assert_eq!(status, devo_core::TurnStatus::Pending);

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 42,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "objective": "do not skip queued input",
                    "status": "active"
                }
            }),
        )
        .await
        .context("goal/set response")?;

    release_first.notify_one();
    let queued_turn_started = wait_for_notification(&mut notifications_rx, "turn/started").await?;
    wait_for_captured_request_count(&provider.requests, /*expected*/ 2).await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;
    {
        let requests = provider.requests.lock().expect("lock requests");
        assert_eq!(requests.len(), 2);
        assert!(
            request_contains_text(&requests[1], "queued user input wins"),
            "queued user turn should be the next provider request"
        );
    }

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 43,
                "method": "_devo/goal/set",
                "params": {
                    "sessionId": session_id,
                    "status": "paused"
                }
            }),
        )
        .await
        .context("goal pause response")?;
    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 44,
                "method": "_devo/turn/interrupt",
                "params": {
                    "session_id": session_id,
                    "turn_id": queued_turn_started["params"]["turn"]["turn_id"],
                    "reason": "test cleanup"
                }
            }),
        )
        .await;

    Ok(())
}
