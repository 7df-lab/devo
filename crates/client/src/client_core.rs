use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use chrono::Utc;
use devo_protocol::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::timeout;

use crate::acp_fs::handle_acp_fs_request;
use crate::acp_permissions::AcpPendingPermissions;
use crate::acp_permissions::handle_acp_request_permission;
use crate::acp_permissions::resolve_acp_permission_response;
use crate::acp_terminal::AcpTerminalManager;
use crate::acp_terminal::handle_acp_terminal_request;
use crate::stdio::ACP_PROMPT_COMPLETED_NOTIFICATION_METHOD;
use crate::stdio::ACP_PROMPT_STARTED_NOTIFICATION_METHOD;
use crate::stdio::ServerNotificationMessage;

const SERVER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

type PendingResponses = Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>;

pub(crate) enum ClientWriteMessage {
    Json(serde_json::Value),
    Close,
}

#[derive(Clone)]
pub(crate) struct ClientWriter {
    tx: mpsc::UnboundedSender<ClientWriteMessage>,
}

impl ClientWriter {
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<ClientWriteMessage>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    fn send_value(&self, value: serde_json::Value) -> Result<()> {
        self.tx
            .send(ClientWriteMessage::Json(value))
            .map_err(|_| anyhow!("client writer is closed"))
    }

    fn send_serializable<T: Serialize>(&self, value: &T) -> Result<()> {
        let value = serde_json::to_value(value).context("serialize client payload")?;
        self.send_value(value)
    }

    pub(crate) fn close(&self) {
        let _ = self.tx.send(ClientWriteMessage::Close);
    }
}

#[derive(Clone)]
pub(crate) struct ServerClientReaderState {
    writer: ClientWriter,
    pending: PendingResponses,
    acp_pending_permissions: AcpPendingPermissions,
    acp_terminals: AcpTerminalManager,
    notifications_tx: mpsc::UnboundedSender<ServerNotificationMessage>,
}

pub(crate) struct ServerClientCore {
    writer: ClientWriter,
    pending: PendingResponses,
    acp_pending_permissions: AcpPendingPermissions,
    acp_terminals: AcpTerminalManager,
    acp_agent_capabilities: Option<AcpAgentCapabilities>,
    client_capabilities: AcpClientCapabilities,
    next_request_id: AtomicU64,
    notifications_rx: mpsc::UnboundedReceiver<ServerNotificationMessage>,
    notifications_tx: mpsc::UnboundedSender<ServerNotificationMessage>,
}

impl ServerClientCore {
    pub(crate) fn new(writer: ClientWriter, client_capabilities: AcpClientCapabilities) -> Self {
        let (notifications_tx, notifications_rx) = mpsc::unbounded_channel();
        Self {
            writer,
            pending: Arc::new(Mutex::new(HashMap::new())),
            acp_pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            acp_terminals: AcpTerminalManager::new(),
            acp_agent_capabilities: None,
            client_capabilities,
            next_request_id: AtomicU64::new(1),
            notifications_rx,
            notifications_tx,
        }
    }

    pub(crate) fn reader_state(&self) -> ServerClientReaderState {
        ServerClientReaderState {
            writer: self.writer.clone(),
            pending: Arc::clone(&self.pending),
            acp_pending_permissions: Arc::clone(&self.acp_pending_permissions),
            acp_terminals: self.acp_terminals.clone(),
            notifications_tx: self.notifications_tx.clone(),
        }
    }

