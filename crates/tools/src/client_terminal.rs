use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::contracts::ToolCallError;

/// Environment variable passed to a client-owned terminal command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientTerminalEnv {
    pub name: String,
    pub value: String,
}

/// Request to create a terminal command in the active client environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientTerminalCreateRequest {
    pub session_id: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<ClientTerminalEnv>,
    pub cwd: Option<PathBuf>,
    pub output_byte_limit: Option<usize>,
}

/// Request targeting an existing client terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientTerminalRequest {
    pub session_id: String,
    pub terminal_id: String,
}

/// Result of an optional client-backed terminal create operation.
///
/// Implementations return `Unsupported` when the connected client did not
/// advertise ACP `terminal` support. Tool handlers should then fall back to
/// their normal server-side command execution behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientTerminalCreate {
    Unsupported,
    Created { terminal_id: String },
}

/// Terminal process exit status reported by the active client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientTerminalExitStatus {
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
}

/// Snapshot of output retained by the active client terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientTerminalOutput {
    pub output: String,
    pub truncated: bool,
    pub exit_status: Option<ClientTerminalExitStatus>,
}

/// Runtime bridge for client-owned terminal execution.
///
/// Implementations should call terminal capabilities exposed by the active
/// client, such as ACP `terminal/create`, `terminal/output`,
/// `terminal/wait_for_exit`, `terminal/kill`, and `terminal/release`.
/// Tool handlers use `Unsupported` to keep server-side fallback behavior when
/// no client terminal capability is available.
#[async_trait]
pub trait ClientTerminal: Send + Sync {
    async fn create(
        self: Arc<Self>,
        _request: ClientTerminalCreateRequest,
        _cancel_token: CancellationToken,
    ) -> Result<ClientTerminalCreate, ToolCallError> {
        Ok(ClientTerminalCreate::Unsupported)
    }

    async fn output(
        self: Arc<Self>,
        _request: ClientTerminalRequest,
        _cancel_token: CancellationToken,
    ) -> Result<ClientTerminalOutput, ToolCallError>;

    async fn wait_for_exit(
        self: Arc<Self>,
        _request: ClientTerminalRequest,
        _timeout: Duration,
        _cancel_token: CancellationToken,
    ) -> Result<ClientTerminalExitStatus, ToolCallError>;

    async fn kill(
        self: Arc<Self>,
        _request: ClientTerminalRequest,
        _cancel_token: CancellationToken,
    ) -> Result<(), ToolCallError>;

    async fn release(
        self: Arc<Self>,
        _request: ClientTerminalRequest,
        _cancel_token: CancellationToken,
    ) -> Result<(), ToolCallError>;
}
