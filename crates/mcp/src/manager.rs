//! MCP server manager backed by `devo-rmcp-client`.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use devo_config::OAuthCredentialsStoreMode;
use devo_rmcp_client::ElicitationAction;
use devo_rmcp_client::ElicitationResponse;
use devo_rmcp_client::LocalStdioServerLauncher;
use devo_rmcp_client::RmcpClient;
use rmcp::model::ClientCapabilities;
use rmcp::model::Implementation;
use rmcp::model::InitializeRequestParams;
use rmcp::model::ProtocolVersion;
use rmcp::model::Tool;
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::warn;

#[path = "reconnect.rs"]
mod reconnect;
use reconnect::OperationSlot;

use devo_core::mcp::McpAuthConfig;
use devo_core::mcp::McpAuthState;
use devo_core::mcp::McpCapability;
use devo_core::mcp::McpConfig;
use devo_core::mcp::McpError;
use devo_core::mcp::McpManager;
use devo_core::mcp::McpServerId;
use devo_core::mcp::McpServerRecord;
use devo_core::mcp::McpServerStatus;
use devo_core::mcp::McpStartupPolicy;
use devo_core::mcp::McpStartupState;
use devo_core::mcp::McpToolDescriptor;
use devo_core::mcp::McpToolInfo;
use devo_core::mcp::McpTransportConfig;

const MCP_OPERATION_TIMEOUT: Duration = Duration::from_secs(10);

/// Runtime MCP manager that owns active RMCP clients.
pub struct RmcpMcpManager {
    config: RwLock<McpConfig>,
    operation_gate: tokio::sync::Mutex<()>,
    operations: HashMap<McpServerId, Arc<OperationSlot>>,
    tool_cache: RwLock<HashMap<McpServerId, Vec<McpToolInfo>>>,
    oauth_store_mode: OAuthCredentialsStoreMode,
    clients: RwLock<HashMap<McpServerId, Arc<RmcpClient>>>,
    statuses: RwLock<HashMap<McpServerId, McpServerStatus>>,
}

impl RmcpMcpManager {
    pub fn new(config: McpConfig, oauth_store_mode: OAuthCredentialsStoreMode) -> Self {
        let server_count = config.servers.len();
        let mut statuses = HashMap::with_capacity(server_count);
        for record in &config.servers {
            let server_id = record.id.clone();
            let startup_state = if record.enabled {
                McpStartupState::NotStarted
            } else {
                McpStartupState::Disabled
            };
            statuses.insert(server_id.clone(), empty_status(server_id, startup_state));
        }

        let operations = config
            .servers
            .iter()
            .map(|record| (record.id.clone(), Arc::new(OperationSlot::default())))
            .collect();
        Self {
            operation_gate: tokio::sync::Mutex::new(()),
            operations,
            tool_cache: RwLock::new(HashMap::new()),
            config: RwLock::new(config),
            oauth_store_mode,
            clients: RwLock::new(HashMap::with_capacity(server_count)),
            statuses: RwLock::new(statuses),
        }
    }

    async fn ensure_client(&self, record: &McpServerRecord) -> Result<Arc<RmcpClient>, McpError> {
        if let Some(client) = self.clients.read().await.get(&record.id).cloned() {
            return Ok(client);
        }
        self.refresh(&record.id).await?;
        self.clients
            .read()
            .await
            .get(&record.id)
            .cloned()
            .ok_or_else(|| McpError::McpServerUnavailable {
                server_id: record.id.clone(),
            })
    }

    async fn record(&self, server_id: &McpServerId) -> Result<McpServerRecord, McpError> {
        self.config
            .read()
            .await
            .servers
            .iter()
            .find(|record| &record.id == server_id)
            .cloned()
            .ok_or_else(|| McpError::McpServerUnavailable {
                server_id: server_id.clone(),
            })
    }

    async fn discoverable_tool_records(&self) -> Vec<McpServerRecord> {
        let config = self.config.read().await;
        config
            .servers
            .iter()
            .filter(|record| {
                record.enabled
                    && tools_capability_allowed(record)
                    && config.auto_start
                    && !matches!(record.startup_policy, McpStartupPolicy::Manual)
            })
            .cloned()
            .collect()
    }
}

fn empty_status(server_id: McpServerId, startup_state: McpStartupState) -> McpServerStatus {
    McpServerStatus {
        server_id,
        startup_state,
        auth_state: McpAuthState::NotRequired,
        tools: Vec::new(),
        resources: Vec::new(),
        resource_templates: Vec::new(),
        last_refreshed_at: None,
    }
}