    pub(crate) async fn initialize(&mut self) -> Result<InitializeResult> {
        let result: AcpInitializeResult = timeout(
            SERVER_RESPONSE_TIMEOUT,
            self.request(
                ACP_INITIALIZE_METHOD,
                AcpInitializeParams {
                    protocol_version: 1,
                    client_capabilities: self.client_capabilities.clone(),
                    client_info: Some(
                        AcpImplementation::new("devo", env!("CARGO_PKG_VERSION"))
                            .with_title("Devo"),
                    ),
                    meta: None,
                },
            ),
        )
        .await
        .context("timed out waiting for initialize response from server")??;
        self.acp_agent_capabilities = Some(result.agent_capabilities.clone());
        let meta = result.meta.as_ref();
        Ok(InitializeResult {
            server_name: result
                .agent_info
                .as_ref()
                .map(|info| info.name.clone())
                .unwrap_or_else(|| "devo-server".to_string()),
            server_version: result
                .agent_info
                .as_ref()
                .map(|info| info.version.clone())
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
            platform_family: meta
                .and_then(|meta| meta.get("devo/platformFamily"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(std::env::consts::FAMILY)
                .into(),
            platform_os: meta
                .and_then(|meta| meta.get("devo/platformOs"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(std::env::consts::OS)
                .into(),
            server_home: meta
                .and_then(|meta| meta.get("devo/serverHome"))
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_default(),
        })
    }

    pub(crate) async fn acp_terminal_output_snapshot(
        &self,
        terminal_id: &str,
    ) -> Result<AcpTerminalOutputResult> {
        self.acp_terminals
            .output(terminal_id)
            .await
            .map_err(anyhow::Error::msg)
    }

    pub(crate) async fn session_start(
        &mut self,
        params: SessionStartParams,
    ) -> Result<SessionStartResult> {
        let result: AcpNewSessionResult = self
            .request(
                ACP_SESSION_NEW_METHOD,
                AcpNewSessionParams {
                    cwd: params.cwd.clone(),
                    additional_directories: params.additional_directories.clone(),
                    mcp_servers: Vec::new(),
                    meta: None,
                },
            )
            .await?;
        let session = result
            .meta
            .as_ref()
            .and_then(|meta| meta.get(DEVO_SESSION_META))
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .context("decode session metadata from ACP session/new response")?
            .unwrap_or_else(|| acp_session_metadata_from_start_params(&params, result.session_id));
        Ok(SessionStartResult { session })
    }

    pub(crate) async fn session_resume(
        &mut self,
        params: SessionResumeParams,
    ) -> Result<SessionResumeResult> {
        let sessions = self.session_list().await?;
        let session = sessions
            .into_iter()
            .find(|session| session.session_id == params.session_id)
            .with_context(|| {
                format!(
                    "session {} not found for ACP session/resume",
                    params.session_id
                )
            })?;
        let result: AcpResumeSessionResult = self
            .request(
                ACP_SESSION_RESUME_METHOD,
                AcpResumeSessionParams {
                    session_id: params.session_id,
                    cwd: session.cwd.clone(),
                    additional_directories: session.additional_directories.clone(),
                    mcp_servers: Vec::new(),
                    meta: None,
                },
            )
            .await?;
        Ok(result
            .meta
            .as_ref()
            .and_then(|meta| meta.get(DEVO_SESSION_RESUME_META))
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .context("decode session resume metadata from ACP session/resume response")?
            .unwrap_or_else(|| SessionResumeResult {
                session,
                latest_turn: None,
                loaded_item_count: 0,
                history_items: Vec::new(),
                pending_texts: Vec::new(),
            }))
    }

    pub(crate) async fn session_list(&mut self) -> Result<Vec<SessionMetadata>> {
        let Some(capabilities) = self.acp_agent_capabilities.as_ref() else {
            bail!("ACP initialize must complete before session/list");
        };
        if capabilities.session_capabilities.list.is_none() {
            bail!("ACP agent does not advertise sessionCapabilities.list");
        }

        let mut cursor = None;
        let mut seen_cursors = HashSet::new();
        let mut sessions = Vec::new();
        loop {
            let result: AcpListSessionsResult = self
                .request(
                    ACP_SESSION_LIST_METHOD,
                    AcpListSessionsParams {
                        cwd: None,
                        cursor,
                        meta: None,
                    },
                )
                .await?;
            for session_info in result.sessions {
                let session = session_info
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.get(DEVO_SESSION_META))
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .context("decode session metadata from ACP session/list response")?
                    .unwrap_or_else(|| acp_session_metadata_from_session_info(&session_info));
                sessions.push(session);
            }
            let Some(next_cursor) = result.next_cursor else {
                break;
            };
            if !seen_cursors.insert(next_cursor.clone()) {
                bail!("ACP session/list returned a repeated nextCursor");
            }
            cursor = Some(next_cursor);
        }
        Ok(sessions)
    }

    pub(crate) async fn request_devo<P, R>(&mut self, method: &str, params: P) -> Result<R>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let method = devo_extension_method(method);
        self.request(&method, params).await
    }

    pub(crate) async fn request<P, R>(&mut self, method: &str, params: P) -> Result<R>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        let (response_tx, response_rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id, response_tx);
        let request = AcpClientRequest::new(serde_json::json!(request_id), method, params);
        if let Err(error) = self.writer.send_serializable(&request) {
            self.pending.lock().await.remove(&request_id);
            return Err(error);
        }

        let response = match timeout(SERVER_RESPONSE_TIMEOUT, response_rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                self.pending.lock().await.remove(&request_id);
                return Err(error)
                    .with_context(|| format!("server dropped response for request {request_id}"));
            }
            Err(error) => {
                self.pending.lock().await.remove(&request_id);
                return Err(error).with_context(|| {
                    format!("timed out waiting for server response to request {request_id}")
                });
            }
        };
        if response.get("error").is_some() {
            bail_server_error(&response)?;
        }
        let success: AcpSuccessResponse<R> =
            serde_json::from_value(response).context("decode success response from server")?;
        Ok(success.result)
    }

