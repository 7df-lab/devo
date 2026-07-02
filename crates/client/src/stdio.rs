//! Stdio transport for a spawned devo server process.
//!
//! The client writes newline-delimited JSON requests to child stdin and owns a
//! background stdout reader that demultiplexes responses by request id while
//! forwarding id-less messages as server notifications.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::acp_fs::handle_acp_fs_request;
use crate::acp_permissions::AcpPendingPermissions;
use crate::acp_permissions::handle_acp_request_permission;
use crate::acp_permissions::resolve_acp_permission_response;
use crate::acp_terminal::AcpTerminalManager;
use crate::acp_terminal::handle_acp_terminal_request;
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use chrono::Utc;
use devo_protocol::ACP_FS_READ_TEXT_FILE_METHOD;
use devo_protocol::ACP_FS_WRITE_TEXT_FILE_METHOD;
use devo_protocol::ACP_INITIALIZE_METHOD;
use devo_protocol::ACP_SESSION_LIST_METHOD;
use devo_protocol::ACP_SESSION_NEW_METHOD;
use devo_protocol::ACP_SESSION_PROMPT_METHOD;
use devo_protocol::ACP_SESSION_RESUME_METHOD;
use devo_protocol::ACP_SESSION_UPDATE_METHOD;
use devo_protocol::ACP_TERMINAL_CREATE_METHOD;
use devo_protocol::ACP_TERMINAL_KILL_METHOD;
use devo_protocol::ACP_TERMINAL_OUTPUT_METHOD;
use devo_protocol::ACP_TERMINAL_RELEASE_METHOD;
use devo_protocol::ACP_TERMINAL_WAIT_FOR_EXIT_METHOD;
use devo_protocol::AcpAgentCapabilities;
use devo_protocol::AcpClientRequest;
use devo_protocol::AcpContentBlock;
use devo_protocol::AcpImplementation;
use devo_protocol::AcpInitializeParams;
use devo_protocol::AcpInitializeResult;
use devo_protocol::AcpListSessionsParams;
use devo_protocol::AcpListSessionsResult;
use devo_protocol::AcpNewSessionParams;
use devo_protocol::AcpNewSessionResult;
use devo_protocol::AcpPromptParams;
use devo_protocol::AcpPromptResult;
use devo_protocol::AcpResumeSessionParams;
use devo_protocol::AcpResumeSessionResult;
use devo_protocol::AcpSessionInfo;
use devo_protocol::AcpSessionNotification;
use devo_protocol::AcpSuccessResponse;
use devo_protocol::AcpTerminalOutputResult;
use devo_protocol::AgentListParams;
use devo_protocol::AgentListResult;
use devo_protocol::ApprovalResponseParams;
use devo_protocol::CloseAgentParams;
use devo_protocol::CloseAgentResult;
use devo_protocol::CommandExecParams;
use devo_protocol::CommandExecResizeParams;
use devo_protocol::CommandExecResizeResult;
use devo_protocol::CommandExecResult;
use devo_protocol::CommandExecTerminateParams;
use devo_protocol::CommandExecTerminateResult;
use devo_protocol::CommandExecWriteParams;
use devo_protocol::CommandExecWriteResult;
use devo_protocol::DEVO_SESSION_RESUME_META;
use devo_protocol::ErrorResponse;
use devo_protocol::GoalClearParams;
use devo_protocol::GoalClearResult;
use devo_protocol::GoalCreateParams;
use devo_protocol::GoalCreateResult;
use devo_protocol::GoalSetParams;
use devo_protocol::GoalSetResult;
use devo_protocol::GoalSetStatusParams;
use devo_protocol::GoalSetStatusResult;
use devo_protocol::GoalStatusParams;
use devo_protocol::GoalStatusResult;
use devo_protocol::InitializeResult;
use devo_protocol::ModelCatalogParams;
use devo_protocol::ModelCatalogResult;
use devo_protocol::ModelSavedParams;
use devo_protocol::ModelSavedResult;
use devo_protocol::NotificationEnvelope;
use devo_protocol::ProtocolErrorCode;
use devo_protocol::ProviderValidateParams;
use devo_protocol::ProviderValidateResult;
use devo_protocol::ProviderVendorListParams;
use devo_protocol::ProviderVendorListResult;
use devo_protocol::ProviderVendorUpsertParams;
use devo_protocol::ProviderVendorUpsertResult;
use devo_protocol::ReferenceSearchCancelParams;
use devo_protocol::ReferenceSearchCancelResult;
use devo_protocol::ReferenceSearchStartParams;
use devo_protocol::ReferenceSearchStartResult;
use devo_protocol::ReferenceSearchUpdateParams;
use devo_protocol::ReferenceSearchUpdateResult;
use devo_protocol::RequestUserInputRespondParams;
use devo_protocol::ServerEvent;
use devo_protocol::SessionCompactParams;
use devo_protocol::SessionCompactResult;
use devo_protocol::SessionForkParams;
use devo_protocol::SessionForkResult;
use devo_protocol::SessionMetadata;
use devo_protocol::SessionMetadataUpdateParams;
use devo_protocol::SessionMetadataUpdateResult;
use devo_protocol::SessionPermissionsUpdateParams;
use devo_protocol::SessionPermissionsUpdateResult;
use devo_protocol::SessionResumeParams;
use devo_protocol::SessionResumeResult;
use devo_protocol::SessionRollbackParams;
use devo_protocol::SessionRollbackResult;
use devo_protocol::SessionRuntimeStatus;
use devo_protocol::SessionStartParams;
use devo_protocol::SessionStartResult;
use devo_protocol::SessionTitleState;
use devo_protocol::SessionTitleUpdateParams;
use devo_protocol::SessionTitleUpdateResult;
use devo_protocol::ShellCommandParams;
use devo_protocol::ShellCommandResult;
use devo_protocol::SkillChangedParams;
use devo_protocol::SkillChangedResult;
use devo_protocol::SkillListParams;
use devo_protocol::SkillListResult;
use devo_protocol::SkillSetEnabledParams;
use devo_protocol::SkillSetEnabledResult;
use devo_protocol::SpawnAgentParams;
use devo_protocol::SpawnAgentResult;
use devo_protocol::TurnId;
use devo_protocol::TurnInterruptParams;
use devo_protocol::TurnInterruptResult;
use devo_protocol::TurnStartParams;
use devo_protocol::TurnStartResult;
use devo_protocol::TurnStatus;
use devo_protocol::TurnSteerParams;
use devo_protocol::TurnSteerResult;
use devo_protocol::devo_extension_inner_method;
use devo_protocol::devo_extension_method;
use devo_protocol::original_event_from_acp_notification;
use serde::de::DeserializeOwned;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::ChildStderr;
use tokio::process::ChildStdin;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Duration;
use tokio::time::timeout;

