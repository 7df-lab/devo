//! Route the session's selected search tool through the normal MCP manager.

use std::sync::Arc;

use devo_core::{
    McpManager, McpServerId, McpServerStatus, McpStartupState, ResolvedWebFetchConfig,
    ResolvedWebSearchConfig,
};
use devo_protocol::native::rpc_session::WebSearchPickerEntry;
use devo_protocol::native::session::WebSearchChoice;
use serde_json::Value;

use super::web_search_builtin as builtin;
use crate::session_context::SessionRuntimeContext;

/// Captures the search and fetch routes for one turn. A built-in supplier
/// selected for both capabilities can share one lazy stdio connection.
#[derive(Default)]
pub(crate) struct WebRoutes {
    pub(super) search: Option<McpWebSearchRoute>,
    pub(super) fetch: WebFetchRoute,
}

#[derive(Default)]
pub(super) enum WebFetchRoute {
    #[default]
    Direct,
    Unavailable(&'static str),
    Mcp {
        server_id: McpServerId,
        manager: Result<Arc<dyn McpManager>, String>,
    },
}

impl WebRoutes {
    pub(super) fn for_turn(
        context: &SessionRuntimeContext,
        search: &ResolvedWebSearchConfig,
        fetch: ResolvedWebFetchConfig,
    ) -> Self {
        let search = McpWebSearchRoute::for_turn(context, search);
        let fetch = match fetch {
            ResolvedWebFetchConfig::Mcp { provider } => {
                let server_id = McpServerId(builtin::server_id(provider).into());
                let manager = search
                    .as_ref()
                    .filter(|route| route.server_id == server_id)
                    .map_or_else(
                        || builtin::manager(context, provider).map_err(|error| error.to_string()),
                        |route| route.manager.clone(),
                    );
                WebFetchRoute::Mcp { server_id, manager }
            }
            ResolvedWebFetchConfig::Disabled => {
                WebFetchRoute::Unavailable("Web fetch is off for this turn.")
            }
            ResolvedWebFetchConfig::Provider => WebFetchRoute::Unavailable(
                "This turn uses model-hosted web fetch, not the Python helper.",
            ),
            ResolvedWebFetchConfig::Local => WebFetchRoute::Direct,
        };
        Self { search, fetch }
    }
}

pub(crate) struct McpWebSearchRoute {
    pub server_id: McpServerId,
    pub tool_name: String,
    pub manager: Result<Arc<dyn McpManager>, String>,
}

impl McpWebSearchRoute {
    pub(super) fn for_turn(
        context: &SessionRuntimeContext,
        config: &ResolvedWebSearchConfig,
    ) -> Option<Self> {
        let ResolvedWebSearchConfig::Mcp {
            server_id,
            tool_name,
        } = config
        else {
            return None;
        };
        let manager = if let Some(supplier) = builtin::supplier_for_server_id(server_id) {
            if tool_name == builtin::SEARCH_TOOL {
                builtin::manager(context, supplier).map_err(|error| error.to_string())
            } else {
                Err("The built-in search tool is no longer available. Change the selection in /web.".into())
            }
        } else {
            Ok(Arc::clone(&context.mcp_manager))
        };
        Some(Self {
            server_id: McpServerId(server_id.clone()),
            tool_name: tool_name.clone(),
            manager,
        })
    }

    pub(super) fn is_builtin(&self) -> bool {
        builtin::supplier_for_server_id(&self.server_id.0).is_some()
    }
}

/// V1 supports a simple query-only call. Unsupported structural combinators
/// require a future explicit field mapping rather than guessed tool arguments.
pub(super) fn supports_query_input(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("object")
        && schema
            .pointer("/properties/query/type")
            .and_then(Value::as_str)
            == Some("string")
        && schema.get("required").is_none_or(|required| {
            required
                .as_array()
                .is_some_and(|fields| fields.iter().all(|name| name == "query"))
        })
        && [
            "$ref",
            "allOf",
            "anyOf",
            "oneOf",
            "if",
            "not",
            "dependentRequired",
            "dependencies",
            "dependentSchemas",
            "patternProperties",
            "propertyNames",
        ]
        .iter()
        .all(|key| schema.get(*key).is_none())
        && schema
            .get("minProperties")
            .is_none_or(|v| v.as_u64().is_some_and(|n| n <= 1))
        && schema
            .get("maxProperties")
            .is_none_or(|v| v.as_u64().is_some_and(|n| n >= 1))
}

pub(super) fn selected_tool_note(
    server: Option<&McpServerStatus>,
    tool_name: &str,
) -> Option<&'static str> {
    match server {
        None => Some("MCP server not configured"),
        Some(server) => match server.startup_state {
            McpStartupState::Ready => match server
                .tools
                .iter()
                .find(|tool| tool.name == tool_name && tool.server_id == server.server_id)
            {
                Some(tool) if supports_query_input(&tool.input_schema) => None,
                Some(_) => Some("Tool no longer accepts a query on its own"),
                None => Some("Search tool not found on this server"),
            },
            McpStartupState::Disabled => Some("MCP server is disabled"),
            McpStartupState::AuthRequired => Some("MCP server needs authentication"),
            _ => Some("MCP server is not connected"),
        },
    }
}

