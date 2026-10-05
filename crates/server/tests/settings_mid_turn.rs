//! Behavior test for L2-DES-CONV-002 Phase 3: tightening the permission
//! preset mid-turn takes effect at the next tool-call authorization. A
//! provider-simulated `ipython` permission request auto-resolves under
//! `fullAccess` but requires user approval under `default`; switching presets
//! while the turn is running must change the outcome of the next tool call.
//!
//! The fixture deliberately adds permission fields not advertised for
//! `ipython` to exercise the server-side approval pipeline.

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

/// First stream request issues tool call `call-1`; the second waits for the
/// test to open the gate and then issues tool call `call-2`; later requests
/// end the turn with plain text.
struct TwoProbesProvider {
    stream_requests: Mutex<Vec<ModelRequest>>,
    go_second: Arc<Notify>,
}

/// Tool-call on request 1, plain-text done afterwards. The first stream is
/// gated so the test can patch settings before the next model request.
struct ToolThenDoneProvider {
    stream_requests: Mutex<Vec<ModelRequest>>,
    stream_started: Arc<Notify>,
    release_stream: Arc<Notify>,
}

#[async_trait]
impl ModelProviderSDK for TwoProbesProvider {
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
        let tool_call_events = |id: &str, response_id: &str| {
            let input = json!({
                "code": PROBE_CODE,
                "sandbox_permissions": "with_additional_permissions",
                "additional_permissions": { "network": { "enabled": true } }
            });
            vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: id.into(),
                    name: "ipython".into(),
                    input: input.clone(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: response_id.into(),
                        content: vec![ResponseContent::ToolUse {
                            id: id.into(),
                            name: "ipython".into(),
                            input,
                        }],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                }),
            ]
        };
        let done_events = || {
            vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "Done.".into(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-done".into(),
                        content: vec![ResponseContent::Text("Done.".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                }),
            ]
        };
        let stream: Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>> =
            match request_number {
                1 => Box::pin(stream::iter(tool_call_events("call-1", "resp-1"))),
                2 => {
                    let go_second = Arc::clone(&self.go_second);
                    let mut events = tool_call_events("call-2", "resp-2").into_iter();
                    let first = events.next().expect("tool call start event");
                    Box::pin(
                        stream::once(async move {
                            go_second.notified().await;
                            first
                        })
                        .chain(stream::iter(events.collect::<Vec<_>>())),
                    )
                }
                _ => Box::pin(stream::iter(done_events())),
            };
        Ok(stream)
    }

    fn name(&self) -> &str {
        "two-probes-provider"
    }
}

#[async_trait]
impl ModelProviderSDK for ToolThenDoneProvider {
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
                    input: json!({ "code": "print('model-switch-probe')" }),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-1".into(),
                        content: vec![ResponseContent::ToolUse {
                            id: "call-1".into(),
                            name: "ipython".into(),
                            input: json!({ "code": "print('model-switch-probe')" }),
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
                        id: "resp-n".into(),
                        content: vec![ResponseContent::Text("Done.".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                }),
            ]
        };
        if request_number == 1 {
            let stream_started = Arc::clone(&self.stream_started);
            let release_stream = Arc::clone(&self.release_stream);
            let mut events = events.into_iter();
            let first = events.next().expect("first stream event");
            Ok(Box::pin(
                stream::once(async move {
                    stream_started.notify_one();
                    release_stream.notified().await;
                    first
                })
                .chain(stream::iter(events.collect::<Vec<_>>())),
            ))
        } else {
            Ok(Box::pin(stream::iter(events)))
        }
    }

    fn name(&self) -> &str {
        "tool-then-done-provider"
    }
}

/// The no-op cell is harmless if executed; the malicious permission fields
/// in the first probe exercise the server-side approval gate.
const PROBE_CODE: &str = "print('permission-probe')";

fn build_runtime(data_root: &Path, provider: Arc<dyn ModelProviderSDK>) -> Arc<ServerRuntime> {
    TestRuntime::new(provider)
        .disabled_skills()
        .db_file("test_settings_mid_turn.db")
        .runtime(data_root)
}

