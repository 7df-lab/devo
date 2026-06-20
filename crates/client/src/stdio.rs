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

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use devo_protocol::ACP_INITIALIZE_METHOD;
use devo_protocol::ACP_SESSION_LIST_METHOD;
use devo_protocol::ACP_SESSION_NEW_METHOD;
use devo_protocol::ACP_SESSION_PROMPT_METHOD;
use devo_protocol::ACP_SESSION_RESUME_METHOD;
use devo_protocol::ACP_SESSION_UPDATE_METHOD;
use devo_protocol::AcpAgentCapabilities;
use devo_protocol::AcpClientRequest;
use devo_protocol::AcpImplementation;
use devo_protocol::AcpInitializeParams;
use devo_protocol::AcpInitializeResult;
use devo_protocol::AcpListSessionsParams;
use devo_protocol::AcpListSessionsResult;
use devo_protocol::AcpNewSessionParams;
use devo_protocol::AcpNewSessionResult;
use devo_protocol::AcpPromptParams;
use devo_protocol::AcpResumeSessionParams;
use devo_protocol::AcpResumeSessionResult;
use devo_protocol::AcpSessionNotification;
use devo_protocol::AcpSuccessResponse;
use devo_protocol::AgentListParams;
use devo_protocol::AgentListResult;
use devo_protocol::ApprovalDecisionPayload;
use devo_protocol::ApprovalDecisionValue;
use devo_protocol::ApprovalRequestPayload;
use devo_protocol::ApprovalRespondParams;
use devo_protocol::ApprovalScopeValue;
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
use devo_protocol::EventContext;
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
use devo_protocol::ItemEnvelope;
use devo_protocol::ItemEventPayload;
use devo_protocol::ItemId;
use devo_protocol::ItemKind;
use devo_protocol::ModelCatalogParams;
use devo_protocol::ModelCatalogResult;
use devo_protocol::ModelSavedParams;
use devo_protocol::ModelSavedResult;
use devo_protocol::NotificationEnvelope;
use devo_protocol::PendingServerRequestContext;
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
use devo_protocol::ServerRequestKind;
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
use devo_protocol::SessionStartParams;
use devo_protocol::SessionStartResult;
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
static ACP_PERMISSION_NEXT_ID: AtomicU64 = AtomicU64::new(1);

type PendingResponses = Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>;

