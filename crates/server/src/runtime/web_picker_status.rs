//! Read-only projection of web routes, hosted evidence, and selectable backends.

use std::sync::Arc;

use devo_core::{
    McpServerId, McpServerStatus, McpStartupState, ResolvedWebFetchConfig, ResolvedWebSearchConfig,
    TurnConfig,
};
use devo_protocol::native::rpc_session::{
    SessionWebStatusReadParams, SessionWebStatusReadResult, WebApiKeyStatus, WebBackendStatus,
    WebPickerGroup, WebSearchPickerEntry,
};
use devo_protocol::native::session::{WebBackendChoice, WebSearchChoice};
use serde_json::Value;

use super::{Capability, HostedSupport, ProbePolicy, hosted_picker_note, hosted_status_note};
use crate::SuccessResponse;
use crate::runtime::{ServerRuntime, web_search_builtin as builtin, web_search_mcp as mcp};
use crate::session_context::SessionRuntimeContext;

const PROVIDER: &str = "Model provider";
const OFF: &str = "Off";

impl ServerRuntime {
    pub(in crate::runtime) async fn handle_native_session_web_status_read(
        self: &Arc<Self>,
        id: Value,
        params: Value,
    ) -> Value {
        let params: SessionWebStatusReadParams =
            match self.parse_rpc_params(id.clone(), params, "session/web/status/read") {
                Ok(params) => params,
                Err(error) => return error,
            };
        let Some(handle) = self.session(params.session_id).await else {
            return self.session_missing(id);
        };
        let (Some(mut summary), Some(context)) =
            (handle.summary().await, handle.runtime_context().await)
        else {
            return self.session_missing(id);
        };
        let config = match context.resolve_session_turn_config(
            &summary,
            summary.model_selection(),
            summary.settings.reasoning_effort.clone(),
        ) {
            Ok(config) => config,
            Err(error) => {
                return self.error_response(
                    id,
                    crate::ProtocolErrorCode::InvalidParams,
                    error.to_string(),
                );
            }
        };
        let search_choice = summary
            .settings
            .web_search
            .take()
            .unwrap_or(WebSearchChoice::Inherit);
        let configured_backend = if search_choice == WebSearchChoice::Inherit {
            describe_configured_backend(&config)
        } else {
            context
                .resolve_session_turn_config(
                    &summary,
                    summary.model_selection(),
                    summary.settings.reasoning_effort.clone(),
                )
                .map(|configured| describe_configured_backend(&configured))
                .unwrap_or_else(|_| "Unavailable".into())
        };

        let router = context.provider_router();
        // Route snapshots can change between launch and opening the picker.
        // On a cache miss, enqueue a deduplicated check; never await network I/O.
        let policy = if matches!(config.web_search, ResolvedWebSearchConfig::Disabled)
            && matches!(config.web_fetch, ResolvedWebFetchConfig::Disabled)
        {
            ProbePolicy::CacheOnly
        } else {
            ProbePolicy::ScheduleIfNeeded
        };
        let hosted = |capability| {
            self.hosted_web_checks
                .inspect(Arc::clone(&router), &config, capability, policy)
        };
        let hosted_search = hosted(Capability::Search);
        let hosted_fetch = hosted(Capability::Fetch);
        let statuses = context.mcp_manager.statuses().await.unwrap_or_default();
        let active_choice = match &search_choice {
            WebSearchChoice::Unified { .. } => Some(search_choice.clone()),
            WebSearchChoice::Inherit => unified_configured_choice(&config),
            WebSearchChoice::Provider | WebSearchChoice::Off | WebSearchChoice::Mcp { .. } => None,
        };
        let current_backend = active_choice
            .is_none()
            .then(|| describe_configured_backend(&config));
        let search_choices = picker_entries(
            &context,
            &statuses,
            active_choice.as_ref(),
            hosted_search,
            hosted_fetch,
        );
        let search_note = match &config.web_search {
            ResolvedWebSearchConfig::Provider => hosted_status_note(hosted_search),
            ResolvedWebSearchConfig::Mcp { server_id, tool_name } => {
                search_choices.iter().find(|entry| {
                    matches!(&entry.choice, WebSearchChoice::Unified { backend: WebBackendChoice::Mcp { server_id: id, tool_name: name }} if id == server_id && name == tool_name)
                }).and_then(|entry| entry.note.clone().or_else(|| match entry.key_status {
                    Some(WebApiKeyStatus::Required) => Some("API key required".into()),
                    Some(WebApiKeyStatus::Rejected) => Some("API key rejected".into()),
                    Some(WebApiKeyStatus::Saved) | None => None,
                }))
            }
            ResolvedWebSearchConfig::Local(_) | ResolvedWebSearchConfig::Disabled => None,
        };
        let fetch_note = match &config.web_fetch {
            ResolvedWebFetchConfig::Provider => hosted_status_note(hosted_fetch),
            ResolvedWebFetchConfig::Mcp { provider } => builtin::status_note(&context, *provider),
            ResolvedWebFetchConfig::CustomMcp { server_id, .. } => {
                mcp::compatible_fetch_tool(&statuses, &McpServerId(server_id.clone()))
                    .is_none()
                    .then(|| "No compatible web_fetch on this server".into())
            }
            ResolvedWebFetchConfig::Local | ResolvedWebFetchConfig::Disabled => None,
        };
        serde_json::to_value(SuccessResponse {
            id,
            result: SessionWebStatusReadResult {
                search: WebBackendStatus {
                    backend: search_label(&config.web_search),
                    note: search_note,
                },
                fetch: WebBackendStatus {
                    backend: fetch_label(&config.web_fetch),
                    note: fetch_note,
                },
                search_choice,
                search_choices,
                configured_backend: Some(configured_backend),
                active_choice,
                current_backend,
            },
        })
        .expect("serialize session web status")
    }
}