const SERVER_CHILD_STDIN_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(100);
const SERVER_CHILD_EXIT_TIMEOUT: Duration = Duration::from_millis(500);
pub const ACP_PROMPT_STARTED_NOTIFICATION_METHOD: &str = "_devo/acp_prompt/started";
pub const ACP_PROMPT_COMPLETED_NOTIFICATION_METHOD: &str = "_devo/acp_prompt/completed";
pub use crate::acp_terminal::ACP_TERMINAL_OUTPUT_NOTIFICATION_METHOD;

type PendingResponses = Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>;

#[derive(Debug, Clone)]
pub struct StdioServerClientConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ServerNotificationMessage {
    pub method: String,
    pub params: serde_json::Value,
}

pub struct StdioServerClient {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: PendingResponses,
    acp_pending_permissions: AcpPendingPermissions,
    acp_terminals: AcpTerminalManager,
    acp_agent_capabilities: Option<AcpAgentCapabilities>,
    next_request_id: AtomicU64,
    notifications_rx: mpsc::UnboundedReceiver<ServerNotificationMessage>,
    notifications_tx: mpsc::UnboundedSender<ServerNotificationMessage>,
}

impl StdioServerClient {
    pub async fn spawn(config: StdioServerClientConfig) -> Result<Self> {
        tracing::info!(
            program = %config.program.display(),
            "spawning stdio server client"
        );
        let mut command = Command::new(&config.program);
        for arg in config.args {
            command.arg(arg);
        }
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.kill_on_drop(true);

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn {}", config.program.display()))?;
        let stdin = child.stdin.take().context("capture server stdin")?;
        let stdout = child.stdout.take().context("capture server stdout")?;
        let stderr = child.stderr.take().context("capture server stderr")?;
        let pending = Arc::new(Mutex::new(
            HashMap::<u64, oneshot::Sender<serde_json::Value>>::new(),
        ));
        let stdin = Arc::new(Mutex::new(stdin));
        let acp_pending_permissions = Arc::new(Mutex::new(HashMap::new()));
        let acp_terminals = AcpTerminalManager::new();
        let (notifications_tx, notifications_rx) = mpsc::unbounded_channel();

        tokio::spawn(run_stdout_reader(
            BufReader::new(stdout).lines(),
            Arc::clone(&pending),
            Arc::clone(&stdin),
            Arc::clone(&acp_pending_permissions),
            acp_terminals.clone(),
            notifications_tx.clone(),
        ));
        tokio::spawn(run_stderr_reader(BufReader::new(stderr).lines()));

        Ok(Self {
            child,
            stdin,
            pending,
            acp_pending_permissions,
            acp_terminals,
            acp_agent_capabilities: None,
            next_request_id: AtomicU64::new(1),
            notifications_rx,
            notifications_tx,
        })
    }

