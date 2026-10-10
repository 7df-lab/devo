use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use devo_core::tools::AgentToolCoordinator;
use devo_core::tools::unified_exec::process::TerminalSize;
use devo_core::tools::unified_exec::process::UnifiedExecProcess;
use devo_core::tools::unified_exec::store::ProcessStore;
use tokio::sync::Mutex;
use tokio::sync::broadcast;
use tokio::time::Duration;

use crate::ProtocolErrorCode;
use crate::SuccessResponse;
use crate::runtime::ServerRuntime;
use crate::runtime::connection::SubscriptionFilter;
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
use devo_protocol::native::event::ServerNotification;
use devo_protocol::native::ids::SessionId;

const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Whether interruption applies to one client or all commands of a deleted session.
pub(super) enum SessionCommandOwner {
    Connection(u64),
    AllConnections,
}
#[derive(Clone)]
pub(super) struct CommandExecManager {
    sessions: Arc<Mutex<HashMap<CommandExecKey, CommandExecSession>>>,
    store: Arc<ProcessStore>,
    /// Bounded per-process output tails for canonical `task/read`
    /// (`output_tail`); recorded alongside the live event stream.
    tails: Arc<Mutex<HashMap<CommandExecKey, Vec<u8>>>>,
    /// Terminal snapshots retained after a process exits (the live entry is
    /// removed on exit), so `task/read`/`task/list` stay meaningful for
    /// finished tasks. Insertion-bounded; oldest entries are dropped.
    completed: Arc<Mutex<VecDeque<CompletedTaskRecord>>>,
}

/// Terminal snapshot of an exited facade process task.
#[derive(Clone)]
pub(super) struct CompletedTaskRecord {
    pub(super) session_id: Option<SessionId>,
    pub(super) process_id: String,
    pub(super) command: String,
    pub(super) exit_code: Option<i32>,
    pub(super) tail: Vec<u8>,
}

/// Live-or-terminal snapshot of one facade process task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TaskProcessSnapshot {
    pub(super) process_id: String,
    pub(super) session_id: Option<SessionId>,
    pub(super) command: String,
    pub(super) is_running: bool,
    pub(super) exit_code: Option<i32>,
    pub(super) tail: Vec<u8>,
}

/// Byte cap for one retained output tail.
const TASK_OUTPUT_TAIL_LIMIT: usize = 16 * 1024;
/// How many exited task records are retained for reads.
const COMPLETED_TASK_RECORD_LIMIT: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CommandExecKey {
    connection_id: u64,
    session_id: Option<SessionId>,
    process_id: String,
}

#[derive(Clone)]
struct CommandExecSession {
    store_process_id: i32,
    command: String,
}

type CommandExecRuntimeResult<T> = Result<T, (ProtocolErrorCode, String)>;

impl CommandExecManager {
    pub(super) fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            store: Arc::new(ProcessStore::new()),
            tails: Arc::new(Mutex::new(HashMap::new())),
            completed: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Appends output to the process tail, bounded to
    /// `TASK_OUTPUT_TAIL_LIMIT` (front-truncated).
    async fn record_tail(&self, key: &CommandExecKey, bytes: &[u8]) {
        let mut tails = self.tails.lock().await;
        let tail = tails.entry(key.clone()).or_default();
        tail.extend_from_slice(bytes);
        if tail.len() > TASK_OUTPUT_TAIL_LIMIT {
            let overflow = tail.len() - TASK_OUTPUT_TAIL_LIMIT;
            tail.drain(..overflow);
        }
    }

    fn output_delta_notification(&self, key: &CommandExecKey, bytes: &[u8]) -> ServerNotification {
        ServerNotification::CommandExecOutputDelta {
            // boundary: UnifiedExecProcess key stores legacy SessionId only
            session_id: key.session_id,
            process_id: key.process_id.clone(),
            stream: CommandExecOutputStream::Pty,
            delta_base64: STANDARD.encode(bytes),
        }
    }

