//! Explicit reconnection of configured Native transports, never authored OAuth login.

use std::sync::Arc;

use devo_core::{McpManager, McpServerId, McpStartupState, McpToolInfo};
use devo_protocol::SuccessResponse;
use devo_protocol::native::rpc_admin::{McpReconnectParams, McpReconnectResult};

use super::ServerRuntime;
use crate::ProtocolErrorCode;

struct ReconnectReservation<'a> {
    runtime: &'a ServerRuntime,
    name: String,
}

impl Drop for ReconnectReservation<'_> {
    fn drop(&mut self) {
        self.runtime
            .mcp_reconnecting
            .lock()
            .expect("MCP reconnect reservation mutex")
            .remove(&self.name);
    }
}

impl ServerRuntime {
    pub(super) async fn handle_mcp_reconnect(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<McpReconnectParams>(params) {
            Ok(params) if !params.name.trim().is_empty() => params,
            _ => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "mcp/reconnect accepts only a non-empty server name, not credentials"
                        .to_string(),
                );
            }
        };
        let name = params.name.trim().to_string();
        let server_id = McpServerId(name.clone());
        let manager = &self.deps.process_context.mcp_manager;
        let _reservation = {
            let mut reconnecting = self
                .mcp_reconnecting
                .lock()
                .expect("MCP reconnect reservation mutex");
            if !reconnecting.insert(name.clone()) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "MCP reconnect is already in progress; wait for it to finish".to_string(),
                );
            }
            ReconnectReservation {
                runtime: self,
                name,
            }
        };
        let mut retained_contexts = Vec::new();
        {
            let _publication = self.mcp_publication_gate.lock().await;
            let status = manager.statuses().await.ok().and_then(|statuses| {
                statuses
                    .into_iter()
                    .find(|status| status.server_id == server_id)
            });
            match status.map(|status| status.startup_state) {
                None | Some(McpStartupState::Disabled) => return self.error_response(
                    request_id, ProtocolErrorCode::InvalidParams,
                    "MCP server is unknown or disabled; enable an existing server before reconnecting".to_string()),
                Some(McpStartupState::Starting) => return self.error_response(
                    request_id, ProtocolErrorCode::InvalidParams,
                    "MCP server is already connecting; wait for it to finish".to_string()),
                Some(McpStartupState::NotStarted | McpStartupState::Ready | McpStartupState::Failed
                    | McpStartupState::AuthRequired | McpStartupState::Degraded | McpStartupState::Stopped) => {}
            }
            retained_contexts.extend(self.deps.take_workspace_contexts());
            // Stop advertising old schemas before starting transport I/O. In-flight
            // calls are not replayed; subsequent turns consume the new registry.
            self.publish_mcp_server_tools(manager, &server_id, Vec::new(), &retained_contexts)
                .await;
        }
        let outcome = manager.reconnect(&server_id).await;
        {
            let _publication = self.mcp_publication_gate.lock().await;
            // Read the CURRENT cache, not the successful operation's old snapshot.
            // A disable or newer manager generation may have won while we waited.
            let tools = manager.cached_tools(&server_id).await.unwrap_or_default();
            self.publish_mcp_server_tools(manager, &server_id, tools, &retained_contexts)
                .await;
        }
        if outcome.is_err() {
            // Transport/provider errors can include authorization headers or bodies.
            // Observed state remains available via mcp/list without those details.
            return self.error_response(request_id, ProtocolErrorCode::InternalError,
                "MCP reconnect did not complete. Check the server configuration and its owning login, then retry. No tool call was replayed.".to_string());
        }
        match self.mcp_server_infos().await {
            Ok(servers) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: McpReconnectResult { servers },
            })
            .expect("serialize mcp/reconnect response"),
            Err(_) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                "MCP reconnect completed but its current status could not be read".to_string(),
            ),
        }
    }

    /// Caller holds the publication gate. No network discovery occurs here.
    /// Replace only this manager's selected server slice. Workspace overrides,
    /// non-MCP tools, allowlists, and explicit actor registries remain intact.
    pub(super) async fn publish_mcp_server_tools(
        &self,
        manager: &Arc<dyn McpManager>,
        server_id: &McpServerId,
        tools: Vec<McpToolInfo>,
        additional_contexts: &[Arc<crate::session_context::SessionRuntimeContext>],
    ) {
        let handles = self.list_session_handles().await;
        let mut contexts = self.deps.take_workspace_contexts();
        contexts.extend(additional_contexts.iter().cloned());
        contexts.push(Arc::clone(&self.deps.process_context));
        for handle in &handles {
            if let Some(context) = handle.runtime_context().await {
                contexts.push(context);
            }
        }
        {
            let mut updated_registries = std::collections::HashSet::new();
            for context in contexts {
                if !Arc::ptr_eq(manager, &context.mcp_manager)
                    || !updated_registries.insert(Arc::as_ptr(&context.registry))
                {
                    continue;
                }
                let registry = context.tool_registry();
                context.replace_tool_registry(Arc::new(registry.replace_mcp_server_tools(
                    server_id,
                    tools.clone(),
                    Arc::clone(manager),
                )));
            }
        }
        for handle in handles {
            let _ = handle
                .replace_mcp_server_tools(server_id.clone(), tools.clone(), Arc::clone(manager))
                .await;
        }
    }
}

#[cfg(test)]
mod tests;