    pub async fn initialize(
        &mut self,
        client_capabilities: &devo_protocol::AcpClientCapabilities,
    ) -> Result<InitializeResult> {
        tracing::info!("initializing stdio server client");
        let result: AcpInitializeResult = timeout(
            Duration::from_secs(3),
            self.request(
                ACP_INITIALIZE_METHOD,
                AcpInitializeParams {
                    protocol_version: 1,
                    client_capabilities: client_capabilities.clone(),
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
        tracing::info!("stdio server client initialized");
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

    pub async fn acp_terminal_output_snapshot(
        &self,
        terminal_id: &str,
    ) -> Result<AcpTerminalOutputResult> {
        self.acp_terminals
            .output(terminal_id)
            .await
            .map_err(anyhow::Error::msg)
    }

    pub async fn session_start(
        &mut self,
        params: SessionStartParams,
    ) -> Result<SessionStartResult> {
        let cwd = params.cwd.clone();
        let additional_directories = params.additional_directories.clone();
        let result: AcpNewSessionResult = self
            .request(
                ACP_SESSION_NEW_METHOD,
                AcpNewSessionParams {
                    cwd,
                    additional_directories,
                    mcp_servers: Vec::new(),
                    meta: None,
                },
            )
            .await?;
        let session = result
            .meta
            .as_ref()
            .and_then(|meta| meta.get(devo_protocol::DEVO_SESSION_META))
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .context("decode session metadata from ACP session/new response")?
            .unwrap_or_else(|| acp_session_metadata_from_start_params(&params, result.session_id));
        Ok(SessionStartResult { session })
    }

    pub async fn session_resume(
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

    pub async fn session_list(&mut self) -> Result<Vec<SessionMetadata>> {
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
                    .and_then(|meta| meta.get(devo_protocol::DEVO_SESSION_META))
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

    pub async fn agent_list(&mut self, params: AgentListParams) -> Result<AgentListResult> {
        self.request_devo("agent/list", params).await
    }

    pub async fn agent_spawn(&mut self, params: SpawnAgentParams) -> Result<SpawnAgentResult> {
        self.request_devo("agent/spawn", params).await
    }

    pub async fn agent_close(&mut self, params: CloseAgentParams) -> Result<CloseAgentResult> {
        self.request_devo("agent/close", params).await
    }

    pub async fn session_title_update(
        &mut self,
        params: SessionTitleUpdateParams,
    ) -> Result<SessionTitleUpdateResult> {
        self.request_devo("session/title/update", params).await
    }

    pub async fn session_metadata_update(
        &mut self,
        params: SessionMetadataUpdateParams,
    ) -> Result<SessionMetadataUpdateResult> {
        self.request_devo("session/metadata/update", params).await
    }

    pub async fn session_permissions_update(
        &mut self,
        params: SessionPermissionsUpdateParams,
    ) -> Result<SessionPermissionsUpdateResult> {
        self.request_devo("session/permissions/update", params)
            .await
    }

    pub async fn session_compact(
        &mut self,
        params: SessionCompactParams,
    ) -> Result<SessionCompactResult> {
        self.request_devo("session/compact", params).await
    }

    pub async fn goal_create(&mut self, params: GoalCreateParams) -> Result<GoalCreateResult> {
        self.request_devo("goal/create", params).await
    }

    pub async fn goal_set(&mut self, params: GoalSetParams) -> Result<GoalSetResult> {
        self.request_devo("goal/set", params).await
    }

    pub async fn goal_status(&mut self, params: GoalStatusParams) -> Result<GoalStatusResult> {
        self.request_devo("goal/status", params).await
    }

    pub async fn goal_pause(&mut self, params: GoalSetStatusParams) -> Result<GoalSetStatusResult> {
        self.request_devo("goal/pause", params).await
    }

    pub async fn goal_resume(
        &mut self,
        params: GoalSetStatusParams,
    ) -> Result<GoalSetStatusResult> {
        self.request_devo("goal/resume", params).await
    }

    pub async fn goal_complete(
        &mut self,
        params: GoalSetStatusParams,
    ) -> Result<GoalSetStatusResult> {
        self.request_devo("goal/complete", params).await
    }

    pub async fn goal_clear(&mut self, params: GoalClearParams) -> Result<GoalClearResult> {
        self.request_devo("goal/clear", params).await
    }

    pub async fn session_fork(&mut self, params: SessionForkParams) -> Result<SessionForkResult> {
        self.request_devo("session/fork", params).await
    }

    pub async fn session_rollback(
        &mut self,
        params: SessionRollbackParams,
    ) -> Result<SessionRollbackResult> {
        self.request_devo("session/rollback", params).await
    }

    pub async fn skills_list(&mut self, params: SkillListParams) -> Result<SkillListResult> {
        self.request_devo("skills/list", params).await
    }

    pub async fn skills_changed(
        &mut self,
        params: SkillChangedParams,
    ) -> Result<SkillChangedResult> {
        self.request_devo("skills/changed", params).await
    }

    pub async fn skills_set_enabled(
        &mut self,
        params: SkillSetEnabledParams,
    ) -> Result<SkillSetEnabledResult> {
        self.request_devo("skills/set_enabled", params).await
    }

    pub async fn model_catalog(
        &mut self,
        params: ModelCatalogParams,
    ) -> Result<ModelCatalogResult> {
        self.request_devo("model/catalog", params).await
    }

    pub async fn model_saved(&mut self, params: ModelSavedParams) -> Result<ModelSavedResult> {
        self.request_devo("model/saved", params).await
    }

    pub async fn provider_vendor_list(
        &mut self,
        params: ProviderVendorListParams,
    ) -> Result<ProviderVendorListResult> {
        self.request_devo("provider/list", params).await
    }

    pub async fn provider_vendor_upsert(
        &mut self,
        params: ProviderVendorUpsertParams,
    ) -> Result<ProviderVendorUpsertResult> {
        self.request_devo("provider/upsert", params).await
    }

    pub async fn provider_validate(
        &mut self,
        params: ProviderValidateParams,
    ) -> Result<ProviderValidateResult> {
        self.request_devo("provider/validate", params).await
    }

    pub async fn command_exec(&mut self, params: CommandExecParams) -> Result<CommandExecResult> {
        self.request_devo("command/exec", params).await
    }

    pub async fn command_exec_write(
        &mut self,
        params: CommandExecWriteParams,
    ) -> Result<CommandExecWriteResult> {
        self.request_devo("command/exec/write", params).await
    }

    pub async fn command_exec_resize(
        &mut self,
        params: CommandExecResizeParams,
    ) -> Result<CommandExecResizeResult> {
        self.request_devo("command/exec/resize", params).await
    }

    pub async fn command_exec_terminate(
        &mut self,
        params: CommandExecTerminateParams,
    ) -> Result<CommandExecTerminateResult> {
        self.request_devo("command/exec/terminate", params).await
    }

    pub async fn turn_start(&mut self, params: TurnStartParams) -> Result<TurnStartResult> {
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

    pub async fn turn_shell_command(
        &mut self,
        params: ShellCommandParams,
    ) -> Result<ShellCommandResult> {
        self.request_devo("turn/shell_command", params).await
    }

    pub async fn turn_interrupt(
        &mut self,
        params: TurnInterruptParams,
    ) -> Result<TurnInterruptResult> {
        self.request_devo("turn/interrupt", params).await
    }

    pub async fn turn_steer(&mut self, params: TurnSteerParams) -> Result<TurnSteerResult> {
        self.request_devo("turn/steer", params).await
    }

    pub async fn approval_respond(&mut self, params: ApprovalResponseParams) -> Result<()> {
        if let Some((response, notification)) =
            resolve_acp_permission_response(&self.acp_pending_permissions, &params).await
        {
            write_acp_client_response(Arc::clone(&self.stdin), response)
                .await
                .context("write ACP permission response")?;
            let _ = self.notifications_tx.send(notification);
            return Ok(());
        }
        anyhow::bail!("no pending ACP permission request exists for approval response")
    }

    pub async fn request_user_input_respond(
        &mut self,
        params: RequestUserInputRespondParams,
    ) -> Result<()> {
        let _: serde_json::Value = self
            .request_devo("request_user_input/respond", params)
            .await?;
        Ok(())
    }

    pub async fn reference_search_start(
        &mut self,
        params: ReferenceSearchStartParams,
    ) -> Result<ReferenceSearchStartResult> {
        self.request_devo("search/start", params).await
    }

    pub async fn reference_search_update(
        &mut self,
        params: ReferenceSearchUpdateParams,
    ) -> Result<ReferenceSearchUpdateResult> {
        self.request_devo("search/update", params).await
    }

    pub async fn reference_search_cancel(
        &mut self,
        params: ReferenceSearchCancelParams,
    ) -> Result<ReferenceSearchCancelResult> {
        self.request_devo("search/cancel", params).await
    }

    pub async fn recv_notification(&mut self) -> Option<ServerNotificationMessage> {
        self.notifications_rx.recv().await
    }

    pub async fn recv_event(&mut self) -> Result<Option<(String, ServerEvent)>> {
        let Some(notification) = self.recv_notification().await else {
            return Ok(None);
        };
        let ServerNotificationMessage { method, params } = notification;
        let event = serde_json::from_value(params)
            .with_context(|| format!("failed to decode server event for method {method}"))?;
        Ok(Some((method, event)))
    }

    pub async fn shutdown(mut self) -> Result<()> {
        tracing::info!("stdio server client shutdown requested");
        let _ = timeout(
            SERVER_CHILD_STDIN_SHUTDOWN_TIMEOUT,
            self.stdin.lock().await.shutdown(),
        )
        .await;
        tracing::info!("stdio server stdin shutdown attempted");
        if let Err(error) = self.child.start_kill() {
            tracing::debug!(%error, "failed to start stdio server child kill");
        } else {
            tracing::info!("stdio server child kill requested");
        }
        match timeout(SERVER_CHILD_EXIT_TIMEOUT, self.child.wait()).await {
            Ok(Ok(status)) => {
                tracing::info!(?status, "stdio server child exited during shutdown");
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "failed to wait for stdio server child exit");
            }
            Err(_elapsed) => {
                tracing::debug!("timed out waiting for stdio server child exit");
            }
        }
        self.acp_terminals.release_all().await;
        Ok(())
    }

    async fn request<P, R>(&mut self, method: &str, params: P) -> Result<R>
    where
        P: serde::Serialize,
        R: DeserializeOwned,
    {
        let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        tracing::debug!(request_id, method, "sending client request");
        let (response_tx, response_rx) = oneshot::channel();
        // The stdout reader owns response routing. Keep the sender in this map
        // only while the request can still be completed by an incoming response.
        self.pending.lock().await.insert(request_id, response_tx);
        let write_result = self
            .write_json(&AcpClientRequest::new(
                serde_json::json!(request_id),
                method,
                params,
            ))
            .await;
        if let Err(error) = write_result {
            self.pending.lock().await.remove(&request_id);
            return Err(error);
        }

        let response = match timeout(Duration::from_secs(10), response_rx).await {
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
        tracing::debug!(request_id, method, "received client response");
        if response.get("error").is_some() {
            bail_server_error(&response)?;
        }
        let success: AcpSuccessResponse<R> =
            serde_json::from_value(response).context("decode success response from server")?;
        Ok(success.result)
    }

    async fn request_devo<P, R>(&mut self, method: &str, params: P) -> Result<R>
    where
        P: serde::Serialize,
        R: DeserializeOwned,
    {
        let method = devo_extension_method(method);
        self.request(&method, params).await
    }

    async fn turn_start_acp_prompt_detached(&mut self, params: TurnStartParams) -> Result<()> {
        let session_id = params.session_id;
        let prompt = params
            .input
            .into_iter()
            .map(acp_content_block_from_input_item)
            .collect();
        let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        tracing::debug!(
            request_id,
            method = ACP_SESSION_PROMPT_METHOD,
            "sending ACP prompt request"
        );
        let (response_tx, response_rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id, response_tx);
        let write_result = self
            .write_json(&AcpClientRequest::new(
                serde_json::json!(request_id),
                ACP_SESSION_PROMPT_METHOD,
                AcpPromptParams {
                    session_id,
                    prompt,
                    meta: None,
                },
            ))
            .await;
        if let Err(error) = write_result {
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

    async fn write_json<T>(&mut self, value: &T) -> Result<()>
    where
        T: serde::Serialize,
    {
        let mut line = serde_json::to_vec(value).context("serialize client payload")?;
        line.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(&line)
            .await
            .context("write client payload")?;
        stdin.flush().await.context("flush client payload")?;
        Ok(())
    }
}

async fn run_stdout_reader<R>(
    mut lines: tokio::io::Lines<R>,
    pending: PendingResponses,
    stdin: Arc<Mutex<ChildStdin>>,
    acp_pending_permissions: AcpPendingPermissions,
    acp_terminals: AcpTerminalManager,
    notifications_tx: mpsc::UnboundedSender<ServerNotificationMessage>,
) where
    R: AsyncBufRead + Unpin,
{
    while let Ok(Some(line)) = lines.next_line().await {
        match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(message) => {
                if let (Some(id), Some(method)) = (
                    message.get("id").cloned(),
                    message.get("method").and_then(serde_json::Value::as_str),
                ) {
                    let stdin = Arc::clone(&stdin);
                    let acp_pending_permissions = Arc::clone(&acp_pending_permissions);
                    let acp_terminals = acp_terminals.clone();
                    let notifications_tx = notifications_tx.clone();
                    let method = method.to_string();
                    let params = message
                        .get("params")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({}));
                    tokio::spawn(async move {
                        handle_acp_client_request(
                            stdin,
                            acp_pending_permissions,
                            acp_terminals,
                            notifications_tx,
                            id,
                            &method,
                            params,
                        )
                        .await;
                    });
                } else if let Some(id) = message.get("id").and_then(serde_json::Value::as_u64) {
                    // Responses are consumed by the request future waiting on the
                    // matching oneshot. Notifications intentionally bypass that
                    // map so event consumers can drain them independently.
                    if let Some(tx) = pending.lock().await.remove(&id) {
                        let _ = tx.send(message);
                    }
                } else if let Ok(notification) =
                    serde_json::from_value::<NotificationEnvelope<serde_json::Value>>(message)
                {
                    if notification.method == ACP_SESSION_UPDATE_METHOD
                        && let Ok(acp_notification) = serde_json::from_value::<AcpSessionNotification>(
                            notification.params.clone(),
                        )
                        && let Some((method, event)) =
                            original_event_from_acp_notification(&acp_notification)
                    {
                        let _ = notifications_tx.send(ServerNotificationMessage {
                            method,
                            params: serde_json::to_value(event)
                                .expect("serialize original ACP event"),
                        });
                        continue;
                    }
                    if let Some(method) = devo_extension_inner_method(&notification.method)
                        && serde_json::from_value::<ServerEvent>(notification.params.clone())
                            .is_ok()
                    {
                        let _ = notifications_tx.send(ServerNotificationMessage {
                            method: method.to_string(),
                            params: notification.params,
                        });
                        continue;
                    }
                    let event_seq = notification
                        .params
                        .get("context")
                        .and_then(|context| context.get("seq"))
                        .and_then(serde_json::Value::as_u64);
                    let item_id = notification_item_id(&notification.params);
                    let assistant_delta =
                        notification_assistant_delta(&notification.method, &notification.params);
                    let delta_len = assistant_delta.map(str::len);
                    let assistant_token_text =
                        assistant_delta.and_then(assistant_token_log_preview);
                    if let Some(assistant_token_text) = assistant_token_text.as_deref() {
                        tracing::debug!(
                            stream_elapsed_ms = stream_trace_elapsed_ms(),
                            method = %notification.method,
                            event_seq,
                            item_id = ?item_id,
                            delta_len = ?delta_len,
                            assistant_token_text,
                            "stdio client received server notification"
                        );
                    } else {
                        tracing::debug!(
                            stream_elapsed_ms = stream_trace_elapsed_ms(),
                            method = %notification.method,
                            event_seq,
                            item_id = ?item_id,
                            delta_len = ?delta_len,
                            "stdio client received server notification"
                        );
                    }
                    let _ = notifications_tx.send(ServerNotificationMessage {
                        method: notification.method,
                        params: notification.params,
                    });
                }
            }
            Err(_) => {
                tracing::warn!(line = %line, "failed to parse JSON from server stdout");
            }
        }
    }
    // Dropping the pending response senders wakes request futures instead of
    // leaving them blocked until their timeout when the child closes stdout.
    let abandoned_response_count = pending.lock().await.drain().count();
    if abandoned_response_count == 0 {
        tracing::warn!("server stdout reader stopped");
    } else {
        tracing::warn!(
            abandoned_response_count,
            "server stdout reader stopped with pending responses"
        );
    }
    acp_terminals.release_all().await;
}

async fn handle_acp_client_request(
    stdin: Arc<Mutex<ChildStdin>>,
    acp_pending_permissions: AcpPendingPermissions,
    acp_terminals: AcpTerminalManager,
    notifications_tx: mpsc::UnboundedSender<ServerNotificationMessage>,
    id: serde_json::Value,
    method: &str,
    params: serde_json::Value,
) {
    if method == "session/request_permission" {
        let response = match handle_acp_request_permission(
            id.clone(),
            params,
            acp_pending_permissions,
            notifications_tx,
        )
        .await
        {
            Ok(()) => return,
            Err(message) => acp_client_error_response(id, -32603, message),
        };
        if let Err(error) = write_acp_client_response(stdin, response).await {
            tracing::warn!(%error, method, "failed to write ACP client response");
        }
        return;
    }
    if matches!(
        method,
        ACP_FS_READ_TEXT_FILE_METHOD | ACP_FS_WRITE_TEXT_FILE_METHOD
    ) {
        let response = match handle_acp_fs_request(id.clone(), method, params).await {
            Ok(response) => response,
            Err(message) => acp_client_error_response(id, -32603, message),
        };
        if let Err(error) = write_acp_client_response(stdin, response).await {
            tracing::warn!(%error, method, "failed to write ACP client response");
        }
        return;
    }
    if matches!(
        method,
        ACP_TERMINAL_CREATE_METHOD
            | ACP_TERMINAL_OUTPUT_METHOD
            | ACP_TERMINAL_WAIT_FOR_EXIT_METHOD
            | ACP_TERMINAL_KILL_METHOD
            | ACP_TERMINAL_RELEASE_METHOD
    ) {
        let response = match handle_acp_terminal_request(
            id.clone(),
            method,
            params,
            acp_terminals,
            notifications_tx,
        )
        .await
        {
            Ok(response) => response,
            Err(message) => acp_client_error_response(id, -32603, message),
        };
        if let Err(error) = write_acp_client_response(stdin, response).await {
            tracing::warn!(%error, method, "failed to write ACP client response");
        }
        return;
    }
    let response = acp_client_error_response(id, -32601, format!("unknown client method {method}"));
    if let Err(error) = write_acp_client_response(stdin, response).await {
        tracing::warn!(%error, method, "failed to write ACP client response");
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

async fn write_acp_client_response(
    stdin: Arc<Mutex<ChildStdin>>,
    value: serde_json::Value,
) -> Result<()> {
    let mut line = serde_json::to_vec(&value).context("serialize ACP client response")?;
    line.push(b'\n');
    let mut stdin = stdin.lock().await;
    stdin
        .write_all(&line)
        .await
        .context("write ACP client response")?;
    stdin.flush().await.context("flush ACP client response")?;
    Ok(())
}

async fn run_stderr_reader(mut lines: tokio::io::Lines<BufReader<ChildStderr>>) {
    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            tracing::warn!(server_stderr = %trimmed, "server child stderr");
        }
    }
    tracing::warn!("server stderr reader stopped");
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

fn bail_server_error(response: &serde_json::Value) -> Result<()> {
    anyhow::bail!("{}", server_error_text(response))
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
        server_error_code(response),
        server_error_message(response)
    )
}

fn server_error_code(response: &serde_json::Value) -> String {
    response
        .get("error")
        .and_then(|error| error.get("code"))
        .map(serde_json::Value::to_string)
        .unwrap_or_else(|| "unknown".to_string())
}

fn server_error_message(response: &serde_json::Value) -> &str {
    response
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown server error")
}

fn acp_content_block_from_input_item(input: devo_protocol::InputItem) -> AcpContentBlock {
    match input {
        devo_protocol::InputItem::Text { text } => AcpContentBlock::text(text),
        devo_protocol::InputItem::Skill { name, path } => AcpContentBlock::Text {
            annotations: None,
            text: format!("Skill {name}: {}", path.display()),
            meta: None,
        },
        devo_protocol::InputItem::LocalImage { path } => AcpContentBlock::Text {
            annotations: None,
            text: format!("Image: {}", path.display()),
            meta: None,
        },
        devo_protocol::InputItem::Mention { path, name } => AcpContentBlock::ResourceLink {
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

fn stream_trace_elapsed_ms() -> u128 {
    static STREAM_TRACE_START: OnceLock<Instant> = OnceLock::new();
    STREAM_TRACE_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}

fn notification_item_id(params: &serde_json::Value) -> Option<&str> {
    params
        .get("context")
        .and_then(|context| context.get("item_id"))
        .and_then(serde_json::Value::as_str)
}

fn notification_assistant_delta<'a>(
    method: &str,
    params: &'a serde_json::Value,
) -> Option<&'a str> {
    (method == "item/agentMessage/delta")
        .then(|| params.get("delta")?.as_str())
        .flatten()
}

fn assistant_token_log_preview(text: &str) -> Option<String> {
    assistant_token_logging_enabled().then(|| {
        let max_chars = assistant_token_log_max_chars();
        format_assistant_token_log_preview(text, max_chars)
    })
}

fn assistant_token_logging_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("DEVO_LOG_ASSISTANT_TOKEN_TEXT")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.as_str(),
                    "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
                )
            })
    })
}

fn assistant_token_log_max_chars() -> usize {
    static ASSISTANT_TOKEN_LOG_MAX_CHARS: OnceLock<usize> = OnceLock::new();
    *ASSISTANT_TOKEN_LOG_MAX_CHARS.get_or_init(|| {
        std::env::var("DEVO_ASSISTANT_TOKEN_LOG_MAX_CHARS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(512)
    })
}

fn format_assistant_token_log_preview(text: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(1);
    let mut preview = String::with_capacity(text.len().min(max_chars));
    let mut chars = text.chars();
    for ch in chars.by_ref().take(max_chars) {
        preview.extend(ch.escape_default());
    }
    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
}

fn acp_session_metadata_from_start_params(
    params: &SessionStartParams,
    session_id: devo_protocol::SessionId,
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn initialize_uses_configured_client_capabilities() {
        let (child, stdin, stdout) = request_capture_child_for_turn_start_test().await;
        let stdin = Arc::new(Mutex::new(stdin));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let acp_pending_permissions = Arc::new(Mutex::new(HashMap::new()));
        let acp_terminals = AcpTerminalManager::new();
        let (notifications_tx, notifications_rx) = mpsc::unbounded_channel();
        let client_capabilities = devo_protocol::AcpClientCapabilities {
            fs: devo_protocol::AcpFileSystemCapabilities {
                read_text_file: true,
                write_text_file: false,
                meta: None,
            },
            terminal: true,
            meta: None,
        };
        let mut client = StdioServerClient {
            child,
            stdin,
            pending: Arc::clone(&pending),
            acp_pending_permissions,
            acp_terminals,
            acp_agent_capabilities: None,
            next_request_id: AtomicU64::new(1),
            notifications_rx,
            notifications_tx,
        };
        let mut stdout_lines = BufReader::new(stdout).lines();
        let expected_capabilities =
            serde_json::to_value(&client_capabilities).expect("serialize client capabilities");

        let initialize = tokio::spawn(async move {
            let result = client.initialize(&client_capabilities).await;
            (result, client)
        });

        let request = read_request_line(&mut stdout_lines).await;
        assert_eq!(request["method"], ACP_INITIALIZE_METHOD);
        assert_eq!(request["params"]["protocolVersion"], serde_json::json!(1));
        assert_eq!(
            request["params"]["clientCapabilities"],
            expected_capabilities
        );
        pending
            .lock()
            .await
            .remove(&1)
            .expect("initialize has pending response")
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "protocolVersion": 1
                }
            }))
            .expect("send initialize response");

