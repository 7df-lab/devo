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

use devo_core::mcp::McpAuthConfig;
use devo_core::mcp::McpAuthState;
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
    config: McpConfig,
    oauth_store_mode: OAuthCredentialsStoreMode,
    clients: RwLock<HashMap<McpServerId, Arc<RmcpClient>>>,
    statuses: RwLock<HashMap<McpServerId, McpServerStatus>>,
}

impl RmcpMcpManager {
    pub fn new(config: McpConfig, oauth_store_mode: OAuthCredentialsStoreMode) -> Self {
        let statuses = config
            .servers
            .iter()
            .map(|record| {
                let startup_state = if record.enabled {
                    McpStartupState::NotStarted
                } else {
                    McpStartupState::Disabled
                };
                (
                    record.id.clone(),
                    McpServerStatus {
                        server_id: record.id.clone(),
                        startup_state,
                        auth_state: McpAuthState::NotRequired,
                        tools: Vec::new(),
                        resources: Vec::new(),
                        resource_templates: Vec::new(),
                        last_refreshed_at: None,
                    },
                )
            })
            .collect();

        Self {
            config,
            oauth_store_mode,
            clients: RwLock::new(HashMap::new()),
            statuses: RwLock::new(statuses),
        }
    }

    async fn ensure_client(&self, record: &McpServerRecord) -> Result<Arc<RmcpClient>, McpError> {
        if let Some(client) = self.clients.read().await.get(&record.id).cloned() {
            return Ok(client);
        }

        let client = Arc::new(create_client(record, self.oauth_store_mode).await?);
        self.clients
            .write()
            .await
            .insert(record.id.clone(), Arc::clone(&client));
        Ok(client)
    }

    fn record(&self, server_id: &McpServerId) -> Result<&McpServerRecord, McpError> {
        self.config
            .servers
            .iter()
            .find(|record| &record.id == server_id)
            .ok_or_else(|| McpError::McpServerUnavailable {
                server_id: server_id.clone(),
            })
    }

    fn discoverable_records(&self) -> impl Iterator<Item = &McpServerRecord> {
        self.config.servers.iter().filter(|record| {
            record.enabled
                && self.config.auto_start
                && !matches!(record.startup_policy, McpStartupPolicy::Manual)
        })
    }
}

#[async_trait]
impl McpManager for RmcpMcpManager {
    async fn statuses(&self) -> Result<Vec<McpServerStatus>, McpError> {
        Ok(self.statuses.read().await.values().cloned().collect())
    }

    async fn discover_tools(&self) -> Result<Vec<McpToolInfo>, McpError> {
        let mut tools = Vec::new();
        for record in self.discoverable_records() {
            match self.refresh(&record.id).await {
                Ok(status) => {
                    let Some(client) = self.clients.read().await.get(&record.id).cloned() else {
                        continue;
                    };
                    match client
                        .list_tools_with_connector_ids(None, Some(MCP_OPERATION_TIMEOUT))
                        .await
                    {
                        Ok(list) => {
                            for tool in list.tools {
                                tools.push(mcp_tool_info_from_rmcp_tool(
                                    record,
                                    tool.tool,
                                    tool.connector_description,
                                    /*supports_parallel_tool_calls*/ false,
                                ));
                            }
                        }
                        Err(err) => {
                            warn!(
                                server_id = %record.id,
                                error = %err,
                                "failed to list MCP tools"
                            );
                        }
                    }
                    self.statuses
                        .write()
                        .await
                        .insert(record.id.clone(), status);
                }
                Err(err) => {
                    warn!(
                        server_id = %record.id,
                        error = %err,
                        "failed to refresh MCP server"
                    );
                }
            }
        }
        Ok(tools)
    }

    async fn refresh(&self, server_id: &McpServerId) -> Result<McpServerStatus, McpError> {
        let record = self.record(server_id)?;
        if !record.enabled {
            return Err(McpError::McpServerUnavailable {
                server_id: server_id.clone(),
            });
        }

        self.statuses.write().await.insert(
            server_id.clone(),
            McpServerStatus {
                server_id: server_id.clone(),
                startup_state: McpStartupState::Starting,
                auth_state: McpAuthState::NotRequired,
                tools: Vec::new(),
                resources: Vec::new(),
                resource_templates: Vec::new(),
                last_refreshed_at: None,
            },
        );

        let client = self.ensure_client(record).await?;
        let list = client
            .list_tools(None, Some(MCP_OPERATION_TIMEOUT))
            .await
            .map_err(|err| McpError::McpProtocolError {
                server_id: server_id.clone(),
                message: err.to_string(),
            })?;

        let status = McpServerStatus {
            server_id: server_id.clone(),
            startup_state: McpStartupState::Ready,
            auth_state: McpAuthState::Authenticated,
            tools: list
                .tools
                .into_iter()
                .map(|tool| McpToolDescriptor {
                    server_id: server_id.clone(),
                    name: tool.name.to_string(),
                    description: tool
                        .description
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                    input_schema: Value::Object((*tool.input_schema).clone()),
                })
                .collect(),
            resources: Vec::new(),
            resource_templates: Vec::new(),
            last_refreshed_at: Some(chrono::Utc::now()),
        };
        self.statuses
            .write()
            .await
            .insert(server_id.clone(), status.clone());
        Ok(status)
    }

    async fn invoke_tool(
        &self,
        server_id: &McpServerId,
        tool_name: &str,
        input: Value,
    ) -> Result<Value, McpError> {
        let record = self.record(server_id)?;
        let client = self.ensure_client(record).await?;
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

fn mcp_tool_info_from_rmcp_tool(
    record: &McpServerRecord,
    tool: Tool,
    source_description: Option<String>,
    supports_parallel_tool_calls: bool,
) -> McpToolInfo {
    let raw_tool_name = tool.name.to_string();
    let description = tool.description.as_ref().map(ToString::to_string);
    let input_schema = Value::Object((*tool.input_schema).clone());
    let read_only_hint = tool
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.read_only_hint)
        .unwrap_or(false);
    let meta = tool.meta.map(|meta| Value::Object(meta.0));
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
    Some(
        env.iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect(),
    )
}

fn non_empty_map(map: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    (!map.is_empty()).then(|| {
        map.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    })
}

fn bearer_token(auth: Option<&McpAuthConfig>) -> Option<String> {
    auth.map(|McpAuthConfig::BearerToken { token }| token.clone())
}
