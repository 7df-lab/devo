use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::contracts::ToolCallError;

/// Result of an optional client-backed text-file read.
///
/// Implementations return `Unsupported` when the connected client did not
/// advertise the ACP `fs.readTextFile` capability. Tool handlers should then
/// fall back to their normal server-side filesystem behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientTextFileRead {
    Unsupported,
    Content(String),
}

/// Result of an optional client-backed text-file write.
///
/// Implementations return `Unsupported` when the connected client did not
/// advertise the ACP `fs.writeTextFile` capability. Tool handlers should then
/// fall back to their normal server-side filesystem behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientTextFileWrite {
    Unsupported,
    Written,
}

/// Runtime bridge for client-owned filesystem access.
///
/// Implementations should call filesystem capabilities exposed by the active
/// client, such as ACP `fs/read_text_file` and `fs/write_text_file`. Tool
/// handlers use `Unsupported` to keep their server-side fallback behavior when
/// no client filesystem capability is available.
#[async_trait]
pub trait ClientFilesystem: Send + Sync {
    async fn read_text_file(
        self: Arc<Self>,
        _session_id: String,
        _path: PathBuf,
        _line: Option<u64>,
        _limit: Option<u64>,
        _cancel_token: CancellationToken,
    ) -> Result<ClientTextFileRead, ToolCallError> {
        Ok(ClientTextFileRead::Unsupported)
    }

    async fn write_text_file(
        self: Arc<Self>,
        _session_id: String,
        _path: PathBuf,
        _content: String,
        _cancel_token: CancellationToken,
    ) -> Result<ClientTextFileWrite, ToolCallError> {
        Ok(ClientTextFileWrite::Unsupported)
    }
}