        let (result, mut client) = initialize.await.expect("initialize task joins");
        result.expect("initialize response is accepted");
        let _ = client.child.start_kill();
        let _ = client.child.wait().await;
    }

    #[tokio::test]
    async fn session_start_accepts_standard_acp_response_without_devo_metadata() {
        let (child, stdin, stdout) = request_capture_child_for_turn_start_test().await;
        let stdin = Arc::new(Mutex::new(stdin));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let acp_pending_permissions = Arc::new(Mutex::new(HashMap::new()));
        let acp_terminals = AcpTerminalManager::new();
        let (notifications_tx, notifications_rx) = mpsc::unbounded_channel();
        let mut client = StdioServerClient {
            child,
            stdin,
            pending: Arc::clone(&pending),
            acp_pending_permissions,
            acp_terminals,
            acp_agent_capabilities: None,
            next_request_id: AtomicU64::new(1),
            notifications_rx,
            notifications_tx,
        };
        let cwd = std::env::current_dir().expect("current dir");
        let additional_directory = cwd.join("shared");
        let session_id = devo_protocol::SessionId::new();
        let params = SessionStartParams {
            cwd: cwd.clone(),
            additional_directories: vec![additional_directory.clone()],
            ephemeral: true,
            title: Some("ACP session".to_string()),
            model: Some("test-model".to_string()),
            model_binding_id: Some("binding".to_string()),
        };
        let mut stdout_lines = BufReader::new(stdout).lines();

        let session_start = tokio::spawn(async move {
            let result = client.session_start(params).await;
            (result, client)
        });

        let request = read_request_line(&mut stdout_lines).await;
        assert_eq!(request["method"], ACP_SESSION_NEW_METHOD);
        pending
            .lock()
            .await
            .remove(&1)
            .expect("session/new has pending response")
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "sessionId": session_id
                }
            }))
            .expect("send session/new response");

        let (result, mut client) = session_start.await.expect("session_start task joins");
        let session = result
            .expect("standard ACP session/new response is accepted")
            .session;
        let expected = SessionMetadata {
            session_id,
            cwd,
            additional_directories: vec![additional_directory],
            created_at: session.created_at,
            updated_at: session.updated_at,
            last_activity_at: session.updated_at,
            title: Some("ACP session".to_string()),
            title_state: SessionTitleState::Provisional,
            parent_session_id: None,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
            ephemeral: true,
            model: Some("test-model".to_string()),
            model_binding_id: Some("binding".to_string()),
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
        };
        assert_eq!(session, expected);

        let _ = client.child.start_kill();
        let _ = client.child.wait().await;
    }

    #[tokio::test]
    async fn session_list_accepts_standard_acp_sessions_without_devo_metadata() {
        let (child, stdin, stdout) = request_capture_child_for_turn_start_test().await;
        let stdin = Arc::new(Mutex::new(stdin));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let acp_pending_permissions = Arc::new(Mutex::new(HashMap::new()));
        let acp_terminals = AcpTerminalManager::new();
        let (notifications_tx, notifications_rx) = mpsc::unbounded_channel();
        let mut client = StdioServerClient {
            child,
            stdin,
            pending: Arc::clone(&pending),
            acp_pending_permissions,
            acp_terminals,
            acp_agent_capabilities: Some(AcpAgentCapabilities {
                session_capabilities: devo_protocol::AcpSessionCapabilities {
                    list: Some(devo_protocol::AcpSessionListCapabilities::default()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            next_request_id: AtomicU64::new(1),
            notifications_rx,
            notifications_tx,
        };
        let cwd = std::env::current_dir().expect("current dir");
        let additional_directory = cwd.join("shared");
        let session_id = devo_protocol::SessionId::new();
        let updated_at = "2026-06-20T00:00:00Z";
        let expected_timestamp = chrono::DateTime::parse_from_rfc3339(updated_at)
            .expect("parse updatedAt")
            .with_timezone(&Utc);
        let mut stdout_lines = BufReader::new(stdout).lines();

        let session_list = tokio::spawn(async move {
            let result = client.session_list().await;
            (result, client)
        });

        let request = read_request_line(&mut stdout_lines).await;
        assert_eq!(
            request,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "session/list",
                "params": {}
            })
        );
        pending
            .lock()
            .await
            .remove(&1)
            .expect("session/list has pending response")
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "sessions": [
                        {
                            "sessionId": session_id,
                            "cwd": cwd,
                            "title": "External ACP",
                            "updatedAt": updated_at,
                            "additionalDirectories": [additional_directory]
                        }
                    ]
                }
            }))
            .expect("send session/list response");

        let (result, mut client) = session_list.await.expect("session_list task joins");
        assert_eq!(
            result.expect("standard ACP session/list response is accepted"),
            vec![SessionMetadata {
                session_id,
                cwd,
                additional_directories: vec![additional_directory],
                created_at: expected_timestamp,
                updated_at: expected_timestamp,
                last_activity_at: expected_timestamp,
                title: Some("External ACP".to_string()),
                title_state: SessionTitleState::Provisional,
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
            }]
        );

        let _ = client.child.start_kill();
        let _ = client.child.wait().await;
    }

    #[tokio::test]
    async fn session_resume_accepts_standard_acp_response_without_devo_metadata() {
        let (child, stdin, stdout) = request_capture_child_for_turn_start_test().await;
        let stdin = Arc::new(Mutex::new(stdin));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let acp_pending_permissions = Arc::new(Mutex::new(HashMap::new()));
        let acp_terminals = AcpTerminalManager::new();
        let (notifications_tx, notifications_rx) = mpsc::unbounded_channel();
        let mut client = StdioServerClient {
            child,
            stdin,
            pending: Arc::clone(&pending),
            acp_pending_permissions,
            acp_terminals,
            acp_agent_capabilities: Some(AcpAgentCapabilities {
                session_capabilities: devo_protocol::AcpSessionCapabilities {
                    list: Some(devo_protocol::AcpSessionListCapabilities::default()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            next_request_id: AtomicU64::new(1),
            notifications_rx,
            notifications_tx,
        };
        let cwd = std::env::current_dir().expect("current dir");
        let additional_directory = cwd.join("shared");
        let session_id = devo_protocol::SessionId::new();
        let updated_at = "2026-06-20T00:00:00Z";
        let expected_timestamp = chrono::DateTime::parse_from_rfc3339(updated_at)
            .expect("parse updatedAt")
            .with_timezone(&Utc);
        let expected_session = SessionMetadata {
            session_id,
            cwd: cwd.clone(),
            additional_directories: vec![additional_directory.clone()],
            created_at: expected_timestamp,
            updated_at: expected_timestamp,
            last_activity_at: expected_timestamp,
            title: Some("External ACP".to_string()),
            title_state: SessionTitleState::Provisional,
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
        };
        let mut stdout_lines = BufReader::new(stdout).lines();

        let session_resume = tokio::spawn(async move {
            let result = client
                .session_resume(SessionResumeParams { session_id })
                .await;
            (result, client)
        });

        let list_request = read_request_line(&mut stdout_lines).await;
        assert_eq!(list_request["method"], ACP_SESSION_LIST_METHOD);
        pending
            .lock()
            .await
            .remove(&1)
            .expect("session/list has pending response")
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "sessions": [
                        {
                            "sessionId": session_id,
                            "cwd": cwd,
                            "title": "External ACP",
                            "updatedAt": updated_at,
                            "additionalDirectories": [additional_directory]
                        }
                    ]
                }
            }))
            .expect("send session/list response");

        let resume_request = read_request_line(&mut stdout_lines).await;
        assert_eq!(
            resume_request,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "session/resume",
                "params": {
                    "sessionId": session_id,
                    "cwd": cwd,
                    "additionalDirectories": [additional_directory],
                    "mcpServers": []
                }
            })
        );
        pending
            .lock()
            .await
            .remove(&2)
            .expect("session/resume has pending response")
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {}
            }))
            .expect("send session/resume response");

        let (result, mut client) = session_resume.await.expect("session_resume task joins");
        assert_eq!(
            result.expect("standard ACP session/resume response is accepted"),
            SessionResumeResult {
                session: expected_session,
                latest_turn: None,
                loaded_item_count: 0,
                history_items: Vec::new(),
                pending_texts: Vec::new(),
            }
        );

        let _ = client.child.start_kill();
        let _ = client.child.wait().await;
    }

    #[tokio::test]
    async fn turn_start_sends_devo_extension_with_full_params() {
        let (child, stdin, stdout) = request_capture_child_for_turn_start_test().await;
        let stdin = Arc::new(Mutex::new(stdin));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let acp_pending_permissions = Arc::new(Mutex::new(HashMap::new()));
        let acp_terminals = AcpTerminalManager::new();
        let (notifications_tx, notifications_rx) = mpsc::unbounded_channel();
        let mut client = StdioServerClient {
            child,
            stdin,
            pending: Arc::clone(&pending),
            acp_pending_permissions,
            acp_terminals,
            acp_agent_capabilities: None,
            next_request_id: AtomicU64::new(1),
            notifications_rx,
            notifications_tx,
        };
        let params = TurnStartParams {
            session_id: devo_protocol::SessionId::new(),
            input: vec![devo_protocol::InputItem::Text {
                text: "research this".to_string(),
            }],
            model: Some("test-model".to_string()),
            model_binding_id: Some("test-binding".to_string()),
            reasoning_effort_selection: Some("high".to_string()),
            sandbox: Some("workspace-write".to_string()),
            approval_policy: Some("on-request".to_string()),
            cwd: Some(PathBuf::from("workspace")),
            collaboration_mode: devo_protocol::CollaborationMode::Plan,
            execution_mode: devo_protocol::TurnExecutionMode::Research,
        };
        let expected_params = serde_json::to_value(&params).expect("serialize turn params");
        let mut stdout_lines = BufReader::new(stdout).lines();

        let turn_start = tokio::spawn(async move {
            let result = client.turn_start(params).await;
            (result, client)
        });
        let request_line = timeout(Duration::from_secs(5), stdout_lines.next_line())
            .await
            .expect("read request line before timeout")
            .expect("read request line")
            .expect("request line is present");
        let request =
            serde_json::from_str::<serde_json::Value>(&request_line).expect("request line is JSON");

        assert_eq!(
            request,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "_devo/turn/start",
                "params": expected_params,
            })
        );

        let response_tx = pending
            .lock()
            .await
            .remove(&1)
            .expect("turn_start has pending response");
        response_tx
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "disposition": "started",
                    "turn_id": devo_protocol::TurnId::new(),
                    "status": "Running",
                    "accepted_at": "2026-06-20T00:00:00Z"
                }
            }))
            .expect("send turn_start response");
        let (result, mut client) = turn_start.await.expect("turn_start task joins");
        result.expect("turn_start response is accepted");

        let _ = client.child.start_kill();
        let _ = client.child.wait().await;
    }

    #[tokio::test]
    async fn turn_start_falls_back_to_acp_prompt_with_lifecycle_notifications() {
        let (child, stdin, stdout) = request_capture_child_for_turn_start_test().await;
        let stdin = Arc::new(Mutex::new(stdin));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let acp_pending_permissions = Arc::new(Mutex::new(HashMap::new()));
        let acp_terminals = AcpTerminalManager::new();
        let (notifications_tx, notifications_rx) = mpsc::unbounded_channel();
        let mut client = StdioServerClient {
            child,
            stdin,
            pending: Arc::clone(&pending),
            acp_pending_permissions,
            acp_terminals,
            acp_agent_capabilities: None,
            next_request_id: AtomicU64::new(1),
            notifications_rx,
            notifications_tx,
        };
        let session_id = devo_protocol::SessionId::new();
        let params = TurnStartParams {
            session_id,
            input: vec![devo_protocol::InputItem::Text {
                text: "native prompt".to_string(),
            }],
            model: Some("ignored-by-acp".to_string()),
            model_binding_id: None,
            reasoning_effort_selection: None,
            sandbox: None,
            approval_policy: None,
            cwd: None,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            execution_mode: devo_protocol::TurnExecutionMode::Regular,
        };
        let mut stdout_lines = BufReader::new(stdout).lines();

        let turn_start = tokio::spawn(async move {
            let result = client.turn_start(params).await;
            (result, client)
        });

        let first_request = read_request_line(&mut stdout_lines).await;
        assert_eq!(first_request["method"], "_devo/turn/start");
        pending
            .lock()
            .await
            .remove(&1)
            .expect("devo turn/start has pending response")
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {
                    "code": -32601,
                    "message": "method not found"
                }
            }))
            .expect("send method-not-found response");

        let second_request = read_request_line(&mut stdout_lines).await;
        assert_eq!(
            second_request,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "session/prompt",
                "params": {
                    "sessionId": session_id,
                    "prompt": [
                        {
                            "type": "text",
                            "text": "native prompt"
                        }
                    ]
                }
            })
        );

        let (result, mut client) = turn_start.await.expect("turn_start task joins");
        result.expect("ACP prompt fallback starts");
        let started = client
            .recv_notification()
            .await
            .expect("ACP prompt started notification");
        assert_eq!(started.method, ACP_PROMPT_STARTED_NOTIFICATION_METHOD);
        assert_eq!(
            started.params,
            serde_json::json!({ "sessionId": session_id })
        );

        pending
            .lock()
            .await
            .remove(&2)
            .expect("ACP prompt has pending response")
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {
                    "stopReason": "end_turn"
                }
            }))
            .expect("send ACP prompt response");
        let completed = timeout(Duration::from_secs(5), client.recv_notification())
            .await
            .expect("completed notification before timeout")
            .expect("ACP prompt completed notification");
        assert_eq!(completed.method, ACP_PROMPT_COMPLETED_NOTIFICATION_METHOD);
        assert_eq!(
            completed.params,
            serde_json::json!({
                "sessionId": session_id,
                "stopReason": "end_turn"
            })
        );

        let _ = client.child.start_kill();
        let _ = client.child.wait().await;
    }

    #[tokio::test]
    async fn stdout_reader_drops_pending_responses_when_stdout_closes() {
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (response_tx, response_rx) = oneshot::channel();
        let request_id = 7;
        pending.lock().await.insert(request_id, response_tx);
        let (notifications_tx, _notifications_rx) = mpsc::unbounded_channel();
        let (mut child, stdin) = child_stdin_for_stdout_reader_test().await;
        let acp_pending_permissions = Arc::new(Mutex::new(HashMap::new()));
        let acp_terminals = AcpTerminalManager::new();

        run_stdout_reader(
            BufReader::new(tokio::io::empty()).lines(),
            Arc::clone(&pending),
            stdin,
            acp_pending_permissions,
            acp_terminals,
            notifications_tx,
        )
        .await;

        assert!(response_rx.await.is_err());
        assert_eq!(pending.lock().await.len(), 0);

        let _ = child.start_kill();
        let _ = child.wait().await;
    }

    #[cfg(windows)]
    async fn request_capture_child_for_turn_start_test()
    -> (Child, ChildStdin, tokio::process::ChildStdout) {
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-Command",
            "for ($i = 0; $i -lt 2; $i++) { $line = [Console]::In.ReadLine(); if ($null -eq $line) { break }; [Console]::Out.WriteLine($line) }; Start-Sleep -Seconds 30",
        ]);
        request_capture_child_for_turn_start_command(command).await
    }

    #[cfg(unix)]
    async fn request_capture_child_for_turn_start_test()
    -> (Child, ChildStdin, tokio::process::ChildStdout) {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "i=0; while [ $i -lt 2 ] && IFS= read -r line; do printf '%s\n' \"$line\"; i=$((i + 1)); done; sleep 30",
        ]);
        request_capture_child_for_turn_start_command(command).await
    }

    async fn request_capture_child_for_turn_start_command(
        mut command: Command,
    ) -> (Child, ChildStdin, tokio::process::ChildStdout) {
        command.kill_on_drop(true);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().expect("spawn request capture child");
        let stdin = child.stdin.take().expect("capture child stdin");
        let stdout = child.stdout.take().expect("capture child stdout");
        (child, stdin, stdout)
    }

    async fn read_request_line<R>(stdout_lines: &mut tokio::io::Lines<R>) -> serde_json::Value
    where
        R: AsyncBufRead + Unpin,
    {
        let request_line = timeout(Duration::from_secs(5), stdout_lines.next_line())
            .await
            .expect("read request line before timeout")
            .expect("read request line")
            .expect("request line is present");
        serde_json::from_str::<serde_json::Value>(&request_line).expect("request line is JSON")
    }

    #[cfg(windows)]
    async fn child_stdin_for_stdout_reader_test() -> (Child, Arc<Mutex<ChildStdin>>) {
        let mut command = Command::new("cmd");
        command.args(["/C", "more >NUL"]);
        child_stdin_for_stdout_reader_command(command).await
    }

    #[cfg(unix)]
    async fn child_stdin_for_stdout_reader_test() -> (Child, Arc<Mutex<ChildStdin>>) {
        let mut command = Command::new("sh");
        command.args(["-c", "cat >/dev/null"]);
        child_stdin_for_stdout_reader_command(command).await
    }

    async fn child_stdin_for_stdout_reader_command(
        mut command: Command,
    ) -> (Child, Arc<Mutex<ChildStdin>>) {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().expect("spawn child for stdin");
        let stdin = child.stdin.take().expect("capture child stdin");
        (child, Arc::new(Mutex::new(stdin)))
    }
}
