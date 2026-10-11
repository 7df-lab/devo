//! Reproduction audit for the reported UX bug: a message queued while a turn
//! is running must NOT leak into the in-flight turn's model requests, and
//! once the running turn ends and the entry drains into the follow-up turn,
//! a subscribed connection must receive `queue/updated` (`drained`) carrying
//! an empty queue — otherwise clients keep rendering the stale entry.

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use futures::stream;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;

use devo_core::BundledSkillsConfig;
use devo_core::SkillsConfig;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::RequestContent;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::SuccessResponse;
use devo_server::test_support::TestRuntime;

const QUEUED_TEXT: &str = "queued follow-up message";

/// First stream request issues an `ipython` tool call; request 2 blocks in the
/// provider until released, then ends the turn with plain text. Later
/// requests end immediately. All requests are captured for content asserts.
struct ToolCallThenGatedDoneProvider {
    stream_requests: Mutex<Vec<ModelRequest>>,
    release: Arc<Notify>,
    final_response_started: Arc<Notify>,
}

#[async_trait]
impl ModelProviderSDK for ToolCallThenGatedDoneProvider {
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
        let done_events = || {
            vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "Done.".into(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-n".into(),
                        content: vec![ResponseContent::Text("Done.".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                }),
            ]
        };
        let events = if request_number == 1 {
            vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "tool-1".into(),
                    name: "ipython".into(),
                    input: json!({ "code": "print('queue-drain-probe')" }),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-1".into(),
                        content: vec![ResponseContent::ToolUse {
                            id: "tool-1".into(),
                            name: "ipython".into(),
                            input: json!({ "code": "print('queue-drain-probe')" }),
                        }],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                }),
            ]
        } else if request_number == 2 {
            let release = Arc::clone(&self.release);
            let final_response_started = Arc::clone(&self.final_response_started);
            let mut events = done_events().into_iter();
            let first = events.next().expect("text delta event");
            let stream = futures::stream::once(async move {
                final_response_started.notify_one();
                first
            })
            .chain(futures::stream::once(async move {
                release.notified().await;
                events.next().expect("message done event")
            }))
            .boxed();
            return Ok(Box::pin(stream));
        } else {
            done_events()
        };
        Ok(Box::pin(stream::iter(events)))
    }

    fn name(&self) -> &str {
        "tool-call-then-gated-done-provider"
    }
}

