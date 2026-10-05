#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use devo_protocol::AcpAvailableCommand;
use devo_protocol::AcpDeleteSessionResult;
use devo_protocol::AcpListSessionsResult;
use devo_protocol::AcpNewSessionResult;
use devo_protocol::AcpSessionNotification;
use devo_protocol::AcpSessionUpdate;
use devo_protocol::SessionId;
use devo_protocol::native::session::Session;
use devo_provider::ModelProviderSDK;
use devo_server::AcpErrorResponse;
use devo_server::AcpInitializeResult;
use devo_server::AcpSuccessResponse;
use devo_server::ClientTransportKind;
use devo_server::DEVO_SESSION_META;
use devo_server::ServerRuntime;
use devo_server::test_support::TestRuntime;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::timeout;

pub(crate) const ACP_TEST_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct AcpTestClient {
    pub name: &'static str,
    pub title: &'static str,
}

impl AcpTestClient {
    pub(crate) const fn new(name: &'static str, title: &'static str) -> Self {
        Self { name, title }
    }
}

pub(crate) struct AcpConnection {
    pub connection_id: u64,
    pub notifications_rx: mpsc::Receiver<Value>,
    pub initialize: AcpInitializeResult,
}

pub(crate) fn path_value(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) fn acp_request(id: u64, method: &str, params: Value) -> Value {
    serde_json::json!({ "id": id, "method": method, "params": params })
}

pub(crate) async fn handle_acp(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request: Value,
) -> Result<Value> {
    runtime
        .handle_incoming(connection_id, request)
        .await
        .context("ACP request response")
}

pub(crate) fn build_acp_test_runtime(
    data_root: &Path,
    provider: Arc<dyn ModelProviderSDK>,
    db_file: &str,
) -> Result<Arc<ServerRuntime>> {
    Ok(TestRuntime::new(provider)
        .with_test_model()
        .db_file(db_file)
        .runtime(data_root))
}

pub(crate) async fn connect_acp(
    runtime: &Arc<ServerRuntime>,
    transport: ClientTransportKind,
    client: AcpTestClient,
    request_id: u64,
) -> Result<AcpConnection> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(4096);
    let connection_id = runtime
        .register_connection(transport, notifications_tx)
        .await;
    let initialize_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": {
                        "name": client.name,
                        "title": client.title,
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    let response: AcpSuccessResponse<AcpInitializeResult> =
        serde_json::from_value(initialize_response)?;
    Ok(AcpConnection {
        connection_id,
        notifications_rx,
        initialize: response.result,
    })
}

pub(crate) async fn connect_acp_stdio(
    runtime: &Arc<ServerRuntime>,
    client: AcpTestClient,
) -> Result<AcpConnection> {
    connect_acp(runtime, ClientTransportKind::Stdio, client, 1).await
}

pub(crate) async fn connect_native_stdio(
    runtime: &Arc<ServerRuntime>,
    request_id: u64,
) -> Result<(u64, mpsc::Receiver<Value>)> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(4096);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "_meta": { "devo": { "protocol": "native" } }
                }
            }),
        )
        .await
        .context("Native initialize response")?;
    anyhow::ensure!(
        response.get("result").is_some(),
        "Native initialize failed: {response}"
    );
    Ok((connection_id, notifications_rx))
}

pub(crate) async fn create_acp_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &Path,
    request_id: u64,
    extra_params: Value,
) -> Result<AcpNewSessionResult> {
    let mut params = serde_json::json!({
        "cwd": path_value(cwd),
        "mcpServers": []
    });
    if let Some(extra) = extra_params.as_object() {
        for (key, value) in extra {
            params[key] = value.clone();
        }
    }
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "session/new",
                "params": params
            }),
        )
        .await
        .context("session/new response")?;
    if response.get("result").is_none() {
        anyhow::bail!("session/new error response: {response}");
    }
    serde_json::from_value::<AcpSuccessResponse<AcpNewSessionResult>>(response)
        .map(|success| success.result)
        .context("decode session/new success response")
}

pub(crate) async fn create_acp_session_id(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &Path,
    request_id: u64,
) -> Result<SessionId> {
    Ok(
        create_acp_session(runtime, connection_id, cwd, request_id, Value::Null)
            .await?
            .session_id,
    )
}

