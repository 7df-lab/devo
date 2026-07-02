use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use devo_core::tools::unified_exec::process::TerminalSize;
use devo_core::tools::unified_exec::process::UnifiedExecProcess;
use devo_core::tools::unified_exec::store::ProcessStore;
use tokio::sync::Mutex;
use tokio::sync::broadcast;
use tokio::time::Duration;

use crate::ProtocolErrorCode;
use crate::ServerEvent;
use crate::SuccessResponse;
use crate::runtime::ServerRuntime;
use crate::runtime::connection::SubscriptionFilter;
use devo_protocol::CommandExecExitedPayload;
use devo_protocol::CommandExecOutputDeltaPayload;
use devo_protocol::CommandExecOutputStream;
use devo_protocol::CommandExecParams;
use devo_protocol::CommandExecProgram;
use devo_protocol::CommandExecResizeParams;
use devo_protocol::CommandExecResizeResult;
use devo_protocol::CommandExecResult;
use devo_protocol::CommandExecTerminalSize;
use devo_protocol::CommandExecTerminateParams;
use devo_protocol::CommandExecTerminateResult;
use devo_protocol::CommandExecWriteParams;
use devo_protocol::CommandExecWriteResult;
use devo_protocol::SessionId;

const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone)]
pub(super) struct CommandExecManager {
    sessions: Arc<Mutex<HashMap<CommandExecKey, CommandExecSession>>>,
    store: Arc<ProcessStore>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CommandExecKey {
    connection_id: u64,
    session_id: Option<SessionId>,
    process_id: String,
}

#[derive(Clone)]
struct CommandExecSession {
    store_process_id: i32,
}

type CommandExecRuntimeResult<T> = Result<T, (ProtocolErrorCode, String)>;

impl CommandExecManager {
    pub(super) fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            store: Arc::new(ProcessStore::new()),
        }
    }

    async fn start(
        &self,
        runtime: Arc<ServerRuntime>,
        connection_id: u64,
        params: CommandExecParams,
        cwd: PathBuf,
    ) -> CommandExecRuntimeResult<CommandExecResult> {
        if params.process_id.trim().is_empty() {
            return Err((
                ProtocolErrorCode::InvalidParams,
                "command/exec process_id must not be empty".to_string(),
            ));
        }
        if let Some(size) = params.size {
            validate_terminal_size(size)?;
        }

        let key = CommandExecKey {
            connection_id,
            session_id: params.session_id,
            process_id: params.process_id.clone(),
        };
        let store_process_id = self.store.reserve_process_id().await.ok_or_else(|| {
            (
                ProtocolErrorCode::InternalError,
                "unable to reserve shell process id".to_string(),
            )
        })?;
        let duplicate_process_id = {
            let mut sessions = self.sessions.lock().await;
            if sessions.contains_key(&key) {
                true
            } else {
                sessions.insert(key.clone(), CommandExecSession { store_process_id });
                false
            }
        };
        if duplicate_process_id {
            self.store.release_reserved(store_process_id).await;
            return Err((
                ProtocolErrorCode::InvalidParams,
                format!("duplicate command/exec process id: {}", params.process_id),
            ));
        }

        let spawn_result =
            spawn_command_exec_process(store_process_id, params.program, cwd, params.size).await;
        let (process, output_rx) = match spawn_result {
            Ok(spawned) => spawned,
            Err(error) => {
                self.sessions.lock().await.remove(&key);
                self.store.release_reserved(store_process_id).await;
                return Err((ProtocolErrorCode::InternalError, error));
            }
        };

        let process = Arc::new(process);
        self.store
            .insert_reserved(store_process_id, Arc::clone(&process))
            .await;
        self.spawn_output_task(runtime, key, Arc::clone(&process), output_rx);

        Ok(CommandExecResult {
            process_id: params.process_id,
        })
    }

    async fn write(
        &self,
        connection_id: u64,
        params: CommandExecWriteParams,
    ) -> CommandExecRuntimeResult<CommandExecWriteResult> {
        if params.delta_base64.is_none() && !params.close_stdin {
            return Err((
                ProtocolErrorCode::InvalidParams,
                "command/exec/write requires delta_base64 or close_stdin".to_string(),
            ));
        }
        let process = self
            .get_process(connection_id, params.session_id, &params.process_id)
            .await?;
        if let Some(delta_base64) = params.delta_base64 {
            let bytes = STANDARD.decode(delta_base64).map_err(|error| {
                (
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid command/exec/write delta_base64: {error}"),
                )
            })?;
            if !bytes.is_empty() {
                let text = String::from_utf8_lossy(&bytes);
                process.write_stdin(&text).map_err(|error| {
                    (
                        ProtocolErrorCode::InvalidParams,
                        format!("failed to write shell stdin: {error}"),
                    )
                })?;
            }
        }
        if params.close_stdin {
            process.close_stdin();
        }
        Ok(CommandExecWriteResult {})
    }

    async fn resize(
        &self,
        connection_id: u64,
        params: CommandExecResizeParams,
    ) -> CommandExecRuntimeResult<CommandExecResizeResult> {
        validate_terminal_size(params.size)?;
        let process = self
            .get_process(connection_id, params.session_id, &params.process_id)
            .await?;
        process
            .resize(protocol_terminal_size(params.size))
            .map_err(|error| (ProtocolErrorCode::InvalidParams, error))?;
        Ok(CommandExecResizeResult {})
    }

    async fn terminate(
        &self,
        connection_id: u64,
        params: CommandExecTerminateParams,
    ) -> CommandExecRuntimeResult<CommandExecTerminateResult> {
        let process = self
            .get_process(connection_id, params.session_id, &params.process_id)
            .await?;
        process.terminate();
        Ok(CommandExecTerminateResult {})
    }

    async fn get_process(
        &self,
        connection_id: u64,
        session_id: Option<SessionId>,
        process_id: &str,
    ) -> CommandExecRuntimeResult<Arc<UnifiedExecProcess>> {
        let store_process_id = {
            let sessions = self.sessions.lock().await;
            sessions
                .iter()
                .find(|(key, _)| {
                    key.connection_id == connection_id
                        && key.session_id == session_id
                        && key.process_id == process_id
                })
                .map(|(_, session)| session.store_process_id)
        };
        let Some(store_process_id) = store_process_id else {
            return Err((
                ProtocolErrorCode::InvalidParams,
                format!("unknown command/exec process id: {process_id}"),
            ));
        };
        self.store.get(store_process_id).await.ok_or_else(|| {
            (
                ProtocolErrorCode::InvalidParams,
                format!("command/exec process is no longer active: {process_id}"),
            )
        })
    }

    fn spawn_output_task(
        &self,
        runtime: Arc<ServerRuntime>,
        key: CommandExecKey,
        process: Arc<UnifiedExecProcess>,
        mut output_rx: broadcast::Receiver<Vec<u8>>,
    ) {
        let manager = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    output = output_rx.recv() => {
                        match output {
                            Ok(bytes) => {
                                let event = ServerEvent::CommandExecOutputDelta(
                                    CommandExecOutputDeltaPayload {
                                        session_id: key.session_id,
                                        process_id: key.process_id.clone(),
                                        stream: CommandExecOutputStream::Pty,
                                        delta_base64: STANDARD.encode(bytes),
                                    },
                                );
                                runtime
                                    .emit_to_connection(
                                        key.connection_id,
                                        event.method_name(),
                                        event,
                                    )
                                    .await;
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    _ = tokio::time::sleep(EXIT_POLL_INTERVAL) => {}
                }

                if process.exit_code().is_some() {
                    break;
                }
            }

            while let Ok(bytes) = output_rx.try_recv() {
                let event = ServerEvent::CommandExecOutputDelta(CommandExecOutputDeltaPayload {
                    session_id: key.session_id,
                    process_id: key.process_id.clone(),
                    stream: CommandExecOutputStream::Pty,
                    delta_base64: STANDARD.encode(bytes),
                });
                runtime
                    .emit_to_connection(key.connection_id, event.method_name(), event)
                    .await;
            }

            let event = ServerEvent::CommandExecExited(CommandExecExitedPayload {
                session_id: key.session_id,
                process_id: key.process_id.clone(),
                exit_code: process.exit_code(),
            });
            runtime
                .emit_to_connection(key.connection_id, event.method_name(), event)
                .await;
            manager.remove_key(&key).await;
        });
    }

    async fn remove_key(&self, key: &CommandExecKey) {
        if let Some(session) = self.sessions.lock().await.remove(key) {
            self.store.remove(session.store_process_id).await;
        }
    }

    pub(super) async fn terminate_connection(&self, connection_id: u64) {
        let sessions = {
            let mut sessions = self.sessions.lock().await;
            let keys = sessions
                .keys()
                .filter(|key| key.connection_id == connection_id)
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| sessions.remove(&key))
                .collect::<Vec<_>>()
        };
        for session in sessions {
            self.store.remove(session.store_process_id).await;
        }
    }

    pub(super) async fn terminate_all(&self) {
        self.sessions.lock().await.clear();
        self.store.terminate_all().await;
    }
}

