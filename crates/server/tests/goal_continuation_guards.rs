use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_protocol::native::rpc_session::GoalIfExists;
use devo_provider::ModelProviderSDK;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[path = "support/goal_continuation.rs"]
mod support;

use support::CapturingProvider;
use support::build_runtime;
use support::collect_until_turn_completed;
use support::create_goal;
use support::initialize_connection;
use support::pause_goal_and_interrupt_session;
use support::set_session_mode;
use support::start_session;
use support::title_response;
use support::wait_for_approval_request;
use support::wait_for_notification;

#[tokio::test]
async fn goal_set_does_not_start_continuation_in_plan_mode() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(CapturingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;
    set_session_mode(&runtime, connection_id, session_id, "plan").await?;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 40,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "plan first" }],
                    "idempotencyKey": format!("native-test-turn-{}", uuid::Uuid::new_v4()),
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
    assert_eq!(provider.requests.lock().expect("lock requests").len(), 1);

    create_goal(
        &runtime,
        connection_id,
        session_id,
        "do not continue in plan mode",
        None,
        GoalIfExists::Reject,
        "goal-plan-mode",
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;
    assert_eq!(provider.requests.lock().expect("lock requests").len(), 1);
    Ok(())
}

/// The Python cell is harmless if it is ever executed, but the adversarial
/// permission fields must park it at an interactive approval.
const PARK_CODE: &str = "print('parked')";

struct ToolCallProvider {
    requests: AtomicUsize,
    tool_name: &'static str,
    tool_input: serde_json::Value,
}

#[async_trait]
impl ModelProviderSDK for ToolCallProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(title_response())
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_number = self.requests.fetch_add(1, Ordering::SeqCst);
        if request_number == 0 {
            return Ok(Box::pin(stream::iter(vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "tool-1".to_string(),
                    name: self.tool_name.to_string(),
                    input: serde_json::json!({}),
                }),
                Ok(StreamEvent::ToolCallInputDelta {
                    index: 0,
                    partial_json: self.tool_input.to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "tool-call-response".to_string(),
                        content: vec![ResponseContent::ToolUse {
                            id: "tool-1".to_string(),
                            name: self.tool_name.to_string(),
                            input: self.tool_input.clone(),
                        }],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                }),
            ])));
        }

        Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::MessageDone {
            response: ModelResponse {
                id: "tool-followup-response".to_string(),
                content: vec![ResponseContent::Text("Tool follow-up done.".to_string())],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
                metadata: ResponseMetadata::default(),
            },
        })])))
    }

    fn name(&self) -> &str {
        "tool-call-goal-provider"
    }
}

#[tokio::test]
async fn goal_set_does_not_start_continuation_while_approval_is_pending() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ToolCallProvider {
        requests: AtomicUsize::new(0),
        tool_name: "ipython",
        tool_input: serde_json::json!({
            "code": PARK_CODE,
            "sandbox_permissions": "with_additional_permissions",
            "additional_permissions": { "network": { "enabled": true } }
        }),
    });
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;
    // Use the default preset for a normal session, then send a malicious
    // ipython call with unadvertised permission fields. The server must still
    // park it for an interactive approval.
    let tighten_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 49,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id.to_string(),
                    "expectedVersion": 1,
                    "settings": { "permissionProfile": "default" }
                }
            }),
        )
        .await
        .context("tighten response")?;
    assert!(
        tighten_response.get("error").is_none(),
        "tighten failed: {tighten_response}"
    );
    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 50,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "ask for approval" }],
                    "idempotencyKey": format!("native-test-turn-{}", uuid::Uuid::new_v4()),
                    "model": null,
                    "sandbox": null,
                    "approval_policy": "on-request",
                    "cwd": null
                }
            }),
        )
        .await
        .context("approval turn/start response")?;
    let _start_result: devo_server::SuccessResponse<
        devo_protocol::native::rpc_turn::TurnStartResult,
    > = serde_json::from_value(start_response)?;
    wait_for_approval_request(&mut notifications_rx).await?;
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);

    create_goal(
        &runtime,
        connection_id,
        session_id,
        "wait for approval first",
        None,
        GoalIfExists::Reject,
        "goal-approval-pending",
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);

    pause_goal_and_interrupt_session(&runtime, connection_id, session_id).await?;
    Ok(())
}

#[tokio::test]
async fn goal_set_does_not_start_continuation_while_user_input_is_pending() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ToolCallProvider {
        requests: AtomicUsize::new(0),
        tool_name: "ipython",
        tool_input: serde_json::json!({
            "code": "await rlm.request_user_input([{'id': 'choice', 'header': 'Path', 'question': 'Which path should the goal use?', 'options': [{'label': 'A', 'description': 'Use path A'}, {'label': 'B', 'description': 'Use path B'}]}])"
        }),
    });
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;
    // This checks goal scheduling while user input is pending, independent of
    // Plan's fail-closed read-only kernel fence. The temp workspace sits under
    // /tmp, which is intentionally a writable scratch ancestor in Plan mode.

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 60,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "ask the user" }],
                    "idempotencyKey": format!("native-test-turn-{}", uuid::Uuid::new_v4()),
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null,
                    "collaboration_mode": "build"
                }
            }),
        )
        .await
        .context("request_user_input turn/start response")?;
    let _start_result: devo_server::SuccessResponse<
        devo_protocol::native::rpc_turn::TurnStartResult,
    > = serde_json::from_value(start_response)?;
    wait_for_notification(&mut notifications_rx, "userInput/request").await?;
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);

    create_goal(
        &runtime,
        connection_id,
        session_id,
        "wait for user input first",
        None,
        GoalIfExists::Reject,
        "goal-user-input-pending",
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);

    pause_goal_and_interrupt_session(&runtime, connection_id, session_id).await?;
    Ok(())
}