pub(crate) fn native_session_from_new(result: &AcpNewSessionResult) -> Result<Session> {
    let session = result
        .meta
        .as_ref()
        .and_then(|meta| meta.get(DEVO_SESSION_META))
        .cloned()
        .context("missing Native session")?;
    serde_json::from_value(session).context("decode Native session")
}

pub(crate) async fn list_acp_sessions(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    cwd: Option<&Path>,
    cursor: Option<String>,
) -> Result<AcpListSessionsResult> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "session/list",
                "params": {
                    "cwd": cwd.map(path_value),
                    "cursor": cursor
                }
            }),
        )
        .await
        .context("session/list response")?;
    if response.get("result").is_none() {
        panic!("session/list error response: {response}");
    }
    let response_for_error = response.clone();
    serde_json::from_value::<AcpSuccessResponse<AcpListSessionsResult>>(response)
        .map(|success| success.result)
        .map_err(|error| {
            anyhow::anyhow!("session/list decode failed: {error}; response={response_for_error}")
        })
}

pub(crate) async fn delete_acp_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: &SessionId,
) -> Result<AcpSuccessResponse<AcpDeleteSessionResult>> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "session/delete",
                "params": {
                    "sessionId": session_id
                }
            }),
        )
        .await
        .context("session/delete response")?;
    serde_json::from_value(response).context("decode session/delete response")
}

pub(crate) async fn wait_for_response<T>(
    notifications_rx: &mut mpsc::Receiver<Value>,
    request_id: u64,
) -> Result<AcpSuccessResponse<T>>
where
    T: serde::de::DeserializeOwned,
{
    timeout(ACP_TEST_TIMEOUT, async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("id") == Some(&serde_json::json!(request_id)) {
                return serde_json::from_value(value).context("decode ACP response");
            }
        }
        anyhow::bail!("notification channel closed before response {request_id}")
    })
    .await
    .with_context(|| format!("timed out waiting for response {request_id}"))?
}

pub(crate) async fn wait_for_available_commands_update(
    notifications_rx: &mut mpsc::Receiver<Value>,
    session_id: SessionId,
) -> Result<Vec<AcpAvailableCommand>> {
    timeout(ACP_TEST_TIMEOUT, async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") != Some(&serde_json::json!("session/update")) {
                continue;
            }
            let notification: AcpSessionNotification =
                serde_json::from_value(value["params"].clone())?;
            if notification.session_id != session_id {
                continue;
            }
            if let AcpSessionUpdate::AvailableCommandsUpdate {
                available_commands, ..
            } = notification.update
            {
                return Ok(available_commands);
            }
        }
        anyhow::bail!("notification channel closed before available commands update")
    })
    .await
    .context("timed out waiting for available commands update")?
}

pub(crate) fn assert_available_command_names(commands: &[AcpAvailableCommand]) -> Result<()> {
    let names = commands
        .iter()
        .map(|command| command.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["compact", "goal"]);
    Ok(())
}

pub(crate) fn assert_acp_slash_command_advertisement(commands: &[AcpAvailableCommand]) {
    assert_available_command_names(commands).expect("slash command names");
    assert_eq!(commands[0].input, None);
    assert_eq!(
        commands[1].input.as_ref().map(|input| input.hint.as_str()),
        Some("objective, pause, resume, or clear")
    );
}

pub(crate) async fn assert_acp_error_message(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    message: Value,
    expected_message: &str,
) -> Result<()> {
    let response = runtime
        .handle_incoming(connection_id, message)
        .await
        .context("ACP error response")?;
    let error: AcpErrorResponse = serde_json::from_value(response)?;
    assert_eq!(error.error.code, -32602);
    assert_eq!(error.error.message, expected_message);
    Ok(())
}

pub(crate) async fn assert_auth_required(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    message: Value,
) -> Result<()> {
    let response = runtime
        .handle_incoming(connection_id, message)
        .await
        .context("auth-required response")?;
    let error: AcpErrorResponse = serde_json::from_value(response)?;
    assert_eq!(error.error.code, -32000);
    assert_eq!(error.error.message, "Authentication required");
    assert_eq!(
        error.error.data,
        serde_json::json!({ "reason": "auth_required" })
    );
    Ok(())
}

pub(crate) async fn assert_removed_session_method(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    method: &str,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": method,
                "params": {}
            }),
        )
        .await
        .context("legacy session method response")?;
    assert_eq!(response["id"], serde_json::json!(request_id));
    assert_eq!(response["error"]["code"], serde_json::json!(-32601));
    assert_eq!(
        response["error"]["message"],
        serde_json::json!(format!("unknown ACP method: {method}"))
    );
    Ok(())
}