impl ServerRuntime {
    pub(crate) async fn handle_command_exec(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: CommandExecParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid command/exec params: {error}"),
                );
            }
        };
        let cwd = match self
            .command_exec_cwd(params.session_id, params.cwd.clone())
            .await
        {
            Ok(cwd) => cwd,
            Err((code, message)) => return self.error_response(request_id, code, message),
        };
        let command_exec_event_types = HashSet::from([
            "command/exec/outputDelta".to_string(),
            "command/exec/exited".to_string(),
        ]);
        if let Some(connection) = self.connections.lock().await.get_mut(&connection_id) {
            let already = connection.subscriptions.iter().any(|subscription| {
                subscription.session_id == params.session_id
                    && subscription.event_types == command_exec_event_types
            });
            if !already {
                connection.subscriptions.push(SubscriptionFilter {
                    session_id: params.session_id,
                    event_types: command_exec_event_types,
                    include_child_agents: false,
                });
            }
        }
        match self
            .command_exec_manager
            .start(Arc::clone(self), connection_id, params, cwd)
            .await
        {
            Ok(result) => success_response(request_id, result),
            Err((code, message)) => self.error_response(request_id, code, message),
        }
    }

    pub(crate) async fn handle_command_exec_write(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: CommandExecWriteParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid command/exec/write params: {error}"),
                );
            }
        };
        match self.command_exec_manager.write(connection_id, params).await {
            Ok(result) => success_response(request_id, result),
            Err((code, message)) => self.error_response(request_id, code, message),
        }
    }

    pub(crate) async fn handle_command_exec_resize(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: CommandExecResizeParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid command/exec/resize params: {error}"),
                );
            }
        };
        match self
            .command_exec_manager
            .resize(connection_id, params)
            .await
        {
            Ok(result) => success_response(request_id, result),
            Err((code, message)) => self.error_response(request_id, code, message),
        }
    }

    pub(crate) async fn handle_command_exec_terminate(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: CommandExecTerminateParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid command/exec/terminate params: {error}"),
                );
            }
        };
        match self
            .command_exec_manager
            .terminate(connection_id, params)
            .await
        {
            Ok(result) => success_response(request_id, result),
            Err((code, message)) => self.error_response(request_id, code, message),
        }
    }

    async fn command_exec_cwd(
        &self,
        session_id: Option<SessionId>,
        cwd: Option<PathBuf>,
    ) -> CommandExecRuntimeResult<PathBuf> {
        if let Some(cwd) = cwd {
            return Ok(cwd);
        }
        let Some(session_id) = session_id else {
            return Err((
                ProtocolErrorCode::InvalidParams,
                "command/exec cwd is required when session_id is omitted".to_string(),
            ));
        };
        let session = self
            .sessions
            .lock()
            .await
            .get(&session_id)
            .cloned()
            .ok_or_else(|| {
                (
                    ProtocolErrorCode::SessionNotFound,
                    format!("session not found: {session_id}"),
                )
            })?;
        let Some(summary) = session.summary().await else {
            return Err((
                ProtocolErrorCode::InternalError,
                "failed to read session summary".to_string(),
            ));
        };
        Ok(summary.cwd)
    }
}

