//! Stdio transport for a spawned devo server process.
//!
//! The client writes newline-delimited JSON requests to child stdin and owns a
//! background stdout reader that demultiplexes responses by request id while
//! forwarding id-less messages as server notifications.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use devo_protocol::AgentListParams;
use devo_protocol::AgentListResult;
use devo_protocol::ApprovalRespondParams;
use devo_protocol::ClientNotification;
use devo_protocol::ClientRequest;
use devo_protocol::ClientTransportKind;
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
use devo_protocol::InitializeParams;
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
use devo_protocol::SessionListParams;
use devo_protocol::SessionListResult;
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
use devo_protocol::SuccessResponse;
use devo_protocol::TurnInterruptParams;
use devo_protocol::TurnInterruptResult;
use devo_protocol::TurnStartParams;
use devo_protocol::TurnStartResult;
use devo_protocol::TurnSteerParams;
use devo_protocol::TurnSteerResult;
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

pub struct StdioServerClient {
    child: Child,
    stdin: ChildStdin,
    pending: PendingResponses,
    next_request_id: AtomicU64,
    notifications_rx: mpsc::UnboundedReceiver<ServerNotificationMessage>,
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
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (notifications_tx, notifications_rx) = mpsc::unbounded_channel();

        tokio::spawn(run_stdout_reader(
            BufReader::new(stdout).lines(),
            Arc::clone(&pending),
            notifications_tx,
        ));
        tokio::spawn(run_stderr_reader(BufReader::new(stderr).lines()));