pub(crate) fn decode_native_session_meta(meta: &Option<devo_protocol::AcpMeta>) -> Result<Session> {
    let session = meta
        .as_ref()
        .and_then(|meta| meta.get(devo_protocol::DEVO_SESSION_META))
        .cloned()
        .context("missing Native session")?;
    serde_json::from_value(session).context("decode Native session")
}

pub(crate) fn stdio_mcp_server_value(name: &str, command: &Path) -> Value {
    serde_json::json!({
        "name": name,
        "command": path_value(command),
        "args": ["--stdio"],
        "env": [{ "name": "ACP_TEST", "value": "1" }]
    })
}

pub(crate) async fn wait_for_notification_method(
    notifications_rx: &mut mpsc::Receiver<Value>,
    method: &str,
) -> Result<Value> {
    timeout(ACP_TEST_TIMEOUT, async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") == Some(&serde_json::json!(method)) {
                return Ok(value);
            }
            if value.get("method") == Some(&serde_json::json!("session/update"))
                && value["params"]["_meta"]["devo/originalMethod"].as_str() == Some(method)
            {
                return Ok(value);
            }
        }
        anyhow::bail!("notification channel closed before {method}")
    })
    .await
    .with_context(|| format!("timed out waiting for {method}"))?
}

pub(crate) async fn send_session_prompt(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: SessionId,
    text: &str,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "session/prompt",
                "params": {
                    "sessionId": session_id,
                    "prompt": [{ "type": "text", "text": text }]
                }
            }),
        )
        .await;
    assert_eq!(response, None);
    Ok(())
}

pub(crate) async fn session_load(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: SessionId,
    cwd: &Path,
    extra_params: Value,
) -> Result<Value> {
    let mut params = serde_json::json!({
        "sessionId": session_id,
        "cwd": path_value(cwd),
        "mcpServers": []
    });
    if let Some(extra) = extra_params.as_object() {
        for (key, value) in extra {
            params[key] = value.clone();
        }
    }
    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "session/load",
                "params": params
            }),
        )
        .await
        .context("session/load response")
}

pub(crate) async fn wait_for_agent_text_update(
    notifications_rx: &mut mpsc::Receiver<Value>,
    session_id: SessionId,
) -> Result<String> {
    timeout(ACP_TEST_TIMEOUT, async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") != Some(&serde_json::json!("session/update")) {
                continue;
            }
            let notification: AcpSessionNotification =
                serde_json::from_value(value["params"].clone())?;
            if notification.session_id != session_id {
                continue;
            }
            if let AcpSessionUpdate::AgentMessageChunk {
                content: devo_protocol::AcpContentBlock::Text { text, .. },
                ..
            } = notification.update
            {
                return Ok(text);
            }
        }
        anyhow::bail!("notification channel closed before agent text update")
    })
    .await
    .context("timed out waiting for agent text update")?
}

pub(crate) async fn wait_for_replayed_history(
    notifications_rx: &mut mpsc::Receiver<Value>,
) -> Result<Vec<AcpSessionUpdate>> {
    timeout(ACP_TEST_TIMEOUT, async {
        let mut updates = Vec::new();
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") != Some(&serde_json::json!("session/update")) {
                continue;
            }
            let notification: AcpSessionNotification =
                serde_json::from_value(value["params"].clone())?;
            if notification.meta.is_some() {
                continue;
            }
            updates.push(notification.update);
            let has_user = updates
                .iter()
                .any(|update| matches!(update, AcpSessionUpdate::UserMessageChunk { .. }));
            let has_agent = updates
                .iter()
                .any(|update| matches!(update, AcpSessionUpdate::AgentMessageChunk { .. }));
            if has_user && has_agent {
                return Ok(updates);
            }
        }
        anyhow::bail!("notification channel closed before replayed history")
    })
    .await
    .context("timed out waiting for replayed history")?
}