    pub(crate) async fn turn_start(&mut self, params: TurnStartParams) -> Result<TurnStartResult> {
        match self
            .request_devo::<_, TurnStartResult>("turn/start", params.clone())
            .await
        {
            Ok(result) => Ok(result),
            Err(error) if is_method_not_found_error(&error) => {
                self.turn_start_acp_prompt_detached(params).await?;
                Ok(TurnStartResult::Started {
                    turn_id: TurnId::new(),
                    status: TurnStatus::Running,
                    accepted_at: Utc::now(),
                })
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn approval_respond(&mut self, params: ApprovalResponseParams) -> Result<()> {
        if let Some((response, notification)) =
            resolve_acp_permission_response(&self.acp_pending_permissions, &params).await
        {
            self.writer.send_value(response)?;
            let _ = self.notifications_tx.send(notification);
            return Ok(());
        }
        bail!("no pending ACP permission request exists for approval response")
    }

    pub(crate) async fn request_user_input_respond(
        &mut self,
        params: RequestUserInputRespondParams,
    ) -> Result<()> {
        let _: serde_json::Value = self
            .request_devo("request_user_input/respond", params)
            .await?;
        Ok(())
    }

    pub(crate) async fn recv_notification(&mut self) -> Option<ServerNotificationMessage> {
        self.notifications_rx.recv().await
    }

    pub(crate) async fn recv_event(&mut self) -> Result<Option<(String, ServerEvent)>> {
        let Some(notification) = self.recv_notification().await else {
            return Ok(None);
        };
        let ServerNotificationMessage { method, params } = notification;
        let event = serde_json::from_value(params)
            .with_context(|| format!("failed to decode server event for method {method}"))?;
        Ok(Some((method, event)))
    }

    pub(crate) async fn shutdown(&self) {
        self.writer.close();
        self.acp_terminals.release_all().await;
    }

    async fn turn_start_acp_prompt_detached(&mut self, params: TurnStartParams) -> Result<()> {
        let session_id = params.session_id;
        let prompt = params
            .input
            .into_iter()
            .map(acp_content_block_from_input_item)
            .collect();
        let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        let (response_tx, response_rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id, response_tx);
        let request = AcpClientRequest::new(
            serde_json::json!(request_id),
            ACP_SESSION_PROMPT_METHOD,
            AcpPromptParams {
                session_id,
                prompt,
                meta: None,
            },
        );
        if let Err(error) = self.writer.send_serializable(&request) {
            self.pending.lock().await.remove(&request_id);
            return Err(error);
        }

        let _ = self.notifications_tx.send(ServerNotificationMessage {
            method: ACP_PROMPT_STARTED_NOTIFICATION_METHOD.to_string(),
            params: serde_json::json!({ "sessionId": session_id }),
        });
        let notifications_tx = self.notifications_tx.clone();
        tokio::spawn(async move {
            let params = match response_rx.await {
                Ok(response) if response.get("error").is_some() => serde_json::json!({
                    "sessionId": session_id,
                    "error": server_error_text(&response),
                }),
                Ok(response) => {
                    match serde_json::from_value::<AcpSuccessResponse<AcpPromptResult>>(response) {
                        Ok(success) => serde_json::json!({
                            "sessionId": session_id,
                            "stopReason": success.result.stop_reason,
                        }),
                        Err(error) => serde_json::json!({
                            "sessionId": session_id,
                            "error": format!("decode ACP prompt response: {error}"),
                        }),
                    }
                }
                Err(error) => serde_json::json!({
                    "sessionId": session_id,
                    "error": format!("server dropped ACP prompt response: {error}"),
                }),
            };
            let _ = notifications_tx.send(ServerNotificationMessage {
                method: ACP_PROMPT_COMPLETED_NOTIFICATION_METHOD.to_string(),
                params,
            });
        });
        Ok(())
    }
}

impl ServerClientReaderState {
    pub(crate) async fn handle_message(&self, message: serde_json::Value) {
        if let (Some(id), Some(method)) = (
            message.get("id").cloned(),
            message.get("method").and_then(serde_json::Value::as_str),
        ) {
            let params = message
                .get("params")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let state = self.clone();
            let method = method.to_string();
            tokio::spawn(async move {
                state.handle_client_request(id, &method, params).await;
            });
            return;
        }
        if let Some(id) = message.get("id").and_then(serde_json::Value::as_u64) {
            if let Some(tx) = self.pending.lock().await.remove(&id) {
                let _ = tx.send(message);
            }
            return;
        }
        if let Ok(notification) =
            serde_json::from_value::<NotificationEnvelope<serde_json::Value>>(message)
        {
            self.handle_notification(notification);
        }
    }

    pub(crate) async fn finish_reader(&self, transport_name: &'static str) {
        let abandoned_response_count = self.pending.lock().await.drain().count();
        if abandoned_response_count == 0 {
            tracing::warn!(transport_name, "server reader stopped");
        } else {
            tracing::warn!(
                transport_name,
                abandoned_response_count,
                "server reader stopped with pending responses"
            );
        }
        self.acp_terminals.release_all().await;
    }

    fn handle_notification(&self, notification: NotificationEnvelope<serde_json::Value>) {
        if notification.method == ACP_SESSION_UPDATE_METHOD
            && let Ok(acp_notification) =
                serde_json::from_value::<AcpSessionNotification>(notification.params.clone())
            && let Some((method, event)) = original_event_from_acp_notification(&acp_notification)
        {
            let _ = self.notifications_tx.send(ServerNotificationMessage {
                method,
                params: serde_json::to_value(event).expect("serialize original ACP event"),
            });
            return;
        }
        if let Some(method) = devo_extension_inner_method(&notification.method)
            && serde_json::from_value::<ServerEvent>(notification.params.clone()).is_ok()
        {
            let _ = self.notifications_tx.send(ServerNotificationMessage {
                method: method.to_string(),
                params: notification.params,
            });
            return;
        }
        let _ = self.notifications_tx.send(ServerNotificationMessage {
            method: notification.method,
            params: notification.params,
        });
    }

    async fn handle_client_request(
        self,
        id: serde_json::Value,
        method: &str,
        params: serde_json::Value,
    ) {
        let response = if method == ACP_SESSION_REQUEST_PERMISSION_METHOD {
            match handle_acp_request_permission(
                id.clone(),
                params,
                self.acp_pending_permissions,
                self.notifications_tx,
            )
            .await
            {
                Ok(()) => return,
                Err(message) => acp_client_error_response(id, -32603, message),
            }
        } else if matches!(
            method,
            ACP_FS_READ_TEXT_FILE_METHOD | ACP_FS_WRITE_TEXT_FILE_METHOD
        ) {
            match handle_acp_fs_request(id.clone(), method, params).await {
                Ok(response) => response,
                Err(message) => acp_client_error_response(id, -32603, message),
            }
        } else if matches!(
            method,
            ACP_TERMINAL_CREATE_METHOD
                | ACP_TERMINAL_OUTPUT_METHOD
                | ACP_TERMINAL_WAIT_FOR_EXIT_METHOD
                | ACP_TERMINAL_KILL_METHOD
                | ACP_TERMINAL_RELEASE_METHOD
        ) {
            match handle_acp_terminal_request(
                id.clone(),
                method,
                params,
                self.acp_terminals,
                self.notifications_tx,
            )
            .await
            {
                Ok(response) => response,
                Err(message) => acp_client_error_response(id, -32603, message),
            }
        } else {
            acp_client_error_response(id, -32601, format!("unknown client method {method}"))
        };
        if let Err(error) = self.writer.send_value(response) {
            tracing::warn!(%error, method, "failed to write ACP client response");
        }
    }
}

fn acp_client_error_response(
    id: serde_json::Value,
    code: i64,
    message: impl Into<String>,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into()
        }
    })
}