fn search_label(search: &ResolvedWebSearchConfig) -> String {
    match search {
        ResolvedWebSearchConfig::Provider => PROVIDER.into(),
        ResolvedWebSearchConfig::Disabled => OFF.into(),
        ResolvedWebSearchConfig::Local(local) => local.provider_id.clone(),
        ResolvedWebSearchConfig::Mcp {
            server_id,
            tool_name,
        } => builtin::supplier_for_server_id(server_id).map_or_else(
            || format!("{server_id} / {tool_name}"),
            |supplier| builtin::label(supplier).into(),
        ),
    }
}

fn fetch_label(fetch: &ResolvedWebFetchConfig) -> String {
    match fetch {
        ResolvedWebFetchConfig::Provider => PROVIDER.into(),
        ResolvedWebFetchConfig::Disabled => OFF.into(),
        ResolvedWebFetchConfig::Local => "Direct HTTP".into(),
        ResolvedWebFetchConfig::Mcp { provider } => builtin::label(*provider).into(),
        ResolvedWebFetchConfig::CustomMcp {
            server_id,
            tool_name,
        } => format!("{server_id} / {tool_name}"),
    }
}

fn unified_configured_choice(config: &TurnConfig) -> Option<WebSearchChoice> {
    let backend = match (&config.web_search, &config.web_fetch) {
        (ResolvedWebSearchConfig::Provider, ResolvedWebFetchConfig::Provider) => {
            WebBackendChoice::Provider
        }
        (ResolvedWebSearchConfig::Disabled, ResolvedWebFetchConfig::Disabled) => {
            WebBackendChoice::Off
        }
        (
            ResolvedWebSearchConfig::Mcp {
                server_id,
                tool_name,
            },
            ResolvedWebFetchConfig::Mcp { provider },
        ) if builtin::server_id(*provider) == server_id && tool_name == builtin::SEARCH_TOOL => {
            WebBackendChoice::Mcp {
                server_id: server_id.clone(),
                tool_name: tool_name.clone(),
            }
        }
        _ => return None,
    };
    Some(WebSearchChoice::Unified { backend })
}

