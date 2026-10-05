//! CPython RLM REPL kernel host (REPL protocol v3).
//!
//! Spawns `python -m rlm.repl`, speaks newline-delimited UTF-8 JSON, and keeps
//! a session-scoped namespace across turns. See `L2-DES-RLM-001`.

#[cfg(unix)]
mod credential_unix;

#[cfg(unix)]
pub use credential_unix::GrantChannel;
mod fence;
mod protocol;
mod session;

pub use fence::FenceState;
pub use protocol::{KernelEvent, KernelRequest, PROTOCOL_VERSION, ReadyEvent, ReplError};
pub use session::{
    ATTACHMENT_DISPLAY_MIME, CellAttachment, CellDiffDisplay, CellOutput, CellWaitOutcome,
    DIFF_DISPLAY_MIME, ExecutionSurface, HostRequestHandler, InFlightCell, KernelFenceSpec,
    KernelSession, KernelSessionConfig, MAX_ATTACHMENT_DATA_CHARS, SpawnError, WorkspaceAccess,
    default_runtime_pythonpath, deny_host_handler, parse_attachment_display, parse_diff_display,
    parse_host_request_data,
};