fn bail_server_error(response: &serde_json::Value) -> Result<()> {
    bail!("{}", server_error_text(response))
}

fn is_method_not_found_error(error: &anyhow::Error) -> bool {
    error.to_string().starts_with("server -32601:")
}

fn server_error_text(response: &serde_json::Value) -> String {
    if let Ok(error) = serde_json::from_value::<ErrorResponse>(response.clone()) {
        let data = if error.error.data.is_null() {
            String::new()
        } else {
            format!(" data={}", error.error.data)
        };
        return format!(
            "server {}: {}{}",
            format_protocol_error_code(&error.error.code),
            error.error.message,
            data
        );
    }
    format!(
        "server {}: {}",
        response
            .get("error")
            .and_then(|error| error.get("code"))
            .map(serde_json::Value::to_string)
            .unwrap_or_else(|| "unknown".to_string()),
        response
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown server error")
    )
}

fn format_protocol_error_code(code: &ProtocolErrorCode) -> &'static str {
    match code {
        ProtocolErrorCode::NotInitialized => "not_initialized",
        ProtocolErrorCode::InvalidParams => "invalid_params",
        ProtocolErrorCode::SessionNotFound => "session_not_found",
        ProtocolErrorCode::TurnNotFound => "turn_not_found",
        ProtocolErrorCode::TurnAlreadyRunning => "turn_already_running",
        ProtocolErrorCode::ApprovalNotFound => "approval_not_found",
        ProtocolErrorCode::PolicyDenied => "policy_denied",
        ProtocolErrorCode::ContextLimitExceeded => "context_limit_exceeded",
        ProtocolErrorCode::NoActiveTurn => "no_active_turn",
        ProtocolErrorCode::ExpectedTurnMismatch => "expected_turn_mismatch",
        ProtocolErrorCode::ActiveTurnNotSteerable => "active_turn_not_steerable",
        ProtocolErrorCode::EmptyInput => "empty_input",
        ProtocolErrorCode::AlreadyResolved => "already_resolved",
        ProtocolErrorCode::ParentSessionNotFound => "parent_session_not_found",
        ProtocolErrorCode::ForkTurnNotFound => "fork_turn_not_found",
        ProtocolErrorCode::ForkTurnNotStable => "fork_turn_not_stable",
        ProtocolErrorCode::PermissionDenied => "permission_denied",
        ProtocolErrorCode::WorkspaceUnavailable => "workspace_unavailable",
        ProtocolErrorCode::InheritedSegmentWriteFailed => "inherited_segment_write_failed",
        ProtocolErrorCode::ForkRetentionRequired => "fork_retention_required",
        ProtocolErrorCode::InvalidConfirmToken => "invalid_confirm_token",
        ProtocolErrorCode::UnsupportedDeletePolicy => "unsupported_delete_policy",
        ProtocolErrorCode::InheritedSegmentMaterializationFailed => {
            "inherited_segment_materialization_failed"
        }
        ProtocolErrorCode::ExpectedTargetMessageMismatch => "expected_target_message_mismatch",
        ProtocolErrorCode::OlderMessageRequiresFork => "older_message_requires_fork",
        ProtocolErrorCode::ActiveTurnEditRejected => "active_turn_edit_rejected",
        ProtocolErrorCode::InvalidContentParts => "invalid_content_parts",
        ProtocolErrorCode::InvalidMentions => "invalid_mentions",
        ProtocolErrorCode::WorkspaceRestoreFailedToStart => "workspace_restore_failed_to_start",
        ProtocolErrorCode::InternalError => "internal_error",
    }
}

