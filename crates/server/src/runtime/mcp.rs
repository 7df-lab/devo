//! MCP admin RPC handlers (`mcp/list`, `mcp/tools`, `mcp/set_enabled`).

use std::sync::Arc;

use devo_core::McpServerId;
use devo_core::McpServerStatus;
use devo_core::McpStartupState;
use devo_protocol::SuccessResponse;
use devo_protocol::native::rpc_admin::McpListParams;
use devo_protocol::native::rpc_admin::McpListResult;
use devo_protocol::native::rpc_admin::McpServerInfo;
use devo_protocol::native::rpc_admin::McpSetEnabledParams;
use devo_protocol::native::rpc_admin::McpSetEnabledResult;
use devo_protocol::native::rpc_admin::McpToolEntry;
use devo_protocol::native::rpc_admin::McpToolsParams;
use devo_protocol::native::rpc_admin::McpToolsResult;

use super::ServerRuntime;
use crate::ProtocolErrorCode;

impl ServerRuntime {
    pub(super) async fn handle_mcp_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        if let Err(error) = serde_json::from_value::<McpListParams>(params) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("invalid mcp/list params: {error}"),
            );
        }

        match self.mcp_server_infos().await {
            Ok(servers) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: McpListResult { servers },
            })
            .expect("serialize mcp/list response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to list mcp servers: {error}"),
            ),
        }
    }

    pub(super) async fn handle_mcp_tools(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<McpToolsParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid mcp/tools params: {error}"),
                );
            }
        };

        let name = params.name.trim();
        if name.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "mcp/tools requires a non-empty name".to_string(),
            );
        }
        let server_id = McpServerId(name.to_string());
        let manager = &self.deps.process_context.mcp_manager;

        let statuses = match manager.statuses().await {
            Ok(statuses) => statuses,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to read mcp statuses: {error}"),
                );
            }
        };
        let existing = statuses
            .iter()
            .find(|status| status.server_id == server_id)
            .cloned();

        let status = if mcp_tools_need_refresh(existing.as_ref()) {
            match manager.refresh(&server_id).await {
                Ok(status) => status,
                Err(error) => {
                    tracing::warn!(server = %server_id, error = %error, "mcp/tools refresh failed");
                    match existing {
                        Some(status) => status,
                        None => {
                            return self.error_response(
                                request_id,
                                ProtocolErrorCode::InvalidParams,
                                format!("mcp server `{name}` not found"),
                            );
                        }
                    }
                }
            }
        } else {
            match existing {
                Some(status) => status,
                None => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("mcp server `{name}` not found"),
                    );
                }
            }
        };

        let mut tools = status
            .tools
            .into_iter()
            .map(|tool| McpToolEntry {
                name: tool.name,
                description: tool.description,
            })
            .collect::<Vec<_>>();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: McpToolsResult { tools },
        })
        .expect("serialize mcp/tools response")
    }

    pub(super) async fn handle_mcp_set_enabled(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<McpSetEnabledParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid mcp/set_enabled params: {error}"),
                );
            }
        };

        let name = params.name.trim();
        if name.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "mcp/set_enabled requires a non-empty name".to_string(),
            );
        }

        let config_file = {
            let store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            store
                .user_config_dir()
                .join("config.toml")
                .display()
                .to_string()
        };
        if let Some(reason) = self
            .config_change_hook_block_reason("mcp", Some(config_file))
            .await
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::PolicyDenied,
                format!("mcp config change blocked by hook: {reason}"),
            );
        }

        let mut contexts = Vec::new();
        for handle in self.list_session_handles().await {
            if let Some(context) = handle.runtime_context().await {
                contexts.push(context);
            }
        }
        let server_id = McpServerId(name.to_string());
        let mut managers = vec![Arc::clone(&self.deps.process_context.mcp_manager)];
        {
            let _publication = self.mcp_publication_gate.lock().await;
            let current_record = {
                let mut store = self
                    .deps
                    .config_store
                    .lock()
                    .expect("app config store mutex");
                if let Err(error) = store.set_mcp_server_enabled(name, params.enabled) {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to update mcp config: {error}"),
                    );
                }
                store
                    .effective_config()
                    .mcp_runtime
                    .servers
                    .iter()
                    .find(|record| record.id == server_id)
                    .cloned()
            };
            contexts.extend(self.deps.take_workspace_contexts());
            // Re-read effective workspace configuration after the global write.
            // This follows inherited records through repeated toggles while
            // preserving explicit workspace transport/enablement overrides.
            for context in &contexts {
                let inherits_record = {
                    let mut store = context.config_store.lock().expect("app config store mutex");
                    store.reload().is_ok()
                        && store
                            .workspace_mcp_server_ids()
                            .is_ok_and(|ids| !ids.contains(&server_id))
                        && store
                            .effective_config()
                            .mcp_runtime
                            .servers
                            .iter()
                            .find(|record| record.id == server_id)
                            == current_record.as_ref()
                };
                if inherits_record
                    && !managers
                        .iter()
                        .any(|manager| Arc::ptr_eq(manager, &context.mcp_manager))
                {
                    managers.push(Arc::clone(&context.mcp_manager));
                }
            }
            // Configure every affected manager before any startup I/O. A later
            // disable therefore invalidates ALL older enable operations, including
            // managers whose discovery has not started yet.
            for manager in &managers {
                if manager
                    .configure_enabled(&server_id, params.enabled)
                    .await
                    .is_err()
                {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        "failed to apply MCP enablement; check the server configuration"
                            .to_string(),
                    );
                }
                let tools = manager.cached_tools(&server_id).await.unwrap_or_default();
                self.publish_mcp_server_tools(manager, &server_id, tools, &contexts)
                    .await;
            }
        }
        if params.enabled {
            for manager in &managers {
                // Refresh never changes desired enablement. Disable can run while
                // this I/O waits, and a disabled manager rejects late discovery.
                let _ = manager.refresh(&server_id).await;
                let _publication = self.mcp_publication_gate.lock().await;
                let tools = manager.cached_tools(&server_id).await.unwrap_or_default();
                self.publish_mcp_server_tools(manager, &server_id, tools, &contexts)
                    .await;
            }
        }

        match self.mcp_server_infos().await {
            Ok(servers) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: McpSetEnabledResult { servers },
            })
            .expect("serialize mcp/set_enabled response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("mcp enablement applied but list failed: {error}"),
            ),
        }
    }

    pub(super) async fn mcp_server_infos(&self) -> Result<Vec<McpServerInfo>, String> {
        let statuses = self
            .deps
            .process_context
            .mcp_manager
            .statuses()
            .await
            .map_err(|error| error.to_string())?;
        let mut servers = statuses
            .into_iter()
            .map(|status| McpServerInfo {
                name: status.server_id.0,
                status: startup_state_label(&status.startup_state).to_string(),
                tool_count: status.tools.len() as u32,
            })
            .collect::<Vec<_>>();
        servers.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(servers)
    }
}