    async fn snapshot_for_key(&self, key: &CommandExecKey) -> TaskProcessSnapshot {
        // OS exit can precede the output pump. Only the retained record marks
        // completion, so terminal snapshots always include the drained tail.
        if let Some(record) = self.completed.lock().await.iter().find(|record| {
            record.session_id == key.session_id && record.process_id == key.process_id
        }) {
            return TaskProcessSnapshot {
                process_id: record.process_id.clone(),
                session_id: record.session_id,
                command: record.command.clone(),
                is_running: false,
                exit_code: record.exit_code,
                tail: record.tail.clone(),
            };
        }
        let command = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(key)
                .map(|session| session.command.clone())
                .unwrap_or_default()
        };
        let tail = self
            .tails
            .lock()
            .await
            .get(key)
            .cloned()
            .unwrap_or_default();
        TaskProcessSnapshot {
            process_id: key.process_id.clone(),
            session_id: key.session_id,
            command,
            is_running: true,
            exit_code: None,
            tail,
        }
    }

    /// Reads one facade process task by process id (any owning connection):
    /// the live entry first, then the retained terminal record.
    pub(super) async fn task_snapshot(&self, process_id: &str) -> Option<TaskProcessSnapshot> {
        // Drop the sessions lock before snapshotting: `snapshot_for_key`
        // re-acquires it for the command lookup, and the tokio mutex is not
        // reentrant — holding it across the await self-deadlocks every
        // live-process task/read (and, with the lock parked forever, every
        // later exec-manager operation).
        let live = {
            let sessions = self.sessions.lock().await;
            sessions
                .keys()
                .find(|key| key.process_id == process_id)
                .cloned()
        };
        if let Some(key) = live {
            return Some(self.snapshot_for_key(&key).await);
        }
        let completed = self.completed.lock().await;
        completed
            .iter()
            .find(|record| record.process_id == process_id)
            .map(|record| TaskProcessSnapshot {
                process_id: record.process_id.clone(),
                session_id: record.session_id,
                command: record.command.clone(),
                is_running: false,
                exit_code: record.exit_code,
                tail: record.tail.clone(),
            })
    }

    /// Lists live and recently-exited facade process tasks of one session.
    pub(super) async fn task_snapshots_for_session(
        &self,
        session_id: SessionId,
    ) -> Vec<TaskProcessSnapshot> {
        let mut snapshots = Vec::new();
        let keys: Vec<CommandExecKey> = {
            let sessions = self.sessions.lock().await;
            sessions
                .keys()
                .filter(|key| key.session_id == Some(session_id))
                .cloned()
                .collect()
        };
        for key in keys {
            snapshots.push(self.snapshot_for_key(&key).await);
        }
        let completed = self.completed.lock().await;
        for record in completed.iter() {
            if record.session_id == Some(session_id)
                && !snapshots
                    .iter()
                    .any(|snapshot| snapshot.process_id == record.process_id)
            {
                snapshots.push(TaskProcessSnapshot {
                    process_id: record.process_id.clone(),
                    session_id: record.session_id,
                    command: record.command.clone(),
                    is_running: false,
                    exit_code: record.exit_code,
                    tail: record.tail.clone(),
                });
            }
        }
        snapshots
    }

    async fn start(
        &self,
        runtime: Arc<ServerRuntime>,
        connection_id: u64,
        params: CommandExecParams,
        cwd: PathBuf,
        sandbox_profile: Option<String>,
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

        let command = match &params.program {
            CommandExecProgram::OneShot { command } => command.clone(),
            CommandExecProgram::InteractiveShell => String::new(),
        };
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
                sessions.insert(
                    key.clone(),
                    CommandExecSession {
                        store_process_id,
                        command: command.clone(),
                    },
                );
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

        let spawn_result = spawn_command_exec_process(
            store_process_id,
            params.program,
            cwd,
            params.size,
            sandbox_profile,
        )
        .await;
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

    /// Terminates session commands within the requested client ownership scope.
    pub(super) async fn terminate_session(
        &self,
        session_id: SessionId,
        owner: SessionCommandOwner,
    ) -> usize {
        let process_ids = {
            let sessions = self.sessions.lock().await;
            sessions
                .iter()
                .filter(|(key, _)| {
                    (match owner {
                        SessionCommandOwner::Connection(connection_id) => {
                            key.connection_id == connection_id
                        }
                        SessionCommandOwner::AllConnections => true,
                    }) && key.session_id == Some(session_id)
                })
                .map(|(_, session)| session.store_process_id)
                .collect::<Vec<_>>()
        };
        let mut terminated = 0;
        for process_id in process_ids {
            if let Some(process) = self.store.get(process_id).await
                && process.is_running()
            {
                process.terminate();
                terminated += 1;
            }
        }
        terminated
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
                                manager.record_tail(&key, &bytes).await;
                                runtime
                                    .emit_notification_to_connection(
                                        key.connection_id,
                                        manager.output_delta_notification(&key, &bytes),
                                    )
                                    .await;
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    _ = tokio::time::sleep(EXIT_POLL_INTERVAL) => {}
                }

                if !process.is_running() && process.exit_code().is_some() {
                    break;
                }
            }

            while let Ok(bytes) = output_rx.try_recv() {
                manager.record_tail(&key, &bytes).await;
                runtime
                    .emit_notification_to_connection(
                        key.connection_id,
                        manager.output_delta_notification(&key, &bytes),
                    )
                    .await;
            }

            let command = {
                let sessions = manager.sessions.lock().await;
                sessions
                    .get(&key)
                    .map(|session| session.command.clone())
                    .unwrap_or_default()
            };
            let tail = manager
                .tails
                .lock()
                .await
                .get(&key)
                .cloned()
                .unwrap_or_default();
            {
                let mut completed = manager.completed.lock().await;
                completed.retain(|record| {
                    !(record.session_id == key.session_id && record.process_id == key.process_id)
                });
                completed.push_back(CompletedTaskRecord {
                    session_id: key.session_id,
                    process_id: key.process_id.clone(),
                    command: command.clone(),
                    exit_code: process.exit_code(),
                    tail: tail.clone(),
                });
                while completed.len() > COMPLETED_TASK_RECORD_LIMIT {
                    completed.pop_front();
                }
            }
            let notification = ServerNotification::CommandExecExited {
                // boundary: UnifiedExecProcess key stores legacy SessionId only
                session_id: key.session_id,
                process_id: key.process_id.clone(),
                exit_code: process.exit_code(),
            };
            runtime
                .emit_notification_to_connection(key.connection_id, notification)
                .await;
            if let Some(session_id) = key.session_id {
                runtime
                    .persist_shell_background_task(
                        session_id,
                        &key.process_id,
                        &command,
                        process.exit_code(),
                        &tail,
                    )
                    .await;
            }
            manager.remove_key(&key).await;
        });
    }

    async fn remove_key(&self, key: &CommandExecKey) {
        self.tails.lock().await.remove(key);
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
            let mut tails = self.tails.lock().await;
            for key in &keys {
                tails.remove(key);
            }
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
        self.tails.lock().await.clear();
        self.completed.lock().await.clear();
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
        self.handle_command_exec_translated(connection_id, request_id, params)
            .await
    }

    /// Native `task/start` (L2-DES-APP-008 DD-7, unified task model).
    /// `kind: "process"` translates onto the command/exec machinery with the
    /// item id doubling as the process id; `kind: "agent"` is a child session
    /// projected as a task item.
    pub(crate) async fn handle_native_task_start(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_turn::TaskStartParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical task/start params: {error}"),
                    );
                }
            };
        let (session_id, input, idempotency_key, agent_policies) = match params {
            devo_protocol::native::rpc_turn::TaskStartParams::Process {
                session_id,
                command,
                cwd,
                idempotency_key,
            } => {
                let legacy_session_id = session_id;
                if let Some(item_id) = self
                    .task_start_idempotency
                    .lock()
                    .await
                    .get(&(legacy_session_id, idempotency_key.clone()))
                    .cloned()
                {
                    return success_response(
                        request_id,
                        devo_protocol::native::rpc_turn::TaskStartResult {
                            item_id: devo_protocol::native::ids::ItemId::from_string(item_id),
                        },
                    );
                }
                let item_id = devo_protocol::native::ids::ItemId::new();
                let response = self
                    .handle_command_exec_translated(
                        connection_id,
                        request_id.clone(),
                        CommandExecParams {
                            session_id: Some(legacy_session_id),
                            process_id: item_id.as_str().to_string(),
                            cwd,
                            program: devo_protocol::CommandExecProgram::OneShot { command },
                            size: None,
                        },
                    )
                    .await;
                if response.get("error").is_some() {
                    return response;
                }
                self.task_start_idempotency.lock().await.insert(
                    (legacy_session_id, idempotency_key),
                    item_id.as_str().to_string(),
                );
                return success_response(
                    request_id,
                    devo_protocol::native::rpc_turn::TaskStartResult { item_id },
                );
            }
            devo_protocol::native::rpc_turn::TaskStartParams::Agent {
                session_id,
                input,
                fork_turns,
                max_turns,
                tool_policy,
                ephemeral,
                idempotency_key,
            } => {
                let message = input
                    .iter()
                    .filter_map(|item| match item {
                        devo_protocol::native::item::UserInput::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if message.is_empty() {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        "task/start kind=agent requires at least one text input",
                    );
                }
                (
                    session_id,
                    message,
                    idempotency_key,
                    Some((fork_turns, max_turns, tool_policy, ephemeral)),
                )
            }
        };
        let legacy_session_id = session_id;
        if let Some(item_id) = self
            .task_start_idempotency
            .lock()
            .await
            .get(&(session_id, idempotency_key.clone()))
            .cloned()
        {
            return success_response(
                request_id,
                devo_protocol::native::rpc_turn::TaskStartResult {
                    item_id: devo_protocol::native::ids::ItemId::from_string(item_id),
                },
            );
        }
        let (fork_turns, max_turns, tool_policy, ephemeral) =
            agent_policies.expect("agent branch always carries policies");
        let result = match Arc::clone(self)
            .spawn_agent(devo_protocol::SpawnAgentParams {
                session_id: legacy_session_id,
                message: input,
                fork_turns,
                max_turns,
                tool_policy: tool_policy.unwrap_or(devo_protocol::AgentToolPolicy::Inherit),
                ephemeral,
                nickname: None,
            })
            .await
        {
            Ok(result) => result,
            Err(error) => return self.tool_error_response(request_id, error),
        };
        let item_id = devo_protocol::native::ids::ItemId::from_string(format!(
            "item_{}",
            result.child_session_id
        ));
        self.task_start_idempotency
            .lock()
            .await
            .insert((session_id, idempotency_key), item_id.as_str().to_string());
        success_response(
            request_id,
            devo_protocol::native::rpc_turn::TaskStartResult { item_id },
        )
    }

    /// Native `task/interrupt` (DD-7): the item id doubles as the process
    /// id in the current facade; the owning session is recovered from the
    /// task/start idempotency map (the terminate path keys processes by
    /// `(connection, session, process)`).
    pub(crate) async fn handle_native_task_interrupt(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_turn::TaskInterruptParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical task/interrupt params: {error}"),
                    );
                }
            };
        let owning_session = self.task_owning_session(params.item_id.as_str()).await;
        self.handle_command_exec_terminate(
            connection_id,
            request_id,
            serde_json::json!({
                "session_id": owning_session,
                "process_id": params.item_id.as_str(),
            }),
        )
        .await
    }

    /// Native `task/write_stdin` (DD-7 facade): base64-encoded write into
    /// the process pty.
    pub(crate) async fn handle_native_task_write_stdin(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_turn::TaskWriteStdinParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical task/write_stdin params: {error}"),
                    );
                }
            };
        let owning_session = self.task_owning_session(params.item_id.as_str()).await;
        self.handle_command_exec_write(
            connection_id,
            request_id,
            serde_json::json!({
                "session_id": owning_session,
                "process_id": params.item_id.as_str(),
                "delta_base64": STANDARD.encode(params.data.as_bytes()),
                "close_stdin": false,
            }),
        )
        .await
    }

    /// Native `task/resize` (DD-7 facade).
    pub(crate) async fn handle_native_task_resize(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_turn::TaskResizeParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical task/resize params: {error}"),
                    );
                }
            };
        let owning_session = self.task_owning_session(params.item_id.as_str()).await;
        self.handle_command_exec_resize(
            connection_id,
            request_id,
            serde_json::json!({
                "session_id": owning_session,
                "process_id": params.item_id.as_str(),
                "size": {
                    "rows": params.rows,
                    "cols": params.cols,
                },
            }),
        )
        .await
    }

    /// Recovers the owning session of a facade task from the task/start
    /// idempotency map (exec processes are keyed by
    /// `(connection, session, process)`).
    async fn task_owning_session(&self, item_id: &str) -> Option<SessionId> {
        self.task_start_idempotency
            .lock()
            .await
            .iter()
            .find(|((_, _), stored_item_id)| stored_item_id.as_str() == item_id)
            .map(|((session_id, _), _)| *session_id)
    }

    /// Native `task/read` (DD-7): one background task's item snapshot.
    /// Agent items resolve through the agent facade; process items carry the
    /// captured output tail.
    pub(crate) async fn handle_native_task_read(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_turn::TaskReadParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical task/read params: {error}"),
                    );
                }
            };
        if let Some((parent_session_id, child_session_id)) =
            self.agent_item_target(params.item_id.as_str()).await
        {
            let info = match self
                .agent_info(parent_session_id, child_session_id.as_ref())
                .await
            {
                Ok(info) => info,
                Err(error) => return self.tool_error_response(request_id, error),
            };
            return success_response(
                request_id,
                devo_protocol::native::rpc_turn::TaskReadResult {
                    item: crate::runtime::agents::handlers::subagent_item_from_agent_info(&info),
                    output_tail: None,
                },
            );
        }
        if let Some(snapshot) = self
            .command_exec_manager
            .task_snapshot(params.item_id.as_str())
            .await
        {
            let output_tail = (!snapshot.tail.is_empty())
                .then(|| String::from_utf8_lossy(&snapshot.tail).into_owned());
            return success_response(
                request_id,
                devo_protocol::native::rpc_turn::TaskReadResult {
                    item: background_task_item_from_snapshot(&snapshot),
                    output_tail,
                },
            );
        }
        self.error_response(
            request_id,
            ProtocolErrorCode::SessionNotFound,
            "task is not addressable by this server",
        )
    }

    /// Native `task/list` (DD-7): all background tasks of the session —
    /// process tasks from the exec manager and agent tasks from the agent
    /// registry — as item envelopes.
    pub(crate) async fn handle_native_task_list(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_turn::TaskListParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical task/list params: {error}"),
                    );
                }
            };
        let legacy_session_id = params.session_id;
        let agents = match Arc::clone(self)
            .list_agents(devo_protocol::AgentListParams {
                session_id: legacy_session_id,
                path_prefix: None,
            })
            .await
        {
            Ok(agents) => agents,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to list agent tasks: {error}"),
                );
            }
        };
        let mut tasks: Vec<devo_protocol::native::item::ItemEnvelope> = agents
            .iter()
            .map(crate::runtime::agents::handlers::subagent_item_from_agent_info)
            .collect();
        let snapshots = self
            .command_exec_manager
            .task_snapshots_for_session(legacy_session_id)
            .await;
        tasks.extend(snapshots.iter().map(background_task_item_from_snapshot));
        success_response(
            request_id,
            devo_protocol::native::rpc_turn::TaskListResult { tasks },
        )
    }

    async fn handle_command_exec_translated(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: CommandExecParams,
    ) -> serde_json::Value {
        let cwd = match self
            .command_exec_cwd(params.session_id, params.cwd.clone())
            .await
        {
            Ok(cwd) => cwd,
            Err((code, message)) => return self.error_response(request_id, code, message),
        };
        let sandbox_profile = self
            .command_exec_sandbox_profile(params.session_id, &cwd)
            .await;
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
            .start(
                Arc::clone(self),
                connection_id,
                params,
                cwd,
                sandbox_profile,
            )
            .await
        {
            Ok(result) => success_response(request_id, result),
            Err((code, message)) => self.error_response(request_id, code, message),
        }
    }

    /// The session's sandbox profile for a command/exec spawn, or `None` for
    /// sessionless execs and sessions without a profile. Resolved through the
    /// session actor so PTY spawns enforce the same profile as tool-run shells.
    async fn command_exec_sandbox_profile(
        &self,
        session_id: Option<SessionId>,
        cwd: &std::path::Path,
    ) -> Option<String> {
        let session = self.sessions.lock().await.get(&session_id?).cloned()?;
        session
            .shell_exec_context(cwd.to_path_buf())
            .await
            .and_then(|context| context.sandbox_profile)
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
        Ok(summary.cwd.clone())
    }

    /// Persist a completed user `!` shell task into the session rollout so
    /// resume can rebuild BashExecution chrome from `session/items/list`.
    pub(super) async fn persist_shell_background_task(
        &self,
        session_id: SessionId,
        process_id: &str,
        command: &str,
        exit_code: Option<i32>,
        tail: &[u8],
    ) {
        if command.trim().is_empty() {
            return;
        }
        use chrono::Utc;
        use devo_protocol::native::ids::{ItemId, TurnId};
        use devo_protocol::native::item::{
            BackgroundTaskKind, Item, ItemEnvelope, ItemState, SpawnedWorkState,
        };

        let output = String::from_utf8_lossy(tail).into_owned();
        let item = Item::BackgroundTask {
            origin_call_id: None,
            task_kind: BackgroundTaskKind::Shell,
            state: SpawnedWorkState::Completed,
            execution_handle: Some(process_id.to_string()),
            cwd: None,
            exit_code,
            command: Some(command.to_string()),
            output: (!output.is_empty()).then_some(output),
        };
        if let Some(session_handle) = self.session(session_id).await {
            session_handle
                .append_history_item(devo_protocol::SessionHistoryEntry::item(item.clone()))
                .await;
        }
        let Some(rollout_path) = self.session_rollout_path(session_id).await else {
            return;
        };
        let now = Utc::now();
        let envelope = ItemEnvelope {
            id: ItemId::from_string(process_id.to_string()),
            session_id,
            // boundary: user bash is session-scoped, not turn-scoped yet
            turn_id: TurnId::from_string(session_id.to_string()),
            seq: u64::try_from(now.timestamp_millis().max(0)).unwrap_or(0),
            revision: 1,
            created_at: now,
            updated_at: now,
            state: ItemState::Completed,
            item,
            parent_id: None,
        };
        if let Err(error) = self
            .rollout_store
            .append_canonical_item_at(&rollout_path, envelope)
        {
            tracing::warn!(
                session_id = %session_id,
                process_id,
                error = %error,
                "failed to persist shell background task"
            );
        }
    }
}