fn acp_content_block_from_input_item(input: InputItem) -> AcpContentBlock {
    match input {
        InputItem::Text { text } => AcpContentBlock::text(text),
        InputItem::Skill { name, path } => AcpContentBlock::Text {
            annotations: None,
            text: format!("Skill {name}: {}", path.display()),
            meta: None,
        },
        InputItem::LocalImage { path } => AcpContentBlock::Text {
            annotations: None,
            text: format!("Image: {}", path.display()),
            meta: None,
        },
        InputItem::Mention { path, name } => AcpContentBlock::ResourceLink {
            annotations: None,
            uri: file_uri_from_path(&path),
            name: name.unwrap_or_else(|| path.clone()),
            title: None,
            description: None,
            mime_type: None,
            size: None,
            meta: None,
        },
    }
}

fn file_uri_from_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/') {
        format!("file://{normalized}")
    } else {
        format!("file:///{normalized}")
    }
}

fn acp_session_metadata_from_start_params(
    params: &SessionStartParams,
    session_id: SessionId,
) -> SessionMetadata {
    let now = Utc::now();
    SessionMetadata {
        session_id,
        cwd: params.cwd.clone(),
        additional_directories: params.additional_directories.clone(),
        created_at: now,
        updated_at: now,
        last_activity_at: now,
        title: params.title.clone(),
        title_state: acp_title_state(&params.title),
        parent_session_id: None,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
        ephemeral: params.ephemeral,
        model: params.model.clone(),
        model_binding_id: params.model_binding_id.clone(),
        reasoning_effort_selection: None,
        reasoning_effort: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_creation_tokens: 0,
        total_cache_read_tokens: 0,
        prompt_token_estimate: 0,
        last_query_total_tokens: 0,
        status: SessionRuntimeStatus::Idle,
    }
}

fn acp_session_metadata_from_session_info(session_info: &AcpSessionInfo) -> SessionMetadata {
    let updated_at = session_info
        .updated_at
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    SessionMetadata {
        session_id: session_info.session_id,
        cwd: session_info.cwd.clone(),
        additional_directories: session_info.additional_directories.clone(),
        created_at: updated_at,
        updated_at,
        last_activity_at: updated_at,
        title: session_info.title.clone(),
        title_state: acp_title_state(&session_info.title),
        parent_session_id: None,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
        ephemeral: false,
        model: None,
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_creation_tokens: 0,
        total_cache_read_tokens: 0,
        prompt_token_estimate: 0,
        last_query_total_tokens: 0,
        status: SessionRuntimeStatus::Idle,
    }
}

fn acp_title_state(title: &Option<String>) -> SessionTitleState {
    if title.is_some() {
        SessionTitleState::Provisional
    } else {
        SessionTitleState::Unset
    }
}