/// True when a tool result for `call_id` reports an actual execution. The
/// permission denial itself is recorded as an *error* tool result — that one
/// must not count.
fn executed_tool_result(value: &serde_json::Value, call_id: &str) -> bool {
    let item = &value["params"]["item"]["item"];
    item.get("type") == Some(&serde_json::json!("toolResult"))
        && item["callId"] == serde_json::json!(call_id)
        && item["isError"] != serde_json::json!(true)
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

/// Trace: L2-DES-CONV-002
/// Verifies: tightening the permission preset mid-turn changes the next
/// tool-call authorization outcome — auto-run under fullAccess, interactive
/// approval required after switching to default (Phase 3 behavior level).
#[tokio::test]
async fn mid_turn_tighten_to_default_triggers_approval_for_ipython_permission_request() -> Result<()>
{
    let temp_dir = TempDir::new()?;
    let workspace_root = temp_dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root)?;

    let go_second = Arc::new(Notify::new());
    let provider = Arc::new(TwoProbesProvider {
        stream_requests: Mutex::new(Vec::new()),
        go_second: Arc::clone(&go_second),
    });
    let runtime = build_runtime(temp_dir.path(), provider);
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": workspace_root,
                    "idempotencyKey": "settings-mid-turn-1"
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

    // Start permissive: the first probe must run without any approval.
    let settings_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 4,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id.to_string(),
                    "expectedVersion": 1,
                    "settings": { "permissionProfile": "fullAccess" }
                }
            }),
        )
        .await
        .context("settings update response")?;
    assert!(
        settings_response.get("error").is_none(),
        "settings update failed: {settings_response}"
    );

    let turn_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 5,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "Probe twice." }],
                    "idempotencyKey": "settings-mid-turn-turn-1"
                }
            }),
        )
        .await
        .context("turn/start response")?;
    assert!(
        turn_response.get("error").is_none(),
        "turn/start failed: {turn_response}"
    );

    // The first probe executes without asking (fullAccess).
    let mut seen = Vec::new();
    timeout(Duration::from_secs(10), async {
        while let Some(value) = notifications_rx.recv().await {
            let executed = executed_tool_result(&value, "call-1");
            seen.push(value);
            if executed {
                return;
            }
        }
        panic!("notification channel closed before the first probe executed");
    })
    .await
    .context("first probe should execute under fullAccess without approval")?;

    // Tighten mid-turn; the override must reach the running turn.
    let tighten_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 6,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id.to_string(),
                    "expectedVersion": 2,
                    "settings": { "permissionProfile": "default" }
                }
            }),
        )
        .await
        .context("tighten response")?;
    let tighten_result: devo_protocol::native::rpc_session::SessionMetadataUpdateResult =
        serde_json::from_value(tighten_response["result"].clone())
            .with_context(|| format!("tighten response: {tighten_response}"))?;
    assert!(tighten_result.applied_to_active_turn);

    // Release the second probe: under default, its explicit network request
    // requires interactive approval, so the approval notification must arrive
    // and the Python cell must NOT execute.
    go_second.notify_one();
    timeout(Duration::from_secs(10), async {
        while let Some(value) = notifications_rx.recv().await {
            let is_approval = matches!(
                value.get("method").and_then(serde_json::Value::as_str),
                Some("approval/permission/request") | Some("approval/command/request")
            );
            seen.push(value);
            if is_approval {
                return;
            }
        }
        panic!("notification channel closed before the approval request");
    })
    .await
    .context("second probe must require an approval under default")?;
    let second_ran = seen
        .iter()
        .any(|value| executed_tool_result(value, "call-2"));
    assert!(
        !second_ran,
        "the second probe must wait for approval instead of executing; seen: {seen:?}"
    );

    // Cleanup: interrupt the stalled turn.
    let _ = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 7,
                "method": "session/interrupt",
                "params": {
                    "scope": { "scope": "session", "sessionId": session_id }
                }
            }),
        )
        .await;
    Ok(())
}