async fn spawn_command_exec_process(
    store_process_id: i32,
    program: CommandExecProgram,
    cwd: PathBuf,
    size: Option<CommandExecTerminalSize>,
) -> Result<(UnifiedExecProcess, broadcast::Receiver<Vec<u8>>), String> {
    match program {
        CommandExecProgram::OneShot { command } => {
            if command.trim().is_empty() {
                return Err("command/exec one-shot command must not be empty".to_string());
            }
            UnifiedExecProcess::spawn(
                store_process_id,
                &command,
                &cwd,
                /*shell*/ None,
                /*login*/ true,
                /*tty*/ true,
            )
            .await
        }
        CommandExecProgram::InteractiveShell => {
            UnifiedExecProcess::spawn_interactive_shell(
                store_process_id,
                &cwd,
                /*shell*/ None,
                /*login*/ true,
                size.map(protocol_terminal_size),
            )
            .await
        }
    }
}

fn validate_terminal_size(size: CommandExecTerminalSize) -> CommandExecRuntimeResult<()> {
    if size.rows == 0 || size.cols == 0 {
        return Err((
            ProtocolErrorCode::InvalidParams,
            "command/exec terminal size rows and cols must be greater than 0".to_string(),
        ));
    }
    Ok(())
}

fn protocol_terminal_size(size: CommandExecTerminalSize) -> TerminalSize {
    TerminalSize {
        rows: size.rows,
        cols: size.cols,
    }
}

fn success_response<T: serde::Serialize>(
    request_id: serde_json::Value,
    result: T,
) -> serde_json::Value {
    serde_json::to_value(SuccessResponse {
        id: request_id,
        result,
    })
    .expect("serialize command/exec response")
}