fn build_runtime(data_root: &Path, provider: Arc<dyn ModelProviderSDK>) -> Arc<ServerRuntime> {
    TestRuntime::new(provider)
        .skills(SkillsConfig {
            enabled: false,
            user_roots: Vec::new(),
            workspace_roots: Vec::new(),
            watch_for_changes: false,
            bundled: Some(BundledSkillsConfig { enabled: false }),
            include_instructions: Some(false),
            config: Vec::new(),
        })
        .db_file("test_queue_drain.db")
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
                    "_meta": { "devo": { "protocol": "native" } },
                    "clientInfo": { "name": "test", "title": "test", "version": "1.0.0" }
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

fn all_user_request_texts(request: &ModelRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .filter(|message| message.role == "user")
        .flat_map(|message| {
            message.content.iter().filter_map(|content| match content {
                RequestContent::Reasoning { text } => Some(text.clone()),
                RequestContent::Text { text } => Some(text.clone()),
                RequestContent::ProviderReasoning { .. }
                | RequestContent::ToolUse { .. }
                | RequestContent::HostedToolUse { .. }
                | RequestContent::ToolResult { .. }
                | RequestContent::Image { .. } => None,
            })
        })
        .collect()
}

fn queue_updated_change(value: &serde_json::Value) -> Option<&str> {
    if value.get("method").and_then(serde_json::Value::as_str) != Some("queue/updated") {
        return None;
    }
    value
        .get("params")
        .and_then(|params| params.get("change"))
        .and_then(serde_json::Value::as_str)
}

async fn recv_until(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    label: &str,
    predicate: impl Fn(&serde_json::Value) -> bool,
    collected: &mut Vec<serde_json::Value>,
) -> Result<serde_json::Value> {
    loop {
        match timeout(Duration::from_secs(10), notifications_rx.recv()).await {
            Ok(Some(value)) => {
                if predicate(&value) {
                    return Ok(value);
                }
                collected.push(value);
            }
            Ok(None) => anyhow::bail!("notification channel closed waiting for {label}"),
            Err(_) => {
                let seen: Vec<String> = collected
                    .iter()
                    .filter_map(|value| {
                        value
                            .get("method")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    })
                    .collect();
                anyhow::bail!("timed out waiting for {label}; seen methods: {seen:?}");
            }
        }
    }
}

/// True when a tool result for `call_id` reports an actual execution.
fn executed_tool_result(value: &serde_json::Value, call_id: &str) -> bool {
    let item = &value["params"]["item"]["item"];
    item.get("type") == Some(&serde_json::json!("toolResult"))
        && item["callId"] == serde_json::json!(call_id)
        && item["isError"] != serde_json::json!(true)
}

fn is_turn_completed(value: &serde_json::Value) -> bool {
    value.get("method").and_then(serde_json::Value::as_str) == Some("turn/completed")
        || value
            .get("params")
            .and_then(|params| params.get("_meta").or_else(|| params.get("meta")))
            .and_then(|meta| meta.get("devo/originalMethod"))
            .and_then(serde_json::Value::as_str)
            == Some("turn/completed")
}

#[tokio::test]
async fn queued_input_drains_into_followup_turn_and_broadcasts_empty_queue() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let workspace_root = temp_dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root)?;

    let release = Arc::new(Notify::new());
    let provider = Arc::new(ToolCallThenGatedDoneProvider {
        stream_requests: Mutex::new(Vec::new()),
        release: Arc::clone(&release),
        final_response_started: Arc::new(Notify::new()),
    });
    let runtime = build_runtime(temp_dir.path(), provider.clone());
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": workspace_root,
                    "idempotencyKey": "queue-drain-session"
                }
            }),
        )
        .await
        .context("session/new response")?;
    let session_result: SuccessResponse<devo_protocol::native::rpc_session::SessionNewResult> =
        serde_json::from_value(session_response.clone())
            .with_context(|| format!("session/new response: {session_response}"))?;
    let session_id = devo_protocol::SessionId::from(session_result.result.session.id.as_str());

    // Subscribe exactly like the TUI does so `event_selectors` is populated
    // and `queue/updated` broadcasts target this connection.
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

    let turn_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 4,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "Start with the tool." }],
                    "idempotencyKey": format!("native-test-turn-{}", uuid::Uuid::new_v4()),
                    "model": null,
                    "thinking": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    assert!(
        turn_response.get("error").is_none(),
        "turn/start failed: {turn_response}"
    );
    // The Python probe executes; request 2 is then parked inside the
    // provider, keeping the turn in flight deterministically.
    timeout(Duration::from_secs(10), async {
        while let Some(value) = notifications_rx.recv().await {
            if executed_tool_result(&value, "tool-1") {
                return;
            }
        }
        panic!("notification channel closed before the probe executed");
    })
    .await
    .context("timed out waiting for the ipython probe to execute")?;

    // Turn is running: explicitly queue a follow-up (TUI Alt+Enter behavior).
    let push_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 5,
                "method": "session/queue/push",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": QUEUED_TEXT }],
                    "idempotencyKey": format!("native-test-turn-{}", uuid::Uuid::new_v4()),
                    "idempotencyKey": "queue-drain-audit"
                }
            }),
        )
        .await
        .context("session/queue/push response")?;
    let push_result: SuccessResponse<devo_protocol::native::rpc_turn::SessionQueuePushResult> =
        serde_json::from_value(push_response.clone())
            .with_context(|| format!("push_response: {push_response}"))?;
    let devo_protocol::native::rpc_turn::SessionQueuePushResult::Queued { entry } =
        push_result.result
    else {
        panic!("busy push must queue");
    };
    let queue_item_id = entry.queue_item_id.as_str().to_string();

    // The push broadcast must reach the subscribed connection.
    let mut collected = Vec::new();
    let added = recv_until(
        &mut notifications_rx,
        "queue/updated(added)",
        |value| queue_updated_change(value) == Some("added"),
        &mut collected,
    )
    .await?;
    assert_eq!(
        added["params"]["queue"].as_array().map(Vec::len),
        Some(1),
        "added notification should carry the queued entry: {added}"
    );

    release.notify_one();

    // First turn completes, the entry drains into the follow-up turn.
    recv_until(
        &mut notifications_rx,
        "first turn/completed",
        is_turn_completed,
        &mut collected,
    )
    .await?;
    let drained = recv_until(
        &mut notifications_rx,
        "queue/updated(drained)",
        |value| queue_updated_change(value) == Some("drained"),
        &mut collected,
    )
    .await?;
    assert_eq!(
        drained["params"]["queueItemId"].as_str(),
        Some(queue_item_id.as_str()),
        "drained notification should name the drained entry: {drained}"
    );
    assert_eq!(
        drained["params"]["queue"].as_array().map(Vec::len),
        Some(0),
        "drained notification must carry the empty queue, otherwise clients \
         keep rendering the stale entry: {drained}"
    );
    assert!(
        drained["params"]["startedTurnId"].as_str().is_some(),
        "drained notification should carry the follow-up turn id: {drained}"
    );
    recv_until(
        &mut notifications_rx,
        "second turn/completed",
        is_turn_completed,
        &mut collected,
    )
    .await?;

    // The queue is really empty server-side.
    let list_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 6,
                "method": "session/queue/list",
                "params": { "sessionId": session_id }
            }),
        )
        .await
        .context("session/queue/list response")?;
    assert_eq!(
        list_response["result"]["entries"].as_array().map(Vec::len),
        Some(0),
        "queue/list should be empty after the drain: {list_response}"
    );

    // Queued input must NOT steer the in-flight turn: the first turn's
    // post-tool request (request 2) never sees the queued text; only the
    // drained follow-up turn (request 3) does.
    let requests = provider
        .stream_requests
        .lock()
        .expect("captured requests lock");
    assert_eq!(requests.len(), 3, "expected T1 x2 + T2 x1 model requests");
    let in_flight_texts = all_user_request_texts(&requests[1]);
    assert!(
        in_flight_texts
            .iter()
            .all(|text| !text.contains(QUEUED_TEXT)),
        "queued input must not leak into the in-flight turn: {in_flight_texts:?}"
    );
    let followup_texts = all_user_request_texts(&requests[2]);
    assert!(
        followup_texts.iter().any(|text| text.contains(QUEUED_TEXT)),
        "drained input should appear in the follow-up turn: {followup_texts:?}"
    );
    Ok(())
}

