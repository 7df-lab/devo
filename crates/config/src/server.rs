use serde::{Deserialize, Serialize};

/// Stores transport and connection-management defaults for the runtime server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    /// The websocket listener addresses the server should bind to by default.
    pub listen: Vec<String>,
    /// The maximum number of simultaneous client connections.
    pub max_connections: u32,
    /// The per-connection event buffer size used for streaming notifications.
    pub event_buffer_size: usize,
    /// The idle timeout applied to loaded sessions, in seconds.
    pub idle_session_timeout_secs: u64,
    /// Whether ephemeral sessions should be persisted despite their transient nature.
    pub persist_ephemeral_sessions: bool,
}
