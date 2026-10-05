//! Behavior tests for approval-parked turn preemption: a user message sent
//! while a turn is parked at an approval checkpoint must not be rejected or
//! stranded — the parked approval is cancelled, the parked turn is
//! interrupted, and the message runs as a fresh turn.
//!
//! The scripted provider simulates a malicious model call: it adds an
//! unadvertised network permission request to an `ipython` call. The server
//! must route it through interactive approval while keeping MCP and shell
//! tools hidden from the model.

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::{Context, Result};
use async_trait::async_trait;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;

use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::test_support::TestRuntime;

/// The Python cell has no side effects. Its adversarial permission fields
/// must still park the turn before the cell can run.
const PROBE_CODE: &str = "print('parked')";

fn permission_probe_input() -> serde_json::Value {
    json!({
        "code": PROBE_CODE,
        "sandbox_permissions": "with_additional_permissions",
        "additional_permissions": { "network": { "enabled": true } }
    })
}

/// First stream request issues an `ipython` call with unadvertised permission
/// fields (which parks the turn); later requests end the turn with plain text.
struct ToolCallThenDoneProvider {
    stream_requests: Mutex<Vec<ModelRequest>>,
}

#[async_trait]
impl ModelProviderSDK for ToolCallThenDoneProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title-1".into(),
            content: vec![ResponseContent::Text("Generated title".into())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_number = {
            let mut requests = self.stream_requests.lock().expect("stream request lock");
            requests.push(request);
            requests.len()
        };
        let events = if request_number == 1 {
            vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "call-1".into(),
                    name: "ipython".into(),
                    input: permission_probe_input(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "stream-1".into(),
                        content: vec![ResponseContent::ToolUse {
                            id: "call-1".into(),
                            name: "ipython".into(),
                            input: permission_probe_input(),
                        }],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                }),
            ]
        } else {
            vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "Done.".into(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: format!("stream-{request_number}"),
                        content: vec![ResponseContent::Text("Done.".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                }),
            ]
        };
        Ok(Box::pin(futures::stream::iter(events)))
    }
    fn name(&self) -> &str {
        "tool-call-then-done-provider"
    }
}

fn build_runtime(data_root: &Path, provider: Arc<ToolCallThenDoneProvider>) -> Arc<ServerRuntime> {
    TestRuntime::new(provider)
        .disabled_skills()
        .db_file("test_approval_park_preemption.db")
        .runtime(data_root)
}

async fn initialize_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(4096);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let initialize_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": { "name": "test", "title": "test", "version": "1.0.0" },
                    "_meta": { "devo": { "protocol": "native" } }
                }
            }),
        )
        .await
        .context("initialize response")?;
    assert_eq!(
        initialize_response["result"]["agentInfo"]["name"],
        json!("devo-server")
    );
    Ok((connection_id, notifications_rx))
}

/// Starts a session and a turn that parks at an `ipython` approval. Returns
/// (connection_id, notifications_rx, session_id, parked_turn_id).
async fn park_turn_at_approval(
    runtime: &Arc<ServerRuntime>,
    workspace_root: &Path,
) -> Result<(
    u64,
    mpsc::Receiver<serde_json::Value>,
    devo_protocol::SessionId,
    String,
)> {
    let (connection_id, mut notifications_rx) = initialize_connection(runtime).await?;
    let session_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": workspace_root,
                    "idempotencyKey": "approval-park-preemption-1"
                }
            }),
        )
        .await
        .context("session/new response")?;
    let session_result: devo_protocol::native::rpc_session::SessionNewResult =
        serde_json::from_value(session_response["result"].clone())
            .with_context(|| format!("session/new response: {session_response}"))?;
    let session_id = session_result.session.id;
    let subscription_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 3,
                "method": "subscription/create",
                "params": {
                    "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
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
    // Sessions start at AutoReview. The explicit unadvertised network request
    // on the `ipython` call still requires an interactive approval.
    let tighten_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 4,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id.to_string(),
                    "expectedVersion": 1,
                    "settings": { "permissionProfile": "default" }
                }
            }),
        )
        .await
        .context("settings update response")?;
    assert!(
        tighten_response.get("error").is_none(),
        "settings update failed: {tighten_response}"
    );
    let turn_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 5,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "Fetch the probe URL." }],
                    "idempotencyKey": "approval-park-preemption-turn-1"
                }
            }),
        )
        .await
        .context("turn/start response")?;
    assert!(
        turn_response.get("error").is_none(),
        "turn/start failed: {turn_response}"
    );
    let parked_turn_id = turn_response["result"]["turn"]["id"]
        .as_str()
        .context("turn/start result carries turn id")?
        .to_owned();
    // Wait until the explicit permission request parks the turn.
    timeout(Duration::from_secs(10), async {
        while let Some(value) = notifications_rx.recv().await {
            // `ipython` permission requests arrive on the generic permission route.
            if matches!(
                value.get("method").and_then(serde_json::Value::as_str),
                Some("approval/permission/request") | Some("approval/command/request")
            ) {
                return;
            }
        }
        panic!("notification channel closed before approval request");
    })
    .await
    .context("approval request should arrive for the ipython permission probe")?;
    Ok((connection_id, notifications_rx, session_id, parked_turn_id))
}