pub(super) async fn invoke(route: &McpWebSearchRoute, query: &str) -> Result<Value, String> {
    let manager = route.manager.as_ref().map_err(Clone::clone)?;
    let status = if route.is_builtin() {
        Some(manager.refresh(&route.server_id).await.map_err(|_| {
            "Could not start the built-in web MCP. Check /web and the supplier configuration."
                .to_string()
        })?)
    } else {
        manager.statuses().await.ok().and_then(|statuses| {
            statuses
                .into_iter()
                .find(|status| status.server_id == route.server_id)
        })
    };
    if let Some(note) = selected_tool_note(status.as_ref(), &route.tool_name) {
        return Err(format!("{note}. Check /web or your MCP configuration."));
    }
    let result = manager
        .invoke_tool(
            &route.server_id,
            &route.tool_name,
            serde_json::json!({"query": query}),
        )
        .await
        .map_err(|_| {
            "The selected MCP search failed. Check /web or your MCP configuration.".to_string()
        })?;
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err("The selected MCP tool returned a search error.".into());
    }
    Ok(result)
}

pub(super) fn picker_entries(
    context: &SessionRuntimeContext,
    statuses: &[McpServerStatus],
    selected: &WebSearchChoice,
    hosted_note: Option<String>,
) -> Vec<WebSearchPickerEntry> {
    let mut entries = vec![
        WebSearchPickerEntry {
            label: "Follow defaults".into(),
            choice: WebSearchChoice::Inherit,
            note: None,
        },
        WebSearchPickerEntry {
            label: "Model provider".into(),
            choice: WebSearchChoice::Provider,
            note: hosted_note,
        },
    ];
    entries.extend(builtin::SUPPLIERS.into_iter().map(|supplier| {
        mcp_picker_entry(
            context,
            statuses,
            builtin::server_id(supplier),
            builtin::SEARCH_TOOL,
        )
    }));
    let mut discovered: Vec<_> = statuses
        .iter()
        .filter(|status| {
            builtin::supplier_for_server_id(&status.server_id.0).is_none()
                && status.startup_state == McpStartupState::Ready
        })
        .flat_map(|status| {
            status.tools.iter().filter(move |tool| {
                tool.server_id == status.server_id && supports_query_input(&tool.input_schema)
            })
        })
        .map(|tool| mcp_picker_entry(context, statuses, &tool.server_id.0, &tool.name))
        .collect();
    discovered.sort_by(|left, right| left.label.cmp(&right.label));
    discovered.dedup_by(|left, right| left.choice == right.choice);
    entries.extend(discovered);
    if let WebSearchChoice::Mcp {
        server_id,
        tool_name,
    } = selected
        && !entries.iter().any(|entry| &entry.choice == selected)
    {
        entries.push(mcp_picker_entry(context, statuses, server_id, tool_name));
    }
    entries.push(WebSearchPickerEntry {
        label: "Off".into(),
        choice: WebSearchChoice::Off,
        note: None,
    });
    entries
}

pub(super) fn mcp_picker_entry(
    context: &SessionRuntimeContext,
    statuses: &[McpServerStatus],
    server_id: &str,
    tool_name: &str,
) -> WebSearchPickerEntry {
    let (label, note) = if let Some(supplier) = builtin::supplier_for_server_id(server_id) {
        let note = if tool_name == builtin::SEARCH_TOOL {
            builtin::status_note(context, supplier)
        } else {
            Some("Built-in search tool not found".into())
        };
        (builtin::label(supplier).into(), note)
    } else {
        let status = statuses
            .iter()
            .find(|status| status.server_id.0 == server_id);
        (
            format!("{server_id} / {tool_name}"),
            selected_tool_note(status, tool_name).map(str::to_owned),
        )
    };
    WebSearchPickerEntry {
        label,
        choice: WebSearchChoice::Mcp {
            server_id: server_id.into(),
            tool_name: tool_name.into(),
        },
        note,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    #[test]
    fn only_tools_that_can_be_called_with_just_query_are_eligible() {
        assert_eq!(
            [
                supports_query_input(
                    &json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]})
                ),
                supports_query_input(
                    &json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"}}})
                ),
                supports_query_input(
                    &json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query","tenant"]})
                ),
                supports_query_input(
                    &json!({"type":"object","properties":{"query":{"type":"object"}}})
                ),
                supports_query_input(
                    &json!({"type":"object","properties":{"query":{"type":"string"}},"allOf":[{"required":["tenant"]}]})
                ),
            ],
            [true, true, false, false, false]
        );
    }
}
