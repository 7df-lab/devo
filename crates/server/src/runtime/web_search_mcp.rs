//! Route the session's selected search tool through the normal MCP manager.

use std::sync::Arc;

use devo_core::{
    McpManager, McpServerId, McpServerStatus, McpStartupState, McpToolDescriptor,
    ResolvedWebFetchConfig, ResolvedWebSearchConfig,
};
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
    CustomMcp {
        server_id: McpServerId,
        tool_name: String,
        manager: Result<Arc<dyn McpManager>, String>,
    },
}

impl WebRoutes {
    pub(super) fn for_turn(
        context: &SessionRuntimeContext,
        search: &ResolvedWebSearchConfig,
        fetch: &ResolvedWebFetchConfig,
    ) -> Self {
        let search = McpWebSearchRoute::for_turn(context, search);
        let fetch = match fetch {
            ResolvedWebFetchConfig::Mcp { provider } => {
                let server_id = McpServerId(builtin::server_id(*provider).into());
                let manager = search
                    .as_ref()
                    .filter(|route| route.server_id == server_id)
                    .map_or_else(
                        || builtin::manager(context, *provider).map_err(|error| error.to_string()),
                        |route| route.manager.clone(),
                    );
                WebFetchRoute::Mcp { server_id, manager }
            }
            ResolvedWebFetchConfig::CustomMcp {
                server_id,
                tool_name,
            } if tool_name == "web_fetch" => {
                let server_id = McpServerId(server_id.clone());
                let manager = search
                    .as_ref()
                    .filter(|route| route.server_id == server_id)
                    .map_or_else(
                        || Ok(Arc::clone(&context.mcp_manager)),
                        |route| route.manager.clone(),
                    );
                WebFetchRoute::CustomMcp {
                    server_id,
                    tool_name: tool_name.clone(),
                    manager,
                }
            }
            ResolvedWebFetchConfig::CustomMcp { .. } => WebFetchRoute::Unavailable(
                "Custom MCP fetch must use the canonical web_fetch tool.",
            ),
            ResolvedWebFetchConfig::Disabled => {
                WebFetchRoute::Unavailable("Web fetch is off for this turn.")
            }
            ResolvedWebFetchConfig::Provider => WebFetchRoute::Unavailable(
                "This turn uses model-hosted web fetch, not the Python helper.",
            ),
            ResolvedWebFetchConfig::ProviderViaSearch => WebFetchRoute::Unavailable(
                "This turn uses OpenAI Responses hosted web_search open_page, not the Python helper.",
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
    supports_single_string_input(schema, "query")
}

fn supports_single_string_input(schema: &Value, field: &str) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("object")
        && schema
            .pointer(&format!("/properties/{field}/type"))
            .and_then(Value::as_str)
            == Some("string")
        && schema.get("required").is_none_or(|required| {
            required
                .as_array()
                .is_some_and(|fields| fields.iter().all(|name| name == field))
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

pub(super) fn supports_url_input(schema: &Value) -> bool {
    supports_single_string_input(schema, "url")
}

/// Returns the canonical fetch tool only when it is ready and can be invoked
/// unambiguously with `{ "url": ... }` on this exact MCP server.
pub(super) fn compatible_fetch_tool<'a>(
    statuses: &'a [McpServerStatus],
    server_id: &McpServerId,
) -> Option<&'a McpToolDescriptor> {
    let mut matching_servers = statuses
        .iter()
        .filter(|status| status.server_id == *server_id);
    let server = matching_servers.next()?;
    if matching_servers.next().is_some() || server.startup_state != McpStartupState::Ready {
        return None;
    }
    let mut named_tools = server
        .tools
        .iter()
        .filter(|tool| tool.server_id == *server_id && tool.name == "web_fetch");
    let tool = named_tools.next()?;
    (named_tools.next().is_none() && supports_url_input(&tool.input_schema)).then_some(tool)
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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    #[test]
    fn canonical_fetch_requires_ready_same_server_and_compatible_url_schema() {
        let server_id = McpServerId("docs".into());
        let status = |startup_state: McpStartupState,
                      tool_server_id: McpServerId,
                      name: &str,
                      input_schema: Value| McpServerStatus {
            server_id: server_id.clone(),
            startup_state,
            auth_state: devo_core::McpAuthState::Authenticated,
            tools: vec![McpToolDescriptor {
                server_id: tool_server_id,
                name: name.into(),
                description: String::new(),
                input_schema,
            }],
            resources: Vec::new(),
            resource_templates: Vec::new(),
            last_refreshed_at: None,
        };
        let compatible = status(
            McpStartupState::Ready,
            server_id.clone(),
            "web_fetch",
            json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}),
        );
        assert_eq!(
            compatible_fetch_tool(std::slice::from_ref(&compatible), &server_id),
            compatible.tools.first(),
        );
        let mut duplicate_tool = compatible.clone();
        duplicate_tool.tools.extend(compatible.tools.clone());
        assert_eq!(compatible_fetch_tool(&[duplicate_tool], &server_id), None);
        assert_eq!(
            compatible_fetch_tool(&[compatible.clone(), compatible.clone()], &server_id),
            None,
        );
        let unsupported_schema = status(
            McpStartupState::Ready,
            server_id.clone(),
            "web_fetch",
            json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url","token"]}),
        );
        let wrong_server = status(
            McpStartupState::Ready,
            McpServerId("other".into()),
            "web_fetch",
            json!({"type":"object","properties":{"url":{"type":"string"}}}),
        );
        let not_ready = status(
            McpStartupState::AuthRequired,
            server_id.clone(),
            "web_fetch",
            json!({"type":"object","properties":{"url":{"type":"string"}}}),
        );
        for status in [unsupported_schema, wrong_server, not_ready] {
            assert_eq!(compatible_fetch_tool(&[status], &server_id), None);
        }
    }

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