pub(crate) async fn assert_no_replayed_history(
    notifications_rx: &mut mpsc::Receiver<Value>,
) -> Result<()> {
    let result = timeout(Duration::from_millis(100), async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") == Some(&serde_json::json!("session/update")) {
                let notification: AcpSessionNotification =
                    serde_json::from_value(value["params"].clone())?;
                if matches!(
                    notification.update,
                    AcpSessionUpdate::AvailableCommandsUpdate { .. }
                ) {
                    continue;
                }
                anyhow::bail!("unexpected session/update notification: {value}");
            }
        }
        Ok(())
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => Ok(()),
    }
}

pub(crate) async fn wait_for_prompt_update_and_response(
    notifications_rx: &mut mpsc::Receiver<Value>,
    request_id: u64,
    session_id: SessionId,
) -> Result<(
    Vec<AcpSessionUpdate>,
    Vec<AcpSessionUpdate>,
    AcpSuccessResponse<devo_protocol::AcpPromptResult>,
)> {
    // ACP prompt streams can pause while the runtime is busy. Use one overall
    // deadline instead of failing on a short gap between notifications.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut updates_before_response = Vec::new();
    let mut updates_after_response = Vec::new();
    let mut seen_messages = Vec::new();
    let response = loop {
        let Some(value) = tokio::time::timeout_at(deadline, notifications_rx.recv())
            .await
            .with_context(|| {
                format!(
                    "timed out waiting for prompt response {request_id}; seen={seen_messages:?}"
                )
            })?
        else {
            anyhow::bail!(
                "notification channel closed before prompt response {request_id}; seen={seen_messages:?}"
            );
        };
        seen_messages.push(value.clone());
        if value.get("method") == Some(&serde_json::json!("session/update")) {
            let notification: AcpSessionNotification =
                serde_json::from_value(value["params"].clone())
                    .context("decode ACP session/update notification")?;
            if notification.session_id == session_id && notification.meta.is_none() {
                updates_before_response.push(notification.update);
            }
            continue;
        }
        if value.get("id") == Some(&serde_json::json!(request_id)) {
            break serde_json::from_value(value).context("decode ACP prompt response")?;
        }
    };
    while let Ok(Some(value)) = timeout(Duration::from_millis(100), notifications_rx.recv()).await {
        if value.get("method") != Some(&serde_json::json!("session/update")) {
            continue;
        }
        let notification: AcpSessionNotification = serde_json::from_value(value["params"].clone())
            .context("decode ACP trailing session/update notification")?;
        if notification.session_id == session_id && notification.meta.is_none() {
            updates_after_response.push(notification.update);
        }
    }
    Ok((updates_before_response, updates_after_response, response))
}

pub(crate) async fn session_resume(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: SessionId,
    cwd: &Path,
    extra_params: Value,
) -> Result<Value> {
    let mut params = serde_json::json!({
        "sessionId": session_id,
        "cwd": path_value(cwd),
        "mcpServers": []
    });
    if let Some(extra) = extra_params.as_object() {
        for (key, value) in extra {
            params[key] = value.clone();
        }
    }
    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "session/resume",
                "params": params
            }),
        )
        .await
        .context("session/resume response")
}

pub(crate) struct SingleReplyProvider;

#[async_trait::async_trait]
impl ModelProviderSDK for SingleReplyProvider {
    async fn completion(
        &self,
        _request: devo_protocol::ModelRequest,
    ) -> Result<devo_protocol::ModelResponse> {
        Ok(devo_protocol::ModelResponse {
            id: "title-1".into(),
            content: vec![devo_protocol::ResponseContent::Text(
                "Generated ACP title".to_string(),
            )],
            stop_reason: Some(devo_protocol::StopReason::EndTurn),
            usage: devo_protocol::Usage::default(),
            metadata: devo_protocol::ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: devo_protocol::ModelRequest,
    ) -> Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<devo_protocol::StreamEvent>> + Send>>,
    > {
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(devo_protocol::StreamEvent::TextDelta {
                index: 0,
                text: "Hello from ACP lifecycle test.".into(),
            }),
            Ok(devo_protocol::StreamEvent::MessageDone {
                response: devo_protocol::ModelResponse {
                    id: "resp-1".into(),
                    content: vec![devo_protocol::ResponseContent::Text(
                        "Hello from ACP lifecycle test.".into(),
                    )],
                    stop_reason: Some(devo_protocol::StopReason::EndTurn),
                    usage: devo_protocol::Usage::default(),
                    metadata: devo_protocol::ResponseMetadata::default(),
                },
            }),
        ])))
    }

    fn name(&self) -> &str {
        "single-reply-acp-provider"
    }
}
