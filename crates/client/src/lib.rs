//! Client-side transport API for talking to a Devo server.
//!
//! The crate currently exposes the stdio client used by local frontends. It
//! keeps request/notification framing here so UI crates can call typed protocol
//! methods without owning process I/O or response demultiplexing.

mod stdio;

pub use stdio::*;
