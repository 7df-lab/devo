//! Audits the live tool-call parameter refresh: a streamed tool call starts
//! with empty parameters (`item/started`), and when the assembled model turn
//! delivers the complete input the server must re-broadcast `item/started`
//! for the same item so native clients can render the running row's command.
//! Without the refresh the parameters only appear at completion.

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use futures::stream;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
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

const TOOL_CODE: &str = "print('streamed input refresh')";

/// Streams a tool call the way real providers do: `ToolCallStart` with empty
/// input, the arguments via `ToolCallInputDelta`, then the assembled response
/// (whose ToolUse input is still empty — the merged arguments come from the
/// delta accumulation).
#[derive(Default)]
struct StreamedToolProvider {
    requests: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl ModelProviderSDK for StreamedToolProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("test provider does not support completion")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_number = self
            .requests
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let events = if request_number == 0 {
            let tool_input = json!({ "code": TOOL_CODE });
            vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "ipython-1".to_string(),
                    name: "ipython".to_string(),
                    input: json!({}),
                }),
                Ok(StreamEvent::ToolCallInputDelta {
                    index: 0,
                    partial_json: tool_input.to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-tools".to_string(),
                        content: vec![ResponseContent::ToolUse {
                            id: "ipython-1".to_string(),
                            name: "ipython".to_string(),
                            input: json!({}),
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
                    text: "done".to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-done".to_string(),
                        content: vec![ResponseContent::Text("done".to_string())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                }),
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }

    fn name(&self) -> &str {
        "streamed-tool-test-provider"
    }
}

fn build_runtime(data_root: &Path) -> Arc<ServerRuntime> {
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(StreamedToolProvider::default());
    TestRuntime::new(provider)
        .disabled_skills()
        .db_file("test_tool_param_refresh.db")
        .runtime(data_root)
}

fn tool_call_started_payload(value: &serde_json::Value) -> Option<&serde_json::Value> {
    if value.get("method").and_then(serde_json::Value::as_str) != Some("item/started") {
        return None;
    }
    let item = value.get("params")?.get("item")?;
    if item
        .get("item")?
        .get("type")
        .and_then(serde_json::Value::as_str)
        != Some("toolCall")
    {
        return None;
    }
    Some(item)
}

#[tokio::test]
async fn streamed_tool_call_rebroadcasts_started_with_complete_parameters() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let workspace_root = temp_dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root)?;

    let runtime = build_runtime(temp_dir.path());
    let (notifications_tx, mut notifications_rx) = devo_server::test_outbound_channel(4096);
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
                    "_meta": { "devo": { "protocol": "native", "typedItems": true } },
                    "clientInfo": { "name": "test", "title": "test", "version": "1.0.0" }
                }
            }),
        )
        .await
        .context("initialize response")?;
    assert!(
        initialize_response.get("error").is_none(),
        "initialize failed: {initialize_response}"
    );

    let session_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": workspace_root,
                    "idempotencyKey": "tool-param-refresh-session"
                }
            }),
        )
        .await
        .context("session/new response")?;
    assert!(
        session_response.get("error").is_none(),
        "session/new failed: {session_response}"
    );
    let session_id = session_response["result"]["session"]["id"]
        .as_str()
        .context("session id in session/new response")?
        .to_string();

    let subscription_response = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 3,
                "method": "subscription/create",
                "params": {
                    "selectors": [{ "kind": "session", "sessionId": session_id }],
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
                    "input": [{ "type": "text", "text": "Run the tool." }],
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

    let mut tool_call_started = Vec::new();
    let deadline = Duration::from_secs(30);
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        let Ok(Some(value)) = timeout(Duration::from_secs(10), notifications_rx.recv()).await
        else {
            break;
        };
        if tool_call_started_payload(&value).is_some() {
            tool_call_started.push(value.clone());
        }
        let is_turn_completed =
            value.get("method").and_then(serde_json::Value::as_str) == Some("turn/completed");
        if is_turn_completed {
            break;
        }
    }

    assert_eq!(
        tool_call_started.len(),
        2,
        "expected exactly two item/started notifications for the streamed tool call, got: {tool_call_started:?}"
    );

    let first = tool_call_started[0]["params"]["item"].clone();
    let second = tool_call_started[1]["params"]["item"].clone();
    assert_eq!(
        first["id"], second["id"],
        "refresh must reuse the original item id"
    );
    assert_eq!(first["seq"], second["seq"]);

    let first_input = first["item"]["input"].clone();
    let first_input_empty = first_input.is_null()
        || matches!(&first_input, serde_json::Value::Object(map) if map.is_empty());
    assert!(
        first_input_empty,
        "first item/started should carry empty streamed parameters: {first}"
    );

    assert_eq!(
        second["item"]["input"]["code"],
        json!(TOOL_CODE),
        "refreshed item/started must carry the complete parameters: {second}"
    );
    assert_eq!(
        second["state"],
        json!("running"),
        "refresh keeps the item in the running state: {second}"
    );

    Ok(())
}