fn startup_state_label(state: &McpStartupState) -> &'static str {
    match state {
        McpStartupState::Disabled => "disabled",
        McpStartupState::NotStarted => "not_started",
        McpStartupState::Starting => "starting",
        McpStartupState::Ready => "ready",
        McpStartupState::Failed => "failed",
        McpStartupState::AuthRequired => "auth_required",
        McpStartupState::Degraded => "degraded",
        McpStartupState::Stopped => "stopped",
    }
}

fn mcp_tools_need_refresh(status: Option<&McpServerStatus>) -> bool {
    let Some(status) = status else {
        return true;
    };
    match status.startup_state {
        McpStartupState::Disabled => false,
        McpStartupState::NotStarted | McpStartupState::Stopped | McpStartupState::Failed => true,
        McpStartupState::Starting
        | McpStartupState::Ready
        | McpStartupState::AuthRequired
        | McpStartupState::Degraded => status.tools.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use devo_core::McpAuthState;
    use devo_core::McpToolDescriptor;

    fn status_with(startup_state: McpStartupState, tool_count: usize) -> McpServerStatus {
        McpServerStatus {
            server_id: McpServerId("docs".to_string()),
            startup_state,
            auth_state: McpAuthState::NotRequired,
            tools: (0..tool_count)
                .map(|index| McpToolDescriptor {
                    server_id: McpServerId("docs".to_string()),
                    name: format!("tool_{index}"),
                    description: String::new(),
                    input_schema: serde_json::json!({}),
                })
                .collect(),
            resources: Vec::new(),
            resource_templates: Vec::new(),
            last_refreshed_at: None,
        }
    }

    /// Verifies: mcp/tools does not start a disabled server and does start idle enabled ones.
    #[test]
    fn mcp_tools_refresh_skips_disabled_and_filled_ready_servers() {
        assert_eq!(
            [
                mcp_tools_need_refresh(None),
                mcp_tools_need_refresh(Some(&status_with(McpStartupState::Disabled, 0))),
                mcp_tools_need_refresh(Some(&status_with(McpStartupState::NotStarted, 0))),
                mcp_tools_need_refresh(Some(&status_with(McpStartupState::Ready, 0))),
                mcp_tools_need_refresh(Some(&status_with(McpStartupState::Ready, 2))),
                mcp_tools_need_refresh(Some(&status_with(McpStartupState::Failed, 0))),
            ],
            [true, false, true, true, false, true]
        );
    }
}