#[async_trait]
impl McpManager for RmcpMcpManager {
    async fn statuses(&self) -> Result<Vec<McpServerStatus>, McpError> {
        Ok(self
            .statuses
            .read()
            .await
            .values()
            .cloned()
            .map(|mut status| {
                // Cancellation releases the reservation synchronously. Never leave
                // an orphan Starting state that prevents a later explicit retry.
                if status.startup_state == McpStartupState::Starting
                    && !self.operations[&status.server_id].is_busy()
                {
                    status.startup_state = McpStartupState::Stopped;
                }
                status
            })
            .collect())
    }

    async fn discover_tools(&self) -> Result<Vec<McpToolInfo>, McpError> {
        let records = self.discoverable_tool_records().await;
        let mut tools = Vec::new();
        for record in records {
            match self
                .discover_server(&record.id, reconnect::ConnectionMode::Reuse)
                .await
            {
                Ok((_, discovered)) => tools.extend(discovered),
                Err(err) => {
                    warn!(server_id = %record.id, error = %err, "failed to refresh MCP server")
                }
            }
        }
        // Manual/auto-start-disabled servers must not be started implicitly,
        // but a verified explicit connection still belongs in later registries.
        let connected_manual = {
            let config = self.config.read().await;
            config
                .servers
                .iter()
                .filter(|record| {
                    record.enabled
                        && tools_capability_allowed(record)
                        && (!config.auto_start
                            || matches!(record.startup_policy, McpStartupPolicy::Manual))
                })
                .map(|record| record.id.clone())
                .collect::<Vec<_>>()
        };
        for server_id in connected_manual {
            let slot = &self.operations[&server_id];
            loop {
                let completed = slot.completed.notified();
                tokio::pin!(completed);
                completed.as_mut().enable();
                let gate = self.operation_gate.lock().await;
                if !slot.is_busy() {
                    tools.extend(self.cached_tools(&server_id).await?);
                    break;
                }
                // Observe an explicit connection attempt without starting or
                // retrying a manual server, even if that attempt is cancelled.
                drop(gate);
                completed.await;
            }
        }
        Ok(tools)
    }

    async fn refresh(&self, server_id: &McpServerId) -> Result<McpServerStatus, McpError> {
        self.discover_server(server_id, reconnect::ConnectionMode::Reuse)
            .await
            .map(|(status, _)| status)
    }

    async fn reconnect(&self, server_id: &McpServerId) -> Result<McpServerStatus, McpError> {
        self.discover_server(server_id, reconnect::ConnectionMode::Replace)
            .await
            .map(|(status, _)| status)
    }