        Ok(Self {
            child,
            stdin,
            pending,
            next_request_id: AtomicU64::new(1),
            notifications_rx,
        })
    }

    pub async fn initialize(&mut self) -> Result<InitializeResult> {
        tracing::info!("initializing stdio server client");
        let result = timeout(
            Duration::from_secs(10),
            self.request(
                "initialize",
                InitializeParams {
                    client_name: "devo".into(),
                    client_version: env!("CARGO_PKG_VERSION").into(),
                    transport: ClientTransportKind::Stdio,
                    supports_streaming: true,
                    supports_binary_images: false,
                    opt_out_notification_methods: Vec::new(),
                },
            ),
        )
        .await
        .context("timed out waiting for initialize response from server")??;
        self.notify("initialized", serde_json::json!({})).await?;
        tracing::info!("stdio server client initialized");
        Ok(result)
    }

    pub async fn session_start(
        &mut self,
        params: SessionStartParams,
    ) -> Result<SessionStartResult> {
        self.request("session/start", params).await
    }

    pub async fn session_resume(
        &mut self,
        params: SessionResumeParams,
    ) -> Result<SessionResumeResult> {
        self.request("session/resume", params).await
    }

    pub async fn session_list(&mut self, params: SessionListParams) -> Result<SessionListResult> {
        self.request("session/list", params).await
    }

    pub async fn agent_list(&mut self, params: AgentListParams) -> Result<AgentListResult> {
        self.request("agent/list", params).await
    }

    pub async fn agent_spawn(&mut self, params: SpawnAgentParams) -> Result<SpawnAgentResult> {
        self.request("agent/spawn", params).await
    }

    pub async fn agent_close(&mut self, params: CloseAgentParams) -> Result<CloseAgentResult> {
        self.request("agent/close", params).await
    }

    pub async fn session_title_update(
        &mut self,
        params: SessionTitleUpdateParams,
    ) -> Result<SessionTitleUpdateResult> {
        self.request("session/title/update", params).await
    }

    pub async fn session_metadata_update(
        &mut self,
        params: SessionMetadataUpdateParams,
    ) -> Result<SessionMetadataUpdateResult> {
        self.request("session/metadata/update", params).await
    }

    pub async fn session_permissions_update(
        &mut self,
        params: SessionPermissionsUpdateParams,
    ) -> Result<SessionPermissionsUpdateResult> {
        self.request("session/permissions/update", params).await
    }

    pub async fn session_compact(
        &mut self,
        params: SessionCompactParams,
    ) -> Result<SessionCompactResult> {
        self.request("session/compact", params).await
    }

    pub async fn goal_create(&mut self, params: GoalCreateParams) -> Result<GoalCreateResult> {
        self.request("goal/create", params).await
    }

    pub async fn goal_set(&mut self, params: GoalSetParams) -> Result<GoalSetResult> {
        self.request("goal/set", params).await
    }

    pub async fn goal_status(&mut self, params: GoalStatusParams) -> Result<GoalStatusResult> {
        self.request("goal/status", params).await
    }

    pub async fn goal_pause(&mut self, params: GoalSetStatusParams) -> Result<GoalSetStatusResult> {
        self.request("goal/pause", params).await
    }

    pub async fn goal_resume(
        &mut self,
        params: GoalSetStatusParams,
    ) -> Result<GoalSetStatusResult> {
        self.request("goal/resume", params).await
    }

    pub async fn goal_complete(
        &mut self,
        params: GoalSetStatusParams,
    ) -> Result<GoalSetStatusResult> {
        self.request("goal/complete", params).await
    }

    pub async fn goal_clear(&mut self, params: GoalClearParams) -> Result<GoalClearResult> {
        self.request("goal/clear", params).await
    }

    pub async fn session_fork(&mut self, params: SessionForkParams) -> Result<SessionForkResult> {
        self.request("session/fork", params).await
    }

    pub async fn session_rollback(
        &mut self,
        params: SessionRollbackParams,
    ) -> Result<SessionRollbackResult> {
        self.request("session/rollback", params).await
    }

    pub async fn skills_list(&mut self, params: SkillListParams) -> Result<SkillListResult> {
        self.request("skills/list", params).await
    }

    pub async fn skills_changed(
        &mut self,
        params: SkillChangedParams,
    ) -> Result<SkillChangedResult> {
        self.request("skills/changed", params).await
    }

    pub async fn skills_set_enabled(
        &mut self,
        params: SkillSetEnabledParams,
    ) -> Result<SkillSetEnabledResult> {
        self.request("skills/set_enabled", params).await
    }

    pub async fn model_catalog(
        &mut self,
        params: ModelCatalogParams,
    ) -> Result<ModelCatalogResult> {
        self.request("model/catalog", params).await
    }

    pub async fn model_saved(&mut self, params: ModelSavedParams) -> Result<ModelSavedResult> {
        self.request("model/saved", params).await
    }

    pub async fn provider_vendor_list(
        &mut self,
        params: ProviderVendorListParams,
    ) -> Result<ProviderVendorListResult> {
        self.request("provider/list", params).await
    }

    pub async fn provider_vendor_upsert(
        &mut self,
        params: ProviderVendorUpsertParams,
    ) -> Result<ProviderVendorUpsertResult> {
        self.request("provider/upsert", params).await
    }

    pub async fn provider_validate(
        &mut self,
        params: ProviderValidateParams,
    ) -> Result<ProviderValidateResult> {
        self.request("provider/validate", params).await
    }

    pub async fn command_exec(&mut self, params: CommandExecParams) -> Result<CommandExecResult> {
        self.request("command/exec", params).await
    }

    pub async fn command_exec_write(
        &mut self,
        params: CommandExecWriteParams,
    ) -> Result<CommandExecWriteResult> {
        self.request("command/exec/write", params).await
    }

    pub async fn command_exec_resize(
        &mut self,
        params: CommandExecResizeParams,
    ) -> Result<CommandExecResizeResult> {
        self.request("command/exec/resize", params).await
    }

    pub async fn command_exec_terminate(
        &mut self,
        params: CommandExecTerminateParams,
    ) -> Result<CommandExecTerminateResult> {
        self.request("command/exec/terminate", params).await
    }

    pub async fn turn_start(&mut self, params: TurnStartParams) -> Result<TurnStartResult> {
        self.request("turn/start", params).await
    }

    pub async fn turn_shell_command(
        &mut self,
        params: ShellCommandParams,
    ) -> Result<ShellCommandResult> {
        self.request("turn/shell_command", params).await
    }

    pub async fn turn_interrupt(
        &mut self,
        params: TurnInterruptParams,
    ) -> Result<TurnInterruptResult> {
        self.request("turn/interrupt", params).await
    }

    pub async fn turn_steer(&mut self, params: TurnSteerParams) -> Result<TurnSteerResult> {
        self.request("turn/steer", params).await
    }

    pub async fn approval_respond(&mut self, params: ApprovalRespondParams) -> Result<()> {
        let _: serde_json::Value = self.request("approval/respond", params).await?;
        Ok(())
    }

    pub async fn request_user_input_respond(
        &mut self,
        params: RequestUserInputRespondParams,
    ) -> Result<()> {
        let _: serde_json::Value = self.request("request_user_input/respond", params).await?;
        Ok(())
    }

    pub async fn reference_search_start(
        &mut self,
        params: ReferenceSearchStartParams,
    ) -> Result<ReferenceSearchStartResult> {
        self.request("search/start", params).await
    }

    pub async fn reference_search_update(
        &mut self,
        params: ReferenceSearchUpdateParams,
    ) -> Result<ReferenceSearchUpdateResult> {
        self.request("search/update", params).await
    }

    pub async fn reference_search_cancel(
        &mut self,
        params: ReferenceSearchCancelParams,
    ) -> Result<ReferenceSearchCancelResult> {
        self.request("search/cancel", params).await
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
        let _ = timeout(SERVER_CHILD_STDIN_SHUTDOWN_TIMEOUT, self.stdin.shutdown()).await;
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
            .write_json(&ClientRequest {
                id: serde_json::json!(request_id),
                method: method.to_string(),
                params,
            })
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
            let error: ErrorResponse =
                serde_json::from_value(response).context("decode error response from server")?;
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
        let success: SuccessResponse<R> =
            serde_json::from_value(response).context("decode success response from server")?;
        Ok(success.result)
    }

    async fn notify<P>(&mut self, method: &str, params: P) -> Result<()>
    where
        P: serde::Serialize,
    {
        self.write_json(&ClientNotification {
            method: method.to_string(),
            params,
        })
        .await
    }

    async fn write_json<T>(&mut self, value: &T) -> Result<()>
    where
        T: serde::Serialize,
    {
        let mut line = serde_json::to_vec(value).context("serialize client payload")?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .context("write client payload")?;
        self.stdin.flush().await.context("flush client payload")?;
        Ok(())
    }
}

async fn run_stdout_reader<R>(
    mut lines: tokio::io::Lines<R>,
    pending: PendingResponses,
    notifications_tx: mpsc::UnboundedSender<ServerNotificationMessage>,
) where
    R: AsyncBufRead + Unpin,
{
    while let Ok(Some(line)) = lines.next_line().await {
        match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(message) => {
                // Responses are consumed by the request future waiting on the
                // matching oneshot. Notifications intentionally bypass that
                // map so event consumers can drain them independently.
                if let Some(id) = message.get("id").and_then(serde_json::Value::as_u64) {
                    if let Some(tx) = pending.lock().await.remove(&id) {
                        let _ = tx.send(message);
                    }
                } else if let Ok(notification) =
                    serde_json::from_value::<NotificationEnvelope<serde_json::Value>>(message)
                {
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

        run_stdout_reader(
            BufReader::new(tokio::io::empty()).lines(),
            Arc::clone(&pending),
            notifications_tx,
        )
        .await;

        assert!(response_rx.await.is_err());
        assert_eq!(pending.lock().await.len(), 0);
    }
}
