//! Basic MCP server manager implementation.
//!
//! Implements the McpManager trait with in-memory server status tracking.
//! Full transport/capability discovery requires MCP JSON-RPC client code.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::{
    McpAuthState, McpError, McpManager, McpServerId, McpServerRecord, McpServerStatus,
    McpStartupState,
};

/// Basic in-memory MCP server manager with status tracking.
#[derive(Debug, Clone)]
pub struct InMemoryMcpManager {
    servers: Arc<RwLock<HashMap<McpServerId, McpServerStatus>>>,
}

impl InMemoryMcpManager {
    pub fn new() -> Self {
        Self {
            servers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register_server(&self, record: McpServerRecord) {
        let id = record.id;
        let state = if record.enabled {
            McpStartupState::NotStarted
        } else {
            McpStartupState::Disabled
        };
        let status = McpServerStatus {
            server_id: id,
            startup_state: state,
            auth_state: McpAuthState::NotRequired,
            tools: Vec::new(),
            resources: Vec::new(),
            resource_templates: Vec::new(),
            last_refreshed_at: None,
        };
        let key = status.server_id.clone();
        self.servers.write().await.insert(key, status);
    }
}

impl Default for InMemoryMcpManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl McpManager for InMemoryMcpManager {
    async fn statuses(&self) -> Result<Vec<McpServerStatus>, McpError> {
        Ok(self.servers.read().await.values().cloned().collect())
    }

    async fn refresh(&self, server_id: &McpServerId) -> Result<McpServerStatus, McpError> {
        let mut servers = self.servers.write().await;
        let status = servers
            .get_mut(server_id)
            .ok_or_else(|| McpError::McpServerUnavailable {
                server_id: server_id.clone(),
            })?;
        status.last_refreshed_at = Some(chrono::Utc::now());
        status.startup_state = McpStartupState::Ready;
        Ok(status.clone())
    }

    async fn invoke_tool(
        &self,
        server_id: &McpServerId,
        tool_name: &str,
        _input: serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        let servers = self.servers.read().await;
        let status = servers
            .get(server_id)
            .ok_or_else(|| McpError::McpServerUnavailable {
                server_id: server_id.clone(),
            })?;
        if status.startup_state != McpStartupState::Ready {
            return Err(McpError::McpServerUnavailable {
                server_id: server_id.clone(),
            });
        }
        Err(McpError::McpToolInvocationFailed {
            server_id: server_id.clone(),
            tool_name: tool_name.to_string(),
            message: "MCP transport not yet implemented".into(),
        })
    }

    async fn read_resource(
        &self,
        server_id: &McpServerId,
        uri: &str,
    ) -> Result<serde_json::Value, McpError> {
        let servers = self.servers.read().await;
        let status = servers
            .get(server_id)
            .ok_or_else(|| McpError::McpServerUnavailable {
                server_id: server_id.clone(),
            })?;
        if status.startup_state != McpStartupState::Ready {
            return Err(McpError::McpServerUnavailable {
                server_id: server_id.clone(),
            });
        }
        Err(McpError::McpResourceReadFailed {
            server_id: server_id.clone(),
            uri: uri.to_string(),
            message: "MCP transport not yet implemented".into(),
        })
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{McpServerId, McpStartupPolicy, McpTransportConfig};

    fn make_record(id: McpServerId) -> McpServerRecord {
        McpServerRecord {
            id,
            display_name: "Test Server".into(),
            transport: McpTransportConfig::Stdio {
                command: vec!["echo".into()],
                cwd: None,
                env: std::collections::BTreeMap::new(),
            },
            startup_policy: McpStartupPolicy::Manual,
            enabled: true,
            trust_policy: crate::McpTrustPolicy::User,
            allowed_capabilities: vec![],
            roots_policy: crate::McpRootsPolicy::None,
            output_limits: crate::McpOutputLimits::default(),
            auth_ref: None,
        }
    }

    #[tokio::test]
    async fn register_and_query_status() {
        let manager = InMemoryMcpManager::new();
        let id = McpServerId("test-server".into());
        manager.register_server(make_record(id.clone())).await;
        let statuses = manager.statuses().await.expect("statuses");
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].startup_state, McpStartupState::NotStarted);
    }

    #[tokio::test]
    async fn refresh_updates_status() {
        let manager = InMemoryMcpManager::new();
        let id = McpServerId("test-server".into());
        manager.register_server(make_record(id.clone())).await;
        let result = manager.refresh(&id).await.expect("refresh");
        assert_eq!(result.startup_state, McpStartupState::Ready);
        assert!(result.last_refreshed_at.is_some());
    }

    #[tokio::test]
    async fn invoke_tool_without_transport_errors() {
        let manager = InMemoryMcpManager::new();
        let id = McpServerId("test-server".into());
        manager.register_server(make_record(id.clone())).await;
        // Server not started yet — should error
        let result = manager
            .invoke_tool(&id, "test", serde_json::json!({}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn unknown_server_errors() {
        let manager = InMemoryMcpManager::new();
        let id = McpServerId("unknown".into());
        let result = manager.refresh(&id).await;
        assert!(result.is_err());
    }
}