/// Trace: L2-DES-CONV-002
/// Verifies: switching model and reasoning selection mid-turn makes the next
/// model request use the model-aware normalized selection.
#[tokio::test]
async fn mid_turn_model_switch_reaches_next_model_request() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let workspace_root = temp_dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root)?;

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let provider = Arc::new(ToolThenDoneProvider {
        stream_requests: Mutex::new(Vec::new()),
        stream_started: Arc::clone(&started),
        release_stream: Arc::clone(&release),
    });
    let runtime = build_runtime(temp_dir.path(), provider.clone());
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;

    let session_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": workspace_root,
                    "idempotencyKey": "settings-model-mid-turn-1"
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

    let permission_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 4,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "settings": { "permissionProfile": "fullAccess" }
                }
            }),
        )
        .await
        .context("permission setup response")?;
    assert!(
        permission_response.get("error").is_none(),
        "permission setup failed: {permission_response}"
    );

    let turn_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 5,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "Probe, then continue." }],
                    "idempotencyKey": "model-mid-turn-turn-1"
                }
            }),
        )
        .await
        .context("turn/start response")?;
    assert!(
        turn_response.get("error").is_none(),
        "turn/start failed: {turn_response}"
    );

    // The first request runs on the turn-start model; its stream is blocked
    // before the first event, so the next request is not built yet.
    if timeout(Duration::from_secs(10), started.notified())
        .await
        .is_err()
    {
        anyhow::bail!(
            "gated stream did not start; model request count={}",
            provider
                .stream_requests
                .lock()
                .expect("requests lock")
                .len()
        );
    }

    // Switch model and exercise the migration vocabulary while the first
    // iteration is blocked. The final `on` maps to the first non-off level
    // (`minimal` for gpt-5.5's minimal/low/medium/high/xhigh ladder).
    for (request_id, raw, expected) in [
        (6, "off", "off"),
        (7, "high", "high"),
        (8, "none", "off"),
        (9, "on", "minimal"),
    ] {
        let switch_response = runtime
            .handle_incoming(
                connection_id,
                json!({
                    "id": request_id,
                    "method": "session/metadata/update",
                    "params": {
                        "sessionId": session_id.to_string(),
                        "expectedVersion": 0,
                        "model": { "provider": "openai", "model": "gpt-5.5" },
                        "settings": { "reasoningEffort": raw }
                    }
                }),
            )
            .await
            .context("model and reasoning switch response")?;
        let switch_result: devo_protocol::native::rpc_session::SessionMetadataUpdateResult =
            serde_json::from_value(switch_response["result"].clone()).with_context(|| {
                format!("model and reasoning switch response: {switch_response}")
            })?;
        assert!(switch_result.applied_to_active_turn);
        assert_eq!(switch_result.session.model.model, "openai/gpt-5.5");
        assert_eq!(
            switch_result.session.settings.reasoning_effort.as_deref(),
            Some(expected)
        );
    }

    // Releasing the stream lets the loop build the next request, which must
    // already use the switched model.
    release.notify_one();
    let second_request: Option<(String, Option<String>)> =
        timeout(Duration::from_secs(10), async {
            loop {
                let requests = provider
                    .stream_requests
                    .lock()
                    .expect("requests lock")
                    .len();
                if requests >= 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            provider
                .stream_requests
                .lock()
                .expect("requests lock")
                .get(1)
                .map(|request| (request.model.clone(), request.request_thinking.clone()))
        })
        .await
        .context("second model request should arrive")?;
    assert_eq!(
        second_request,
        Some(("gpt-5.5".to_string(), Some("enabled".to_string()))),
        "the next model request must use the switched model (wire slug, \
         provider routing is orthogonal)"
    );

    let _ = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 10,
                "method": "session/interrupt",
                "params": {
                    "scope": { "scope": "session", "sessionId": session_id }
                }
            }),
        )
        .await;
    Ok(())
}