async fn spawn_command_exec_process(
    store_process_id: i32,
    program: CommandExecProgram,
    cwd: PathBuf,
    size: Option<CommandExecTerminalSize>,
    sandbox_profile: Option<String>,
) -> Result<(UnifiedExecProcess, broadcast::Receiver<Vec<u8>>), String> {
    match program {
        CommandExecProgram::OneShot { command } => {
            if command.trim().is_empty() {
                return Err("command/exec one-shot command must not be empty".to_string());
            }
            UnifiedExecProcess::spawn_with_sandbox(
                store_process_id,
                &command,
                &cwd,
                /*shell*/ None,
                /*login*/ true,
                /*tty*/ true,
                sandbox_profile,
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
                sandbox_profile,
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

/// Projects one facade process-task snapshot into a canonical
/// `BackgroundTask` item envelope (DD-7 facade). `turn_id` is a
/// session-derived placeholder until tasks become first-class items, the
/// same concession the agent facade documents. A nonzero exit code is a
/// completed shell task, not a protocol-level failure, so the state only
/// distinguishes running from terminal and `exit_code` carries the detail.
fn background_task_item_from_snapshot(
    snapshot: &TaskProcessSnapshot,
) -> devo_protocol::native::item::ItemEnvelope {
    use devo_protocol::native::ids::{ItemId, TurnId};
    use devo_protocol::native::item::{
        BackgroundTaskKind, Item, ItemEnvelope, ItemState, SpawnedWorkState,
    };

    let native_session_id = snapshot.session_id.unwrap_or_default();
    let (state, item_state) = if snapshot.is_running {
        (SpawnedWorkState::Running, ItemState::Running)
    } else {
        (SpawnedWorkState::Completed, ItemState::Completed)
    };
    let now = chrono::Utc::now();
    ItemEnvelope {
        id: ItemId::from_string(snapshot.process_id.clone()),
        // boundary: TaskProcessSnapshot stores native session id only (no turn)
        session_id: native_session_id,
        turn_id: TurnId::from_string(native_session_id.to_string()),
        seq: 0,
        revision: 1,
        created_at: now,
        updated_at: now,
        state: item_state,
        item: Item::BackgroundTask {
            origin_call_id: None,
            task_kind: BackgroundTaskKind::Shell,
            state,
            execution_handle: Some(snapshot.process_id.clone()),
            cwd: None,
            exit_code: snapshot.exit_code,
            command: (!snapshot.command.is_empty()).then(|| snapshot.command.clone()),
            output: {
                let text = String::from_utf8_lossy(&snapshot.tail).into_owned();
                (!text.is_empty()).then_some(text)
            },
        },
        parent_id: None,
    }
}

#[cfg(test)]
#[path = "command_exec_tests.rs"]
mod tests;
