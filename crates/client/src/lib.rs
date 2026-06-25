//! Client-side transport API for talking to a Devo server.
//!
//! The crate currently exposes the stdio client used by local frontends. It
//! keeps request/notification framing here so UI crates can call typed protocol
//! methods without owning process I/O or response demultiplexing.

mod acp_fs;
mod acp_permissions;
mod acp_terminal;
mod client_core;
mod stdio;
mod websocket;

pub use stdio::*;
pub use websocket::*;