fn describe_configured_backend(config: &TurnConfig) -> String {
    let search = search_label(&config.web_search);
    if unified_configured_choice(config).is_some() {
        return search;
    }
    let fetch = fetch_label(&config.web_fetch);
    format!("{search} (search) · {fetch} (fetch)")
}

fn picker_entries(
    context: &SessionRuntimeContext,
    statuses: &[McpServerStatus],
    active: Option<&WebSearchChoice>,
    search: HostedSupport,
    fetch: HostedSupport,
) -> Vec<WebSearchPickerEntry> {
    let mut entries = vec![WebSearchPickerEntry {
        label: PROVIDER.into(),
        choice: WebSearchChoice::Unified {
            backend: WebBackendChoice::Provider,
        },
        note: Some(hosted_picker_note(search, fetch)),
        group: None,
        key_status: None,
        unavailable: (search == HostedSupport::Unavailable && fetch == HostedSupport::Unavailable)
            .then_some(true),
    }];
    entries.extend(builtin::SUPPLIERS.into_iter().map(|supplier| {
        mcp_entry(
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
                tool.server_id == status.server_id && mcp::supports_query_input(&tool.input_schema)
            })
        })
        .map(|tool| mcp_entry(context, statuses, &tool.server_id.0, &tool.name))
        .collect();
    discovered.sort_by(|left, right| left.label.cmp(&right.label));
    discovered.dedup_by(|left, right| left.choice == right.choice);
    entries.extend(discovered);
    if let Some(
        choice @ WebSearchChoice::Unified {
            backend:
                WebBackendChoice::Mcp {
                    server_id,
                    tool_name,
                },
        },
    ) = active
        && !entries.iter().any(|entry| &entry.choice == choice)
    {
        entries.push(mcp_entry(context, statuses, server_id, tool_name));
    }
    entries.extend([
        WebSearchPickerEntry {
            label: OFF.into(),
            choice: WebSearchChoice::Unified {
                backend: WebBackendChoice::Off,
            },
            note: None,
            group: None,
            key_status: None,
            unavailable: None,
        },
        WebSearchPickerEntry {
            label: "Use configured backend".into(),
            choice: WebSearchChoice::Inherit,
            note: None,
            group: None,
            key_status: None,
            unavailable: None,
        },
    ]);
    entries
}

fn mcp_entry(
    context: &SessionRuntimeContext,
    statuses: &[McpServerStatus],
    server_id: &str,
    tool_name: &str,
) -> WebSearchPickerEntry {
    let choice = WebSearchChoice::Unified {
        backend: WebBackendChoice::Mcp {
            server_id: server_id.into(),
            tool_name: tool_name.into(),
        },
    };
    if let Some(supplier) = builtin::supplier_for_server_id(server_id) {
        let (key_status, mut note) = builtin::credential_status(context, supplier);
        if tool_name != builtin::SEARCH_TOOL {
            note = Some("Built-in search tool not found".into());
        }
        return WebSearchPickerEntry {
            label: builtin::label(supplier).into(),
            choice,
            unavailable: (note.is_some() || key_status != WebApiKeyStatus::Saved).then_some(true),
            note,
            group: Some(WebPickerGroup::Builtin),
            key_status: Some(key_status),
        };
    }
    let id = McpServerId(server_id.into());
    let status = statuses.iter().find(|status| status.server_id == id);
    let problem = mcp::selected_tool_note(status, tool_name);
    let note = problem.or_else(|| {
        mcp::compatible_fetch_tool(statuses, &id)
            .is_none()
            .then_some("Search only")
    });
    WebSearchPickerEntry {
        label: format!("{server_id} / {tool_name}"),
        choice,
        note: note.map(str::to_owned),
        group: Some(WebPickerGroup::Custom),
        key_status: None,
        unavailable: problem.is_some().then_some(true),
    }
}