/// Collects notifications until `predicate` matches, returning everything
/// seen (including the match).
async fn collect_until(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> Result<Vec<serde_json::Value>> {
    let mut seen = Vec::new();
    timeout(Duration::from_secs(15), async {
        while let Some(value) = notifications_rx.recv().await {
            let matches = predicate(&value);
            seen.push(value);
            if matches {
                return;
            }
        }
        panic!("notification channel closed before predicate matched");
    })
    .await
    .context("expected notification never arrived")?;
    Ok(seen)
}

fn turn_completed_status(value: &serde_json::Value, turn_id: &str) -> Option<String> {
    if value.get("method").and_then(serde_json::Value::as_str) == Some("turn/completed")
        && value["params"]["turn"]["id"] == serde_json::json!(turn_id)
    {
        return value["params"]["turn"]["status"]
            .as_str()
            .map(ToOwned::to_owned);
    }
    None
}

fn has_cancelled_approval_item(value: &serde_json::Value) -> bool {
    let item = &value["params"]["item"]["item"];
    if item.get("type") != Some(&serde_json::json!("approval")) {
        return false;
    }
    item["decision"]["decision"] == serde_json::json!("cancelled")
}

/// True when a tool result for `call_id` reports an actual execution. The
/// permission denial itself is recorded as an *error* tool result ("permission
/// denied: … client request cancelled") — that one must not count.
fn executed_tool_result(value: &serde_json::Value, call_id: &str) -> bool {
    let item = &value["params"]["item"]["item"];
    item.get("type") == Some(&serde_json::json!("toolResult"))
        && item["callId"] == serde_json::json!(call_id)
        && item["isError"] != serde_json::json!(true)
}

#[tokio::test]
async fn steering_an_approval_parked_turn_cancels_the_approval_and_runs_the_message() -> Result<()>
{
    let temp_dir = TempDir::new()?;
    let workspace_root = temp_dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root)?;
    let provider = Arc::new(ToolCallThenDoneProvider {
        stream_requests: Mutex::new(Vec::new()),
    });
    let runtime = build_runtime(temp_dir.path(), provider);
    let (connection_id, mut notifications_rx, session_id, parked_turn_id) =
        park_turn_at_approval(&runtime, &workspace_root).await?;

    let steer_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 5,
                "method": "turn/steer",
                "params": {
                    "sessionId": session_id,
                    "expectedTurnId": parked_turn_id,
                    "input": [{ "type": "text", "text": "Stop that." }],
                    "idempotencyKey": "approval-park-preemption-steer-1"
                }
            }),
        )
        .await
        .context("turn/steer response")?;
    assert_eq!(
        steer_response["result"]["outcome"],
        serde_json::json!("degradedToQueue"),
        "steer should degrade to queue after preempting the parked turn: {steer_response}"
    );

    // The steered message runs as a fresh turn once the parked turn unwinds.
    let seen = collect_until(&mut notifications_rx, |value| {
        value.get("method").and_then(serde_json::Value::as_str) == Some("turn/completed")
            && value["params"]["turn"]["id"] != serde_json::json!(parked_turn_id)
            && value["params"]["turn"]["status"] == serde_json::json!("completed")
    })
    .await?;
    let cancelled = seen.iter().any(has_cancelled_approval_item);
    assert!(
        cancelled,
        "parked approval must be resolved as cancelled; seen: {seen:?}"
    );
    let parked_interrupted = seen.iter().any(|value| {
        turn_completed_status(value, &parked_turn_id).as_deref() == Some("interrupted")
    });
    assert!(
        parked_interrupted,
        "parked turn must finalize as interrupted; seen: {seen:?}"
    );
    let probe_ran = seen
        .iter()
        .any(|value| executed_tool_result(value, "call-1"));
    assert!(
        !probe_ran,
        "preempted command must never execute; seen: {seen:?}"
    );
    Ok(())
}

#[tokio::test]
async fn turn_start_on_approval_parked_session_admits_a_new_turn() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let workspace_root = temp_dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root)?;
    let provider = Arc::new(ToolCallThenDoneProvider {
        stream_requests: Mutex::new(Vec::new()),
    });
    let runtime = build_runtime(temp_dir.path(), provider);
    let (connection_id, mut notifications_rx, session_id, parked_turn_id) =
        park_turn_at_approval(&runtime, &workspace_root).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 5,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "New message instead." }],
                    "idempotencyKey": "approval-park-preemption-turn-2"
                }
            }),
        )
        .await
        .context("turn/start response")?;
    assert!(
        start_response.get("error").is_none(),
        "turn/start must not be rejected behind an approval-parked turn: {start_response}"
    );
    let new_turn_id = start_response["result"]["turn"]["id"]
        .as_str()
        .context("turn/start result carries the new turn id")?
        .to_owned();
    assert_ne!(
        new_turn_id, parked_turn_id,
        "the admitted turn must be a fresh turn"
    );

    let seen = collect_until(&mut notifications_rx, |value| {
        turn_completed_status(value, &new_turn_id).as_deref() == Some("completed")
    })
    .await?;
    let cancelled = seen.iter().any(has_cancelled_approval_item);
    assert!(
        cancelled,
        "parked approval must be resolved as cancelled; seen: {seen:?}"
    );
    let parked_interrupted = seen.iter().any(|value| {
        turn_completed_status(value, &parked_turn_id).as_deref() == Some("interrupted")
    });
    assert!(
        parked_interrupted,
        "parked turn must finalize as interrupted; seen: {seen:?}"
    );
    let probe_ran = seen
        .iter()
        .any(|value| executed_tool_result(value, "call-1"));
    assert!(
        !probe_ran,
        "preempted command must never execute; seen: {seen:?}"
    );
    Ok(())
}