#[tokio::test]
async fn steering_during_final_response_stays_in_the_active_turn() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let workspace_root = temp_dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root)?;
    let release = Arc::new(Notify::new());
    let final_response_started = Arc::new(Notify::new());
    let provider = Arc::new(ToolCallThenGatedDoneProvider {
        stream_requests: Mutex::new(Vec::new()),
        release: Arc::clone(&release),
        final_response_started: Arc::clone(&final_response_started),
    });
    let runtime = build_runtime(temp_dir.path(), provider.clone());
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let created = runtime
        .handle_incoming(connection_id, json!({
            "id": 2, "method": "session/new",
            "params": { "cwd": workspace_root, "idempotencyKey": "steer-final-response-session" }
        }))
        .await
        .context("session/new response")?;
    let session_id = created["result"]["session"]["id"]
        .as_str()
        .context("session id")?;
    let subscribed = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 3, "method": "subscription/create",
                "params": {
                    "selectors": [{ "kind": "session", "sessionId": session_id }],
                    "includeSnapshot": false
                }
            }),
        )
        .await
        .context("subscription/create response")?;
    assert!(subscribed.get("error").is_none(), "{subscribed}");
    let started = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 4, "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "Start with the tool." }],
                    "idempotencyKey": "steer-final-response-turn"
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let turn_id = started["result"]["turn"]["id"]
        .as_str()
        .context("turn id")?;
    let mut collected = Vec::new();
    recv_until(
        &mut notifications_rx,
        "executed Python probe",
        |value| executed_tool_result(value, "tool-1"),
        &mut collected,
    )
    .await?;
    timeout(Duration::from_secs(10), final_response_started.notified())
        .await
        .context("final response started streaming")?;
    // The next provider response has no tool calls. Enter must still inject
    // into this turn, rather than silently becoming a queued follow-up.
    let steering_text = "Change the remaining task before ending this turn.";
    let steered = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 5, "method": "turn/steer",
                "params": {
                    "sessionId": session_id, "expectedTurnId": turn_id,
                    "input": [{ "type": "text", "text": steering_text }],
                    "idempotencyKey": "steer-final-response-input"
                }
            }),
        )
        .await
        .context("turn/steer response")?;
    assert!(steered.get("error").is_none(), "{steered}");
    assert_eq!(steered["result"]["outcome"], json!("injected"));
    // The blocked model stream must not hold up the control plane.
    for (id, method, params) in [
        (7, "runtime/ping", json!({})),
        (8, "session/list", json!({})),
        (9, "session/items/list", json!({ "sessionId": session_id })),
        (
            10,
            "workspace/changes/read",
            json!({ "sessionId": session_id, "scopes": ["uncommitted"] }),
        ),
    ] {
        let response = timeout(
            Duration::from_secs(2),
            runtime.handle_incoming(
                connection_id,
                json!({ "id": id, "method": method, "params": params }),
            ),
        )
        .await
        .with_context(|| format!("responsive {method}"))?
        .with_context(|| format!("{method} response"))?;
        assert!(response.get("error").is_none(), "{method}: {response}");
    }
    release.notify_one();
    let completed = recv_until(
        &mut notifications_rx,
        "original turn completed",
        is_turn_completed,
        &mut collected,
    )
    .await?;
    assert_eq!(completed["params"]["turn"]["id"], json!(turn_id));
    let assistant_messages = collected
        .iter()
        .filter(|value| {
            value["method"] == "item/completed"
                && value["params"]["item"]["item"]["type"] == "assistantMessage"
        })
        .map(|value| value["params"]["item"]["item"]["text"].clone())
        .collect::<Vec<_>>();
    assert_eq!(assistant_messages, vec![json!("Done."), json!("Done.")]);
    let queued = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 6, "method": "session/queue/list",
                "params": { "sessionId": session_id }
            }),
        )
        .await
        .context("session/queue/list response")?;
    assert_eq!(queued["result"]["entries"], json!([]));
    assert_eq!(
        collected
            .iter()
            .filter(|value| value["method"] == "turn/started")
            .count(),
        1,
    );
    assert_eq!(
        collected
            .iter()
            .filter_map(queue_updated_change)
            .collect::<Vec<_>>(),
        Vec::<&str>::new(),
    );
    let requests = provider
        .stream_requests
        .lock()
        .expect("captured requests lock");
    assert_eq!(
        requests.len(),
        3,
        "steering must request another model leg in the same turn"
    );
    let steering_in_requests = requests
        .iter()
        .map(|request| {
            all_user_request_texts(request)
                .into_iter()
                .filter(|text| text.contains(steering_text))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        steering_in_requests,
        vec![vec![], vec![], vec![steering_text.to_string()]]
    );
    Ok(())
}
