use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
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
use devo_protocol::SessionId;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_protocol::native::rpc_session::GoalIfExists;
use devo_provider::ModelProviderSDK;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::timeout;

#[derive(Default)]
struct GoalTitleProvider {
    title_requests: Mutex<Vec<ModelRequest>>,
    stream_requests: AtomicUsize,
}

#[async_trait]
impl ModelProviderSDK for GoalTitleProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.title_requests
            .lock()
            .expect("lock title requests")
            .push(request);
        Ok(ModelResponse {
            id: "goal-title".to_string(),
            content: vec![ResponseContent::Text("Generated goal title".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.stream_requests.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::MessageDone {
            response: ModelResponse {
                id: "goal-turn".to_string(),
                content: vec![ResponseContent::Text(
                    "Goal continuation complete.".to_string(),
                )],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
                metadata: ResponseMetadata::default(),
            },
        })])))
    }

    fn name(&self) -> &str {
        "goal-title-provider"
    }
}

/// Trace: L2-DES-SERVER-title-generation
/// Verifies: goal/set applies a heuristic title immediately, then optional LLM polish.
#[tokio::test]
async fn goal_set_objective_generates_session_title_for_new_session() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(GoalTitleProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_untitled_session(&runtime, connection_id, data_root.path()).await?;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/goal/set",
                "params": {
                    "sessionId": session_id,
                    "objective": "investigate goal title generation",
                    "ifExists": GoalIfExists::Reject,
                    "idempotencyKey": "goal-title-generation"
                }
            }),
        )
        .await
        .context("session/goal/set response")?;

    wait_for_title_update(&mut notifications_rx, "investigate goal title generation").await?;
    wait_for_title_update(&mut notifications_rx, "Generated goal title").await?;

    let list_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 4,
                "method": "session/list",
                "params": {}
            }),
        )
        .await
        .context("session/list response")?;
    let sessions = decode_native_session_list_response(list_response)?;
    assert_eq!(sessions[0].title.as_deref(), Some("Generated goal title"));

    let title_requests = provider.title_requests.lock().expect("lock title requests");
    assert_eq!(title_requests.len(), 1);
    assert!(
        title_request_contains(&title_requests[0], "investigate goal title generation"),
        "title request should use the goal objective"
    );
    assert!(
        !title_request_contains(&title_requests[0], "/goal"),
        "title request should not include the slash-command wrapper"
    );
    Ok(())
}

#[tokio::test]
async fn goal_create_rejects_unknown_session() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(GoalTitleProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;
    let unknown_session_id = SessionId::new();

    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 5,
                "method": "session/goal/set",
                "params": {
                    "sessionId": unknown_session_id,
                    "objective": "unknown session goal",
                    "ifExists": "reject",
                    "idempotencyKey": "unknown-session-create"
                }
            }),
        )
        .await
        .context("goal/create response")?;

    assert_session_not_found(response)?;
    assert_eq!(
        provider
            .title_requests
            .lock()
            .expect("lock title requests")
            .len(),
        0
    );
    assert_eq!(provider.stream_requests.load(Ordering::SeqCst), 0);
    assert_goal_status_empty(&runtime, connection_id, unknown_session_id).await?;
    Ok(())
}

#[tokio::test]
async fn goal_set_rejects_unknown_session() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(GoalTitleProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;
    let unknown_session_id = SessionId::new();

    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 6,
                "method": "session/goal/set",
                "params": {
                    "sessionId": unknown_session_id,
                    "objective": "unknown session goal",
                    "ifExists": "reject",
                    "idempotencyKey": "unknown-session-set"
                }
            }),
        )
        .await
        .context("goal/set response")?;

    assert_session_not_found(response)?;
    assert_eq!(
        provider
            .title_requests
            .lock()
            .expect("lock title requests")
            .len(),
        0
    );
    assert_eq!(provider.stream_requests.load(Ordering::SeqCst), 0);
    assert_goal_status_empty(&runtime, connection_id, unknown_session_id).await?;
    Ok(())
}

fn build_runtime(
    data_root: &std::path::Path,
    provider: Arc<GoalTitleProvider>,
) -> Result<Arc<ServerRuntime>> {
    Ok(devo_server::test_support::TestRuntime::new(provider)
        .with_test_model()
        .db_file("goal_title.db")
        .runtime(data_root))
}

async fn initialize_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(1024);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let initialize_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "_meta": { "devo": { "protocol": "native" } },
                    "clientInfo": {
                        "name": "goal-title-test",
                        "title": "goal-title-test",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    let response: serde_json::Value = initialize_response;
    assert_eq!(
        response["result"]["agentInfo"]["name"],
        serde_json::json!("devo-server")
    );
    Ok((connection_id, notifications_rx))
}

async fn start_untitled_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &std::path::Path,
) -> Result<SessionId> {
    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": cwd,
                    "idempotencyKey": "goal-title-session"
                }
            }),
        )
        .await
        .context("session/new response")?;
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionNewResult,
    > = serde_json::from_value(start_response)?;
    Ok(SessionId::from(response.result.session.id.as_str()))
}

async fn wait_for_title_update(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    expected_title: &str,
) -> Result<()> {
    timeout(Duration::from_secs(/*secs*/ 5), async {
        while let Some(value) = notifications_rx.recv().await {
            let is_native_title_update = value.get("method")
                == Some(&serde_json::json!("session/metadataUpdated"))
                && value["params"]["session"]["title"] == serde_json::json!(expected_title);
            let is_acp_title_update = value.get("method")
                == Some(&serde_json::json!("session/update"))
                && value["params"]["update"]["sessionUpdate"]
                    == serde_json::json!("session_info_update")
                && value["params"]["update"]["title"] == serde_json::json!(expected_title);
            if is_native_title_update || is_acp_title_update {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before expected session/metadataUpdated")
    })
    .await
    .context("timed out waiting for session/metadataUpdated")??;
    Ok(())
}

fn title_request_contains(request: &ModelRequest, needle: &str) -> bool {
    request.messages.iter().any(|message| {
        message.content.iter().any(|content| match content {
            devo_protocol::RequestContent::Text { text }
            | devo_protocol::RequestContent::Reasoning { text } => text.contains(needle),
            devo_protocol::RequestContent::ProviderReasoning { .. }
            | devo_protocol::RequestContent::ToolUse { .. }
            | devo_protocol::RequestContent::HostedToolUse { .. }
            | devo_protocol::RequestContent::ToolResult { .. }
            | devo_protocol::RequestContent::Image { .. } => false,
        })
    })
}

fn decode_native_session_list_response(
    response: serde_json::Value,
) -> Result<Vec<devo_protocol::native::session::Session>> {
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionListResult,
    > = serde_json::from_value(response)?;
    Ok(response.result.data)
}

fn assert_session_not_found(response: serde_json::Value) -> Result<()> {
    let response: devo_server::ErrorResponse = serde_json::from_value(response)?;
    assert_eq!(
        response.error.code,
        devo_server::ProtocolErrorCode::SessionNotFound
    );
    Ok(())
}

async fn assert_goal_status_empty(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 7,
                "method": "session/goal/read",
                "params": {
                    "sessionId": session_id
                }
            }),
        )
        .await
        .context("session/goal/read response")?;
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionGoalReadResult,
    > = serde_json::from_value(response)?;
    assert_eq!(response.result.goal, None);
    Ok(())
}
