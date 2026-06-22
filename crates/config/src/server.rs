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
    /// Server authentication gate configuration.
    #[serde(default)]
    pub auth: ServerAuthConfig,
}

/// Controls the optional server authentication gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerAuthConfig {
    /// Whether clients must authenticate before calling server methods.
    pub enabled: bool,
    /// ACP authentication method identifier advertised during initialization.
    pub method_id: String,
    /// Human-readable authentication method label.
    pub name: String,
    /// Optional authentication method description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the ACP `logout` method is advertised and supported.
    pub logout: bool,
}

impl Default for ServerAuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            method_id: "agent-login".to_string(),
            name: "Agent login".to_string(),
            description: None,
            logout: true,
        }
    }
}