    async fn cached_tools(&self, server_id: &McpServerId) -> Result<Vec<McpToolInfo>, McpError> {
        self.record(server_id).await?;
        Ok(self
            .tool_cache
            .read()
            .await
            .get(server_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn configure_enabled(
        &self,
        server_id: &McpServerId,
        enabled: bool,
    ) -> Result<McpServerStatus, McpError> {
        let _gate = self.operation_gate.lock().await;
        let mut config = self.config.write().await;
        let record = config
            .servers
            .iter_mut()
            .find(|record| &record.id == server_id)
            .ok_or_else(|| McpError::McpServerUnavailable {
                server_id: server_id.clone(),
            })?;
        if enabled && record.enabled {
            let mut status = self.statuses.read().await[server_id].clone();
            if status.startup_state == McpStartupState::Starting
                && !self.operations[server_id].is_busy()
            {
                status.startup_state = McpStartupState::Stopped;
            }
            return Ok(status);
        }
        record.enabled = enabled;
        self.operations[server_id].invalidate();
        self.clients.write().await.remove(server_id);
        self.tool_cache.write().await.remove(server_id);
        let state = if enabled {
            McpStartupState::NotStarted
        } else {
            McpStartupState::Disabled
        };
        let status = empty_status(server_id.clone(), state);
        self.statuses
            .write()
            .await
            .insert(server_id.clone(), status.clone());
        Ok(status)
    }

    async fn set_enabled(
        &self,
        server_id: &McpServerId,
        enabled: bool,
    ) -> Result<McpServerStatus, McpError> {
        let status = self.configure_enabled(server_id, enabled).await?;
        if !enabled {
            return Ok(status);
        }

        match self.refresh(server_id).await {
            Ok(status) => Ok(status),
            Err(err) => {
                // Persist succeeded; surface Failed status so callers can keep
                // going while siblings remain unaffected.
                Ok(self
                    .statuses
                    .read()
                    .await
                    .get(server_id)
                    .cloned()
                    .unwrap_or_else(|| {
                        tracing::debug!(
                            server_id = %server_id,
                            error = %err,
                            "MCP enable failed without a status entry"
                        );
                        let mut failed = empty_status(server_id.clone(), McpStartupState::Failed);
                        failed.last_refreshed_at = Some(chrono::Utc::now());
                        failed
                    }))
            }
        }
    }

    async fn invoke_tool(
        &self,
        server_id: &McpServerId,
        tool_name: &str,
        input: Value,
    ) -> Result<Value, McpError> {
        let record = self.record(server_id).await?;
        if !record.enabled {
            return Err(McpError::McpServerUnavailable {
                server_id: server_id.clone(),
            });
        }
        if !tools_capability_allowed(&record) {
            return Err(McpError::McpToolInvocationFailed {
                server_id: server_id.clone(),
                tool_name: tool_name.to_string(),
                message: "MCP tools capability is not allowed for this server".to_string(),
            });
        }
        let client = self.ensure_client(&record).await?;
        let result = client
            .call_tool(
                tool_name.to_string(),
                Some(input),
                /*meta*/ None,
                Some(MCP_OPERATION_TIMEOUT),
            )
            .await
            .map_err(|err| McpError::McpToolInvocationFailed {
                server_id: server_id.clone(),
                tool_name: tool_name.to_string(),
                message: err.to_string(),
            })?;
        serde_json::to_value(result).map_err(|err| McpError::McpToolInvocationFailed {
            server_id: server_id.clone(),
            tool_name: tool_name.to_string(),
            message: err.to_string(),
        })
    }

    async fn read_resource(&self, server_id: &McpServerId, uri: &str) -> Result<Value, McpError> {
        Err(McpError::McpResourceReadFailed {
            server_id: server_id.clone(),
            uri: uri.to_string(),
            message: "MCP resource runtime is not wired yet".to_string(),
        })
    }
}

fn tools_capability_allowed(record: &McpServerRecord) -> bool {
    record.allowed_capabilities.is_empty()
        || record.allowed_capabilities.contains(&McpCapability::Tools)
}

fn mcp_tool_info_from_rmcp_tool(
    record: &McpServerRecord,
    tool: Tool,
    source_description: Option<String>,
    supports_parallel_tool_calls: bool,
) -> McpToolInfo {
    let raw_tool_name = tool.name.into_owned();
    let description = tool.description.map(std::borrow::Cow::into_owned);
    let read_only_hint = tool
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.read_only_hint)
        .unwrap_or(false);
    let meta = tool.meta.map(|meta| Value::Object(meta.0));
    let input_schema = Value::Object(Arc::unwrap_or_clone(tool.input_schema));
    let mut info = McpToolInfo::new(
        record.id.clone(),
        record.display_name.clone(),
        raw_tool_name,
        description,
        input_schema,
        read_only_hint,
        supports_parallel_tool_calls,
    );
    info.source_description = source_description;
    info.meta = meta;
    info
}

async fn create_client(
    record: &McpServerRecord,
    oauth_store_mode: OAuthCredentialsStoreMode,
) -> Result<RmcpClient, McpError> {
    let client = match &record.transport {
        McpTransportConfig::Stdio {
            command,
            cwd,
            env,
            env_vars,
        } => {
            let (program, args) = split_stdio_command(record, command)?;
            let env = env_map(env);
            RmcpClient::new_stdio_client(
                program,
                args,
                env,
                env_vars,
                cwd.clone(),
                Arc::new(LocalStdioServerLauncher::new(
                    std::env::current_dir().unwrap_or_else(|_| ".".into()),
                )),
            )
            .await
            .map_err(|err| McpError::McpStartupFailed {
                server_id: record.id.clone(),
                message: err.to_string(),
            })?
        }
        McpTransportConfig::StreamableHttp {
            url,
            auth,
            http_headers,
            env_http_headers,
        } => RmcpClient::new_streamable_http_client(
            &record.id.0,
            url,
            bearer_token(auth.as_ref()),
            non_empty_map(http_headers),
            non_empty_map(env_http_headers),
            oauth_store_mode,
        )
        .await
        .map_err(|err| McpError::McpStartupFailed {
            server_id: record.id.clone(),
            message: err.to_string(),
        })?,
        McpTransportConfig::Sse {
            url,
            auth,
            http_headers,
            env_http_headers,
        } => RmcpClient::new_sse_client(
            url,
            bearer_token(auth.as_ref()),
            non_empty_map(http_headers),
            non_empty_map(env_http_headers),
        )
        .await
        .map_err(|err| McpError::McpStartupFailed {
            server_id: record.id.clone(),
            message: err.to_string(),
        })?,
    };

    client
        .initialize(
            init_params(),
            Some(MCP_OPERATION_TIMEOUT),
            Box::new(|_, _| {
                Box::pin(async {
                    Ok(ElicitationResponse {
                        action: ElicitationAction::Accept,
                        content: Some(serde_json::json!({})),
                        meta: None,
                    })
                })
            }),
        )
        .await
        .map_err(|err| McpError::McpStartupFailed {
            server_id: record.id.clone(),
            message: err.to_string(),
        })?;
    Ok(client)
}

fn init_params() -> InitializeRequestParams {
    InitializeRequestParams {
        capabilities: ClientCapabilities::default(),
        client_info: Implementation {
            name: "devo".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            title: Some("Devo".into()),
            description: None,
            icons: None,
            website_url: None,
        },
        protocol_version: ProtocolVersion::V_2025_06_18,
        meta: None,
    }
}

fn split_stdio_command(
    record: &McpServerRecord,
    command: &[String],
) -> Result<(OsString, Vec<OsString>), McpError> {
    let Some((program, args)) = command.split_first() else {
        return Err(McpError::McpStartupFailed {
            server_id: record.id.clone(),
            message: "stdio MCP command must not be empty".to_string(),
        });
    };
    Ok((
        OsString::from(program),
        args.iter().map(OsString::from).collect(),
    ))
}

fn env_map(env: &BTreeMap<String, String>) -> Option<HashMap<OsString, OsString>> {
    if env.is_empty() {
        return None;
    }
    let mut mapped = HashMap::with_capacity(env.len());
    for (key, value) in env {
        mapped.insert(OsString::from(key), OsString::from(value));
    }
    Some(mapped)
}

fn non_empty_map(map: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    if map.is_empty() {
        return None;
    }
    let mut mapped = HashMap::with_capacity(map.len());
    for (key, value) in map {
        mapped.insert(key.clone(), value.clone());
    }
    Some(mapped)
}

fn bearer_token(auth: Option<&McpAuthConfig>) -> Option<String> {
    auth.map(|McpAuthConfig::BearerToken { token }| token.clone())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    fn record(
        id: &str,
        allowed_capabilities: Vec<McpCapability>,
        startup_policy: McpStartupPolicy,
    ) -> McpServerRecord {
        McpServerRecord {
            id: McpServerId(id.to_string()),
            display_name: id.to_string(),
            transport: McpTransportConfig::Stdio {
                command: Vec::new(),
                cwd: None,
                env: BTreeMap::new(),
                env_vars: Vec::new(),
            },
            startup_policy,
            enabled: true,
            trust_policy: Default::default(),
            allowed_capabilities,
            roots_policy: Default::default(),
            output_limits: Default::default(),
            auth_ref: None,
        }
    }

    fn manager_with(records: Vec<McpServerRecord>) -> RmcpMcpManager {
        RmcpMcpManager::new(
            McpConfig {
                servers: records,
                ..McpConfig::default()
            },
            OAuthCredentialsStoreMode::default(),
        )
    }

    #[tokio::test]
    async fn tool_discovery_respects_allowed_capabilities() {
        let mut disabled = record(
            "disabled",
            vec![McpCapability::Tools],
            McpStartupPolicy::Eager,
        );
        disabled.enabled = false;
        let manager = manager_with(vec![
            record("implicit", Vec::new(), McpStartupPolicy::Eager),
            record(
                "resources",
                vec![McpCapability::Resources],
                McpStartupPolicy::Eager,
            ),
            record("tools", vec![McpCapability::Tools], McpStartupPolicy::Eager),
            record(
                "manual",
                vec![McpCapability::Tools],
                McpStartupPolicy::Manual,
            ),
            disabled,
        ]);

        let server_ids = manager
            .discoverable_tool_records()
            .await
            .into_iter()
            .map(|record| record.id.0)
            .collect::<Vec<_>>();

        assert_eq!(
            server_ids,
            vec!["implicit".to_string(), "tools".to_string()]
        );
    }

    #[tokio::test]
    async fn invoke_tool_rejects_disallowed_tools_before_starting_client() {
        let manager = manager_with(vec![record(
            "resources",
            vec![McpCapability::Resources],
            McpStartupPolicy::Eager,
        )]);

        let error = manager
            .invoke_tool(&McpServerId("resources".to_string()), "search", json!({}))
            .await
            .expect_err("tools capability should be rejected");

        assert_eq!(
            error,
            McpError::McpToolInvocationFailed {
                server_id: McpServerId("resources".to_string()),
                tool_name: "search".to_string(),
                message: "MCP tools capability is not allowed for this server".to_string(),
            }
        );
        assert!(manager.clients.read().await.is_empty());
    }

    #[tokio::test]
    async fn invoke_tool_rejects_disabled_server_before_starting_client() {
        let mut disabled = record(
            "disabled",
            vec![McpCapability::Tools],
            McpStartupPolicy::Eager,
        );
        disabled.enabled = false;
        let manager = manager_with(vec![disabled]);

        let error = manager
            .invoke_tool(&McpServerId("disabled".to_string()), "search", json!({}))
            .await
            .expect_err("disabled server should be rejected");

        assert_eq!(
            error,
            McpError::McpServerUnavailable {
                server_id: McpServerId("disabled".to_string()),
            }
        );
        assert!(manager.clients.read().await.is_empty());
    }

    #[tokio::test]
    async fn set_enabled_flips_config_and_marks_disabled_without_touching_siblings() {
        let mut sibling = record(
            "sibling",
            vec![McpCapability::Tools],
            McpStartupPolicy::Eager,
        );
        sibling.enabled = true;
        let mut target = record(
            "target",
            vec![McpCapability::Tools],
            McpStartupPolicy::Eager,
        );
        target.enabled = true;
        let manager = manager_with(vec![sibling, target]);
        manager.statuses.write().await.insert(
            McpServerId("sibling".to_string()),
            empty_status(McpServerId("sibling".to_string()), McpStartupState::Ready),
        );

        let status = manager
            .set_enabled(&McpServerId("target".to_string()), /*enabled*/ false)
            .await
            .expect("disable should succeed");

        assert_eq!(status.startup_state, McpStartupState::Disabled);
        assert!(
            !manager
                .config
                .read()
                .await
                .servers
                .iter()
                .find(|record| record.id.0 == "target")
                .expect("target")
                .enabled
        );
        assert!(
            manager
                .config
                .read()
                .await
                .servers
                .iter()
                .find(|record| record.id.0 == "sibling")
                .expect("sibling")
                .enabled
        );
        assert_eq!(
            manager
                .statuses
                .read()
                .await
                .get(&McpServerId("sibling".to_string()))
                .expect("sibling status")
                .startup_state,
            McpStartupState::Ready
        );
        assert!(
            !manager
                .clients
                .read()
                .await
                .contains_key(&McpServerId("target".to_string()))
        );
    }

    #[tokio::test]
    async fn set_enabled_true_with_bad_binary_persists_failed_status() {
        let mut bad = record("bad", vec![McpCapability::Tools], McpStartupPolicy::Eager);
        bad.enabled = false;
        bad.transport = McpTransportConfig::Stdio {
            command: vec!["__devo_missing_mcp_binary__".to_string()],
            cwd: None,
            env: BTreeMap::new(),
            env_vars: Vec::new(),
        };
        let mut sibling = record(
            "sibling",
            vec![McpCapability::Tools],
            McpStartupPolicy::Eager,
        );
        sibling.enabled = false;
        let manager = manager_with(vec![sibling, bad]);

        let status = manager
            .set_enabled(&McpServerId("bad".to_string()), /*enabled*/ true)
            .await
            .expect("enable should return status even on startup failure");

        assert_eq!(status.startup_state, McpStartupState::Failed);
        assert!(
            manager
                .config
                .read()
                .await
                .servers
                .iter()
                .find(|record| record.id.0 == "bad")
                .expect("bad")
                .enabled
        );
        assert!(
            !manager
                .config
                .read()
                .await
                .servers
                .iter()
                .find(|record| record.id.0 == "sibling")
                .expect("sibling")
                .enabled
        );
        assert!(manager.clients.read().await.is_empty());
    }
}