#[derive(Debug, Clone)]
pub struct StdioServerClientConfig {
    pub program: PathBuf,
    pub workspace_root: Option<PathBuf>,
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ServerNotificationMessage {
    pub method: String,
    pub params: serde_json::Value,
}

type AcpPendingPermissions = Arc<Mutex<HashMap<String, AcpPendingPermission>>>;

struct AcpPendingPermission {
    request_id: serde_json::Value,
    session_id: devo_protocol::SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    options: Vec<AcpPermissionOption>,
}

struct AcpPermissionOption {
    option_id: String,
    kind: String,
}

pub struct StdioServerClient {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: PendingResponses,
    acp_pending_permissions: AcpPendingPermissions,
    acp_agent_capabilities: Option<AcpAgentCapabilities>,
    next_request_id: AtomicU64,
    notifications_rx: mpsc::UnboundedReceiver<ServerNotificationMessage>,
    notifications_tx: mpsc::UnboundedSender<ServerNotificationMessage>,
}

impl StdioServerClient {
    pub async fn spawn(config: StdioServerClientConfig) -> Result<Self> {
        tracing::info!(
            program = %config.program.display(),
            workspace_root = ?config.workspace_root,
            "spawning stdio server client"
        );
        let mut command = Command::new(&config.program);
        for arg in config.args {
            command.arg(arg);
        }
        if let Some(workspace_root) = config.workspace_root {
            command.arg("--working-root").arg(workspace_root);
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
        let (notifications_tx, notifications_rx) = mpsc::unbounded_channel();

        tokio::spawn(run_stdout_reader(
            BufReader::new(stdout).lines(),
            Arc::clone(&pending),
            Arc::clone(&stdin),
            Arc::clone(&acp_pending_permissions),
            notifications_tx.clone(),
        ));
        tokio::spawn(run_stderr_reader(BufReader::new(stderr).lines()));

        Ok(Self {
            child,
            stdin,
            pending,
            acp_pending_permissions,
            acp_agent_capabilities: None,
            next_request_id: AtomicU64::new(1),
            notifications_rx,
            notifications_tx,
        })
    }

    pub async fn initialize(&mut self) -> Result<InitializeResult> {
        tracing::info!("initializing stdio server client");
        let result: AcpInitializeResult = timeout(
            Duration::from_secs(10),
            self.request(
                ACP_INITIALIZE_METHOD,
                AcpInitializeParams {
                    protocol_version: 1,
                    client_capabilities: serde_json::json!({}),
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
            .context("ACP session/new response missing Devo session metadata")?;
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
                    cwd: session.cwd,
                    additional_directories: session.additional_directories,
                    mcp_servers: Vec::new(),
                    meta: None,
                },
            )
            .await?;
        result
            .meta
            .as_ref()
            .and_then(|meta| meta.get(DEVO_SESSION_RESUME_META))
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .context("decode session resume metadata from ACP session/resume response")?
            .context("ACP session/resume response missing Devo resume metadata")
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
                    .with_context(|| {
                        format!(
                            "ACP session/list response missing Devo session metadata for {}",
                            session_info.session_id
                        )
                    })?;
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

    pub async fn turn_start(&mut self, params: TurnStartParams) -> Result<()> {
        self.request_detached(
            ACP_SESSION_PROMPT_METHOD,
            AcpPromptParams {
                session_id: params.session_id,
                prompt: params
                    .input
                    .into_iter()
                    .map(acp_content_block_from_input_item)
                    .collect(),
                meta: None,
            },
        )
        .await
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

    pub async fn approval_respond(&mut self, params: ApprovalRespondParams) -> Result<()> {
        if let Some(pending) = self
            .acp_pending_permissions
            .lock()
            .await
            .remove(&params.approval_id.to_string())
        {
            let decision = acp_permission_response_from_approval(&params, &pending);
            write_acp_client_response(
                Arc::clone(&self.stdin),
                acp_client_success_response(pending.request_id.clone(), decision),
            )
            .await
            .context("write ACP permission response")?;
            let _ = self
                .notifications_tx
                .send(acp_approval_decision_notification(&params, &pending));
            return Ok(());
        }
        let _: serde_json::Value = self.request_devo("approval/respond", params).await?;
        Ok(())
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

    async fn request_detached<P>(&mut self, method: &str, params: P) -> Result<()>
    where
        P: serde::Serialize,
    {
        let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        tracing::debug!(request_id, method, "sending detached client request");
        let (response_tx, _response_rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id, response_tx);
        self.write_json(&AcpClientRequest::new(
            serde_json::json!(request_id),
            method,
            params,
        ))
        .await
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
}

async fn handle_acp_client_request(
    stdin: Arc<Mutex<ChildStdin>>,
    acp_pending_permissions: AcpPendingPermissions,
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
    let response = match method {
        "fs/read_text_file"
        | "fs/write_text_file"
        | "terminal/create"
        | "terminal/output"
        | "terminal/wait_for_exit"
        | "terminal/kill"
        | "terminal/release" => {
            acp_client_error_response(id, -32601, format!("unsupported client method {method}"))
        }
        _ => acp_client_error_response(id, -32601, format!("unknown client method {method}")),
    };
    if let Err(error) = write_acp_client_response(stdin, response).await {
        tracing::warn!(%error, method, "failed to write ACP client response");
    }
}

async fn handle_acp_request_permission(
    request_id: serde_json::Value,
    params: serde_json::Value,
    pending_permissions: AcpPendingPermissions,
    notifications_tx: mpsc::UnboundedSender<ServerNotificationMessage>,
) -> std::result::Result<(), String> {
    let session_id = params
        .get("sessionId")
        .cloned()
        .ok_or_else(|| "session/request_permission params.sessionId is required".to_string())
        .and_then(|value| {
            serde_json::from_value::<devo_protocol::SessionId>(value)
                .map_err(|error| format!("invalid session/request_permission sessionId: {error}"))
        })?;
    let options = acp_permission_options(&params)?;
    if !options
        .iter()
        .any(|option| option.kind.starts_with("allow"))
    {
        return Err("session/request_permission options must include an allow option".to_string());
    }

    let approval_id = format!(
        "acp-permission-{}",
        ACP_PERMISSION_NEXT_ID.fetch_add(1, Ordering::SeqCst)
    );
    let pending = AcpPendingPermission {
        request_id,
        session_id,
        turn_id: TurnId::new(),
        item_id: ItemId::new(),
        options,
    };
    let notification = acp_approval_request_notification(&approval_id, &params, &pending);
    pending_permissions
        .lock()
        .await
        .insert(approval_id.clone(), pending);
    if let Err(error) = notifications_tx.send(notification) {
        pending_permissions.lock().await.remove(&approval_id);
        return Err(format!("failed to deliver permission request: {error}"));
    }
    Ok(())
}

fn acp_permission_options(
    params: &serde_json::Value,
) -> std::result::Result<Vec<AcpPermissionOption>, String> {
    let options = params
        .get("options")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "session/request_permission params.options must be an array".to_string())?;
    options
        .iter()
        .map(|option| {
            let option_id = option
                .get("optionId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    "session/request_permission option.optionId must be a string".to_string()
                })?
                .to_string();
            let kind = option
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    "session/request_permission option.kind must be a string".to_string()
                })?
                .to_string();
            Ok(AcpPermissionOption { option_id, kind })
        })
        .collect()
}

fn acp_approval_request_notification(
    approval_id: &str,
    params: &serde_json::Value,
    pending: &AcpPendingPermission,
) -> ServerNotificationMessage {
    let action_summary = params
        .get("toolCall")
        .and_then(|tool_call| tool_call.get("title"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("ACP tool permission request")
        .to_string();
    let target = params
        .get("toolCall")
        .and_then(|tool_call| tool_call.get("toolCallId"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let request = PendingServerRequestContext {
        request_id: approval_id.to_string().into(),
        request_kind: ServerRequestKind::ItemPermissionsRequestApproval,
        session_id: pending.session_id,
        turn_id: Some(pending.turn_id),
        item_id: Some(pending.item_id),
    };
    let payload = ApprovalRequestPayload {
        request,
        approval_id: approval_id.to_string().into(),
        action_summary,
        justification: "ACP agent requested permission to continue this tool call.".to_string(),
        resource: target.clone(),
        available_scopes: acp_approval_scopes(&pending.options),
        path: None,
        host: None,
        target,
    };
    acp_item_notification(
        "item/completed",
        ServerEvent::ItemCompleted(ItemEventPayload {
            context: EventContext {
                session_id: pending.session_id,
                turn_id: Some(pending.turn_id),
                item_id: Some(pending.item_id),
                seq: 0,
            },
            item: ItemEnvelope {
                item_id: pending.item_id,
                item_kind: ItemKind::ApprovalRequest,
                payload: serde_json::to_value(payload).expect("serialize ACP approval request"),
            },
        }),
    )
}

fn acp_approval_decision_notification(
    params: &ApprovalRespondParams,
    pending: &AcpPendingPermission,
) -> ServerNotificationMessage {
    let payload = ApprovalDecisionPayload {
        approval_id: params.approval_id.clone(),
        decision: acp_approval_decision_label(&params.decision).to_string(),
        scope: acp_approval_scope_label(&params.scope).to_string(),
    };
    acp_item_notification(
        "item/completed",
        ServerEvent::ItemCompleted(ItemEventPayload {
            context: EventContext {
                session_id: pending.session_id,
                turn_id: Some(pending.turn_id),
                item_id: Some(pending.item_id),
                seq: 0,
            },
            item: ItemEnvelope {
                item_id: ItemId::new(),
                item_kind: ItemKind::ApprovalDecision,
                payload: serde_json::to_value(payload).expect("serialize ACP approval decision"),
            },
        }),
    )
}

fn acp_item_notification(method: &str, event: ServerEvent) -> ServerNotificationMessage {
    ServerNotificationMessage {
        method: method.to_string(),
        params: serde_json::to_value(event).expect("serialize ACP bridged event"),
    }
}

fn acp_approval_scopes(options: &[AcpPermissionOption]) -> Vec<String> {
    let mut scopes = Vec::new();
    if options.iter().any(|option| option.kind == "allow_once") {
        scopes.push("once".to_string());
    }
    if options.iter().any(|option| option.kind == "allow_always") {
        scopes.push("session".to_string());
    }
    scopes
}

fn acp_permission_response_from_approval(
    params: &ApprovalRespondParams,
    pending: &AcpPendingPermission,
) -> serde_json::Value {
    if let Some(option_id) = acp_selected_permission_option(params, pending) {
        serde_json::json!({
            "outcome": {
                "outcome": "selected",
                "optionId": option_id
            }
        })
    } else {
        acp_cancelled_permission_response()
    }
}

fn acp_selected_permission_option(
    params: &ApprovalRespondParams,
    pending: &AcpPendingPermission,
) -> Option<String> {
    let preferred_kinds: &[&str] = match params.decision {
        ApprovalDecisionValue::Approve => match params.scope {
            ApprovalScopeValue::Session => &["allow_always", "allow_once"],
            ApprovalScopeValue::Once
            | ApprovalScopeValue::Turn
            | ApprovalScopeValue::PathPrefix
            | ApprovalScopeValue::Host
            | ApprovalScopeValue::Tool
            | ApprovalScopeValue::CommandPrefix => &["allow_once", "allow_always"],
        },
        ApprovalDecisionValue::Deny => match params.scope {
            ApprovalScopeValue::Session => &["reject_always", "reject_once"],
            ApprovalScopeValue::Once
            | ApprovalScopeValue::Turn
            | ApprovalScopeValue::PathPrefix
            | ApprovalScopeValue::Host
            | ApprovalScopeValue::Tool
            | ApprovalScopeValue::CommandPrefix => &["reject_once", "reject_always"],
        },
        ApprovalDecisionValue::Cancel => return None,
    };
    preferred_kinds.iter().find_map(|kind| {
        pending
            .options
            .iter()
            .find(|option| option.kind == *kind)
            .map(|option| option.option_id.clone())
    })
}

fn acp_cancelled_permission_response() -> serde_json::Value {
    serde_json::json!({
        "outcome": {
            "outcome": "cancelled"
        }
    })
}

fn acp_approval_decision_label(decision: &ApprovalDecisionValue) -> &'static str {
    match decision {
        ApprovalDecisionValue::Approve => "approve",
        ApprovalDecisionValue::Deny => "deny",
        ApprovalDecisionValue::Cancel => "cancel",
    }
}

fn acp_approval_scope_label(scope: &ApprovalScopeValue) -> &'static str {
    match scope {
        ApprovalScopeValue::Once => "once",
        ApprovalScopeValue::Turn => "turn",
        ApprovalScopeValue::Session => "session",
        ApprovalScopeValue::PathPrefix => "path_prefix",
        ApprovalScopeValue::Host => "host",
        ApprovalScopeValue::Tool => "tool",
        ApprovalScopeValue::CommandPrefix => "command_prefix",
    }
}

fn acp_client_success_response(
    id: serde_json::Value,
    result: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
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
    if let Ok(error) = serde_json::from_value::<ErrorResponse>(response.clone()) {
        let data = if error.error.data.is_null() {
            String::new()
        } else {
            format!(" data={}", error.error.data)
        };
        anyhow::bail!(
            "server {}: {}{}",
            format_protocol_error_code(&error.error.code),
            error.error.message,
            data
        );
    }
    let code = response
        .get("error")
        .and_then(|error| error.get("code"))
        .map(serde_json::Value::to_string)
        .unwrap_or_else(|| "unknown".to_string());
    let message = response
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown server error");
    anyhow::bail!("server {code}: {message}")
}

fn acp_content_block_from_input_item(
    input: devo_protocol::InputItem,
) -> devo_protocol::AcpContentBlock {
    match input {
        devo_protocol::InputItem::Text { text } => devo_protocol::AcpContentBlock::text(text),
        devo_protocol::InputItem::Skill { name, path } => devo_protocol::AcpContentBlock::Text {
            annotations: None,
            text: format!("Skill {name}: {}", path.display()),
            meta: None,
        },
        devo_protocol::InputItem::LocalImage { path } => devo_protocol::AcpContentBlock::Text {
            annotations: None,
            text: format!("Image: {}", path.display()),
            meta: None,
        },
        devo_protocol::InputItem::Mention { path, name } => {
            devo_protocol::AcpContentBlock::ResourceLink {
                annotations: None,
                uri: file_uri_from_path(&path),
                name: name.unwrap_or_else(|| path.clone()),
                title: None,
                description: None,
                mime_type: None,
                size: None,
                meta: None,
            }
        }
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
    assistant_token_logging_enabled()
        .then(|| format_assistant_token_log_preview(text, assistant_token_log_max_chars()))
}

fn assistant_token_logging_enabled() -> bool {
    static ASSISTANT_TOKEN_LOGGING_ENABLED: OnceLock<bool> = OnceLock::new();
    *ASSISTANT_TOKEN_LOGGING_ENABLED.get_or_init(|| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn stdout_reader_drops_pending_responses_when_stdout_closes() {
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (response_tx, response_rx) = oneshot::channel();
        let request_id = 7;
        pending.lock().await.insert(request_id, response_tx);
        let (notifications_tx, _notifications_rx) = mpsc::unbounded_channel();
        let (mut child, stdin) = child_stdin_for_stdout_reader_test().await;
        let acp_pending_permissions = Arc::new(Mutex::new(HashMap::new()));

        run_stdout_reader(
            BufReader::new(tokio::io::empty()).lines(),
            Arc::clone(&pending),
            stdin,
            acp_pending_permissions,
            notifications_tx,
        )
        .await;

        assert!(response_rx.await.is_err());
        assert_eq!(pending.lock().await.len(), 0);

        let _ = child.start_kill();
        let _ = child.wait().await;
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
