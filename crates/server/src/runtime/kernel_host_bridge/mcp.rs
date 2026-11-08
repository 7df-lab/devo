//! MCP operations use the session manager; credentials never enter host replies.

use devo_core::McpStartupState;
use serde_json::{Value, json};

use super::{
    HostBridge, ResourceKind, check_or_error, deny_plan_mutation, error_reply, permission_request,
    plan_mode_denies_mutation, synthetic_tool_call_id,
};

/// Return observed connection state without starting servers or exposing config.
pub(crate) async fn handle_mcp_list_servers(bridge: &HostBridge) -> Value {
    let Some(manager) = bridge.mcp_manager.as_ref() else {
        return error_reply("mcp manager not available on this host bridge");
    };
    let mut statuses = match manager.statuses().await {
        Ok(statuses) => statuses,
        Err(error) => return error_reply(error.to_string()),
    };
    statuses.sort_by(|left, right| left.server_id.0.cmp(&right.server_id.0));
    let servers: Vec<Value> = statuses
        .into_iter()
        .map(|status| {
            json!({
                "server": status.server_id.0,
                "status": status.startup_state,
                "authStatus": status.auth_state,
                "toolCount": status.tools.len(),
            })
        })
        .collect();
    json!({ "status": "ok", "result": { "servers": servers } })
}

pub(crate) async fn handle_mcp_call(bridge: &HostBridge, params: &Value) -> Value {
    if plan_mode_denies_mutation(bridge) {
        return deny_plan_mutation("mcp.call");
    }
    let Some(manager) = bridge.mcp_manager.as_ref() else {
        return error_reply("mcp manager not available on this host bridge");
    };
    let server = params
        .get("server")
        .or_else(|| params.get("serverId"))
        .or_else(|| params.get("server_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool_name = params
        .get("tool")
        .or_else(|| params.get("toolName"))
        .or_else(|| params.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if server.is_empty() || tool_name.is_empty() {
        return error_reply("mcp.call requires server and tool");
    }
    let input = params
        .get("arguments")
        .or_else(|| params.get("input"))
        .or_else(|| params.get("args"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let tool_call_id = synthetic_tool_call_id("mcp");
    let req = permission_request(
        bridge,
        tool_call_id,
        "mcp.call",
        json!({
            "server": server,
            "tool": tool_name,
            "arguments": input,
        }),
        ResourceKind::Custom("mcp.call".into()),
        format!("Call MCP tool {server}/{tool_name}"),
        None,
        Some(format!("{server}/{tool_name}")),
    );
    if let Err(reply) = check_or_error(bridge, req).await {
        return reply;
    }
    let server_id = devo_core::McpServerId(server.to_string());
    match manager.invoke_tool(&server_id, tool_name, input).await {
        Ok(result) => json!({
            "status": "ok",
            "result": result,
        }),
        Err(err) => error_reply(err.to_string()),
    }
}

pub(crate) async fn handle_mcp_list_tools(bridge: &HostBridge, params: &Value) -> Value {
    let Some(server) = params
        .get("server")
        .and_then(Value::as_str)
        .filter(|server| !server.is_empty())
    else {
        return error_reply("mcp.list_tools requires server");
    };
    // Listing is read-only; skip Ask. Refreshing one configured server lists its
    // tools once and avoids discovering every server for this per-server API.
    let Some(manager) = bridge.mcp_manager.as_ref() else {
        return error_reply("mcp manager not available on this host bridge");
    };
    let server_id = devo_core::McpServerId(server.to_string());
    match manager.refresh(&server_id).await {
        Ok(status) => {
            // A successful manager call is not necessarily a usable connection.
            // Disabled/auth-required/failed servers must not look like an empty
            // but connected server. Ready with zero tools remains valid.
            if status.server_id != server_id || status.startup_state != McpStartupState::Ready {
                return error_reply(format!(
                    "MCP server '{server}' is not ready; inspect mcp.list_servers before retrying"
                ));
            }
            let tools: Vec<Value> = status
                .tools
                .into_iter()
                .filter(|tool| tool.server_id == server_id)
                .map(|tool| {
                    json!({
                        "server": tool.server_id.0,
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": tool.input_schema,
                    })
                })
                .collect();
            json!({ "status": "ok", "result": { "tools": tools } })
        }
        Err(err) => error_reply(err.to_string()),
    }
}
