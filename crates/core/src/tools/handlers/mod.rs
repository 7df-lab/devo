mod agent;
mod apply_patch;
mod edit;
mod exec_command;
mod file_change_metadata;
mod file_write;
mod glob;
mod goal_update;
mod grep;
#[cfg(test)]
mod invalid;
mod ipython;
#[cfg(test)]
#[path = "ipython_tests.rs"]
mod ipython_tests;
mod lsp;
mod mcp;
mod plan;
mod question;
mod read;
mod ripgrep;
mod shell_command;
mod skill;
mod tool_search;
mod webfetch;
mod websearch;

pub(crate) use agent::register_agent_tools;
pub use apply_patch::ApplyPatchHandler;
pub use edit::EditHandler;
pub use exec_command::{ExecCommandHandler, WriteStdinHandler};
pub use file_write::WriteHandler;
pub use glob::GlobHandler;
pub use goal_update::{GoalUpdateHandler, goal_update_spec};
pub use grep::GrepHandler;
#[cfg(test)]
pub use invalid::InvalidHandler;
pub use ipython::{
    IpythonHandler, KernelNamespaceRestoreState, ensure_kernel, ensure_kernel_with_restore_state,
    kernel_namespace_manifest_path, kernel_namespace_snapshot_path,
};
pub use lsp::LspHandler;
pub use mcp::{McpToolHandler, mcp_search_text, mcp_tool_spec};
pub use plan::PlanHandler;
pub use question::QuestionHandler;
pub use read::ReadHandler;
pub use shell_command::ShellCommandHandler;
pub use skill::SkillHandler;
pub use tool_search::{ToolSearchHandler, tool_search_spec};
pub use webfetch::WebFetchHandler;
pub use websearch::WebSearchHandler;

use std::sync::Arc;

use crate::deferred_loading::DeferredLoadingConfig;
use crate::deferred_loading::LoadedDeferredTools;
use crate::handler_kind::ToolHandlerKind;
use crate::mcp::McpManager;
use crate::mcp::build_mcp_tool_exposure;
use crate::registry::ToolExposure;
use crate::registry::ToolRegistryBuilder;
use crate::registry_plan::{ToolPlanConfig, build_tool_registry_plan};
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolSpec;
use crate::unified_exec::store::ProcessStore;

fn register_plan_specs(
    builder: &mut ToolRegistryBuilder,
    specs: Vec<ToolSpec>,
    config: &ToolPlanConfig,
) {
    for spec in specs {
        if config.execution_surface == devo_kernel::ExecutionSurface::Rlm && spec.name == "bash" {
            // Keep the permission-controlled handler registered for compatibility, but expose the
            // Python kernel as the only execution tool in the RLM model schema and ToolSearch.
            builder.push_spec_with_exposure(spec, ToolExposure::Hidden);
        } else {
            builder.push_spec(spec);
        }
    }
}

pub fn build_registry_from_plan(config: &ToolPlanConfig) -> crate::registry::ToolRegistry {
    let plan = build_tool_registry_plan(config);
    let specs = plan.specs;
    let handlers = plan.handlers;
    let mut builder = ToolRegistryBuilder::new();

    register_plan_specs(&mut builder, specs, config);
    build_registry_from_builder(handlers, builder, Vec::new(), None)
}

pub async fn build_registry_from_plan_with_mcp(
    config: &ToolPlanConfig,
    mcp_manager: Arc<dyn McpManager>,
) -> crate::registry::ToolRegistry {
    rebuild_registry_from_plan_with_mcp(config, mcp_manager, None).await
}

/// Rebuilds a tool registry from the current MCP manager, optionally reusing
/// the previous unified-exec [`ProcessStore`] and deferred-load state so live
/// sessions keep background processes across MCP enable/disable.
pub async fn rebuild_registry_from_plan_with_mcp(
    config: &ToolPlanConfig,
    mcp_manager: Arc<dyn McpManager>,
    previous: Option<&crate::registry::ToolRegistry>,
) -> crate::registry::ToolRegistry {
    build_registry_with_mcp_layer(
        config,
        mcp_manager,
        previous,
        None,
        McpDiscoveryMode::AllConfigured,
    )
    .await
}

/// Builds a turn-scoped registry and carries over MCP handlers from the session
/// registry. RLM turns expose `ipython` only and use the host manager for MCP
/// calls, so rediscovering global MCP schemas on every turn is unnecessary.
/// Keep session-scoped MCP handlers because ACP servers are not in the global
/// manager.
pub async fn build_registry_from_plan_with_session_mcp(
    config: &ToolPlanConfig,
    mcp_manager: Arc<dyn McpManager>,
    session_registry: &crate::registry::ToolRegistry,
) -> crate::registry::ToolRegistry {
    let discovery_mode = if config.execution_surface == devo_kernel::ExecutionSurface::Rlm {
        McpDiscoveryMode::SessionRegistryOnly
    } else {
        McpDiscoveryMode::AllConfigured
    };
    build_registry_with_mcp_layer(
        config,
        mcp_manager,
        None,
        Some(session_registry),
        discovery_mode,
    )
    .await
}

#[derive(Debug, Clone, Copy)]
enum McpDiscoveryMode {
    AllConfigured,
    SessionRegistryOnly,
}

async fn build_registry_with_mcp_layer(
    config: &ToolPlanConfig,
    mcp_manager: Arc<dyn McpManager>,
    previous: Option<&crate::registry::ToolRegistry>,
    session_registry: Option<&crate::registry::ToolRegistry>,
    discovery_mode: McpDiscoveryMode,
) -> crate::registry::ToolRegistry {
    let plan = build_tool_registry_plan(config);
    let specs = plan.specs;
    let handlers = plan.handlers;
    let mut builder = ToolRegistryBuilder::new();

    register_plan_specs(&mut builder, specs, config);
    let mut mcp_handlers = Vec::new();
    let mcp_tools = match discovery_mode {
        McpDiscoveryMode::AllConfigured => match mcp_manager.discover_tools().await {
            Ok(tools) => tools,
            Err(err) => {
                tracing::warn!(error = %err, "failed to discover MCP tools");
                Vec::new()
            }
        },
        McpDiscoveryMode::SessionRegistryOnly => Vec::new(),
    };
    let exposure = build_mcp_tool_exposure(&mcp_tools);
    for info in exposure.direct_tools {
        let spec = mcp_tool_spec(&info);
        let name = spec.name.clone();
        builder.set_search_text(&name, mcp_search_text(&info));
        builder.push_spec_with_exposure(spec, ToolExposure::Direct);
        mcp_handlers.push((
            name,
            Arc::new(McpToolHandler::new(Arc::clone(&mcp_manager), info)) as Arc<dyn ToolHandler>,
        ));
    }
    for info in exposure.deferred_tools {
        let spec = mcp_tool_spec(&info);
        let name = spec.name.clone();
        builder.set_search_text(&name, mcp_search_text(&info));
        builder.push_spec_with_exposure(spec, ToolExposure::Deferred);
        mcp_handlers.push((
            name,
            Arc::new(McpToolHandler::new(Arc::clone(&mcp_manager), info)) as Arc<dyn ToolHandler>,
        ));
    }
    if let Some(session_registry) = session_registry {
        for (name, handler) in session_registry.all_handlers() {
            let is_session_only_mcp = name.starts_with(crate::mcp::MCP_TOOL_PREFIX)
                && !mcp_handlers
                    .iter()
                    .any(|(registered, _)| registered == name);
            if !is_session_only_mcp {
                continue;
            }
            let Some(spec) = session_registry.spec(name) else {
                continue;
            };
            builder.push_spec_with_exposure(spec.clone(), ToolExposure::Direct);
            mcp_handlers.push((name.clone(), Arc::clone(handler)));
        }
    }

    build_registry_from_builder(handlers, builder, mcp_handlers, previous)
}

fn build_registry_from_builder(
    handlers: Vec<(ToolHandlerKind, String)>,
    mut builder: ToolRegistryBuilder,
    mcp_handlers: Vec<(String, Arc<dyn ToolHandler>)>,
    previous: Option<&crate::registry::ToolRegistry>,
) -> crate::registry::ToolRegistry {
    let process_store = previous
        .and_then(|registry| registry.unified_exec_store.clone())
        .unwrap_or_else(|| Arc::new(ProcessStore::new()));
    let background_tasks = Arc::new(crate::tools::background_tasks::BackgroundTaskStore::new(
        Arc::clone(&process_store),
    ));
    register_agent_tools(&mut builder, Arc::clone(&background_tasks));
    builder.push_spec(goal_update_spec());
    builder.push_spec(tool_search_spec());
    builder.restrict_model_exposure_to(&["ipython"]);

    let loaded_deferred_tools = previous
        .map(|registry| Arc::clone(&registry.loaded_deferred_tools))
        .unwrap_or_else(|| Arc::new(std::sync::Mutex::new(LoadedDeferredTools::default())));
    builder.set_unified_exec_store(Arc::clone(&process_store));
    builder.set_loaded_deferred_tools(Arc::clone(&loaded_deferred_tools));
    builder.register_handler("update_goal", Arc::new(GoalUpdateHandler::new()));
    for (kind, name) in handlers {
        let handler: Arc<dyn ToolHandler> = match kind {
            ToolHandlerKind::ShellCommand => Arc::new(ShellCommandHandler::new()),
            ToolHandlerKind::Read => Arc::new(ReadHandler::new()),
            ToolHandlerKind::Write => Arc::new(WriteHandler::new()),
            ToolHandlerKind::Edit => Arc::new(EditHandler::new()),
            ToolHandlerKind::Glob => Arc::new(GlobHandler::new()),
            ToolHandlerKind::Grep => Arc::new(GrepHandler::new()),
            ToolHandlerKind::ApplyPatch => Arc::new(ApplyPatchHandler::new()),
            ToolHandlerKind::Plan => Arc::new(PlanHandler::new()),
            ToolHandlerKind::Question => Arc::new(QuestionHandler::new()),
            ToolHandlerKind::WebFetch => Arc::new(WebFetchHandler::new()),
            ToolHandlerKind::WebSearch => Arc::new(WebSearchHandler::new()),
            ToolHandlerKind::Skill => Arc::new(SkillHandler::new()),
            ToolHandlerKind::Lsp => Arc::new(LspHandler::new()),
            ToolHandlerKind::ExecCommand => Arc::new(ExecCommandHandler::new(
                Arc::clone(&process_store),
                Arc::clone(&background_tasks),
            )),
            ToolHandlerKind::WriteStdin => {
                Arc::new(WriteStdinHandler::new(Arc::clone(&process_store)))
            }
            ToolHandlerKind::ToolSearch => Arc::new(ToolSearchHandler::new(
                builder.tool_search_entries(),
                Arc::clone(&loaded_deferred_tools),
                builder.effective_deferred_loading_config(&DeferredLoadingConfig::default()),
            )),
            ToolHandlerKind::Ipython => Arc::new(IpythonHandler::new()),
        };
        let legacy_alias = match kind {
            ToolHandlerKind::ShellCommand if name == "shell_command" => Some("bash"),
            ToolHandlerKind::Glob if name == "find" => Some("glob"),
            ToolHandlerKind::Question if name == "request_user_input" => Some("question"),
            ToolHandlerKind::WebSearch if name == "web_search" => Some("websearch"),
            _ => None,
        };
        builder.register_handler(&name, Arc::clone(&handler));
        if let Some(alias) = legacy_alias {
            builder.register_handler(alias, Arc::clone(&handler));
        }
        if kind == ToolHandlerKind::WebSearch && name == "web_search" {
            builder.register_handler("web-search", handler);
        }
    }
    for (name, handler) in mcp_handlers {
        builder.register_handler(&name, handler);
    }
    builder.register_handler(
        "ToolSearch",
        Arc::new(ToolSearchHandler::new(
            builder.tool_search_entries(),
            Arc::clone(&loaded_deferred_tools),
            builder.effective_deferred_loading_config(&DeferredLoadingConfig::default()),
        )),
    );

    builder.build()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use pretty_assertions::assert_eq;
    use serde_json::Value;

    use super::*;
    use crate::deferred_loading::{assemble_deferred_tool_prompt, execute_tool_search};
    use crate::mcp::McpError;
    use crate::mcp::McpManager;
    use crate::mcp::McpServerId;
    use crate::mcp::McpServerStatus;
    use crate::mcp::McpToolInfo;

    #[derive(Default)]
    struct EmptyMcpManager {
        discovery_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl McpManager for EmptyMcpManager {
        async fn statuses(&self) -> Result<Vec<McpServerStatus>, McpError> {
            Ok(Vec::new())
        }

        async fn discover_tools(&self) -> Result<Vec<McpToolInfo>, McpError> {
            self.discovery_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }

        async fn refresh(&self, server_id: &McpServerId) -> Result<McpServerStatus, McpError> {
            Err(McpError::McpServerUnavailable {
                server_id: server_id.clone(),
            })
        }

        async fn set_enabled(
            &self,
            server_id: &McpServerId,
            _enabled: bool,
        ) -> Result<McpServerStatus, McpError> {
            Err(McpError::McpServerUnavailable {
                server_id: server_id.clone(),
            })
        }

        async fn invoke_tool(
            &self,
            server_id: &McpServerId,
            tool_name: &str,
            _input: Value,
        ) -> Result<Value, McpError> {
            Err(McpError::McpToolInvocationFailed {
                server_id: server_id.clone(),
                tool_name: tool_name.to_string(),
                message: "empty manager".to_string(),
            })
        }

        async fn read_resource(
            &self,
            server_id: &McpServerId,
            uri: &str,
        ) -> Result<Value, McpError> {
            Err(McpError::McpResourceReadFailed {
                server_id: server_id.clone(),
                uri: uri.to_string(),
                message: "empty manager".to_string(),
            })
        }
    }

    #[test]
    fn default_registry_exposes_shell_command_and_accepts_bash_alias() {
        let registry = build_registry_from_plan(&ToolPlanConfig::default());

        assert!(registry.spec("shell_command").is_some());
        assert!(registry.spec("bash").is_none());
        assert!(registry.get("shell_command").is_some());
        assert!(registry.get("bash").is_some());
    }

    #[test]
    fn default_registry_exposes_update_goal_tool() {
        // Trace: L2-DES-GOAL-001
        let registry = build_registry_from_plan(&ToolPlanConfig::default());

        assert!(registry.spec("update_goal").is_some());
        assert!(registry.get("update_goal").is_some());
    }

    #[test]
    fn discrete_registry_retains_handlers_but_exposes_no_model_tools() {
        let registry = build_registry_from_plan(&ToolPlanConfig::default());

        for stale in ["read", "write", "edit", "find", "grep"] {
            assert!(registry.spec(stale).is_none());
        }
        assert!(registry.spec("shell_command").is_some());
        assert!(registry.spec("apply_patch").is_some());
        let loading = registry.effective_deferred_loading_config(&DeferredLoadingConfig::default());
        let prompt =
            assemble_deferred_tool_prompt(&registry.tool_definitions(), &BTreeSet::new(), &loading);
        assert!(prompt.exposed.is_empty());
        assert!(prompt.deferred.is_empty());
    }

    #[tokio::test]
    async fn rlm_registry_exposes_only_ipython_to_models() {
        let config = ToolPlanConfig {
            execution_surface: devo_kernel::ExecutionSurface::Rlm,
            ..ToolPlanConfig::default()
        };
        let manager = Arc::new(EmptyMcpManager::default());
        let discovery_calls = Arc::clone(&manager.discovery_calls);
        let registry = build_registry_from_plan_with_mcp(&config, manager).await;
        assert_eq!(discovery_calls.load(Ordering::SeqCst), 1);
        let definitions = registry.tool_definitions();
        let loading = registry.effective_deferred_loading_config(&DeferredLoadingConfig::default());
        let prompt = assemble_deferred_tool_prompt(&definitions, &BTreeSet::new(), &loading);

        assert_eq!(
            prompt
                .exposed
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["ipython"]
        );
        assert!(prompt.deferred.is_empty());
        assert!(registry.spec("bash").is_some());
        assert!(!definitions.iter().any(|tool| tool.name == "find"));

        let mut loaded = LoadedDeferredTools::default();
        assert!(
            execute_tool_search(
                "session",
                "select:ipython",
                &definitions,
                &mut loaded,
                &loading,
            )
            .is_ok()
        );
        assert!(
            execute_tool_search(
                "session",
                "select:bash",
                &definitions,
                &mut loaded,
                &loading,
            )
            .is_err()
        );
        assert!(
            execute_tool_search(
                "session",
                "select:find",
                &definitions,
                &mut loaded,
                &loading,
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn rebuild_registry_reuses_process_store() {
        let manager: Arc<dyn McpManager> = Arc::new(EmptyMcpManager::default());
        let first = rebuild_registry_from_plan_with_mcp(
            &ToolPlanConfig::default(),
            Arc::clone(&manager),
            None,
        )
        .await;
        let first_store = first
            .unified_exec_store()
            .expect("first registry should own a process store");
        let second =
            rebuild_registry_from_plan_with_mcp(&ToolPlanConfig::default(), manager, Some(&first))
                .await;
        let second_store = second
            .unified_exec_store()
            .expect("rebuilt registry should own a process store");
        assert!(Arc::ptr_eq(&first_store, &second_store));
    }

    #[tokio::test]
    async fn session_mcp_handlers_survive_rebuild_but_stay_hidden_from_models() {
        // Session MCP handlers survive turn-scoped registry rebuilds for internal
        // host operations, but the Python-only model policy hides their schemas.
        use crate::json_schema::JsonSchema;
        use crate::tool_spec::ToolExecutionMode;
        use crate::tool_spec::ToolOutputMode;
        use crate::tool_spec::ToolSpec;
        use std::collections::BTreeMap;

        let session_registry = {
            let mut builder = ToolRegistryBuilder::new();
            builder.push_spec_with_exposure(
                ToolSpec {
                    name: "mcp__session_tools__echo".to_string(),
                    description: "Session MCP echo".to_string(),
                    input_schema: JsonSchema::object(BTreeMap::new(), None, Some(false)),
                    output_mode: ToolOutputMode::Text,
                    execution_mode: ToolExecutionMode::ReadOnly,
                    capability_tags: vec![],
                    supports_parallel: false,
                    preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
                    display_name: None,
                    supports_cancellation: None,
                    supports_streaming: None,
                },
                ToolExposure::Direct,
            );
            builder.register_handler("mcp__session_tools__echo", Arc::new(InvalidHandler::new()));
            builder.build()
        };
        let manager = Arc::new(EmptyMcpManager::default());
        let discovery_calls = Arc::clone(&manager.discovery_calls);
        let manager: Arc<dyn McpManager> = manager;
        let config = ToolPlanConfig {
            execution_surface: devo_kernel::ExecutionSurface::Rlm,
            ..ToolPlanConfig::default()
        };
        let registry =
            build_registry_from_plan_with_session_mcp(&config, manager, &session_registry).await;
        assert_eq!(discovery_calls.load(Ordering::SeqCst), 0);

        let definitions = registry.tool_definitions();
        assert!(registry.spec("mcp__session_tools__echo").is_some());
        assert!(registry.get("mcp__session_tools__echo").is_some());
        assert!(
            definitions
                .iter()
                .any(|definition| definition.name == "mcp__session_tools__echo"),
            "the internal registry should retain the carried session MCP schema"
        );
        let loading = registry.effective_deferred_loading_config(&DeferredLoadingConfig::default());
        let prompt = assemble_deferred_tool_prompt(&definitions, &BTreeSet::new(), &loading);
        assert_eq!(
            prompt
                .exposed
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["ipython"]
        );
        assert!(prompt.deferred.is_empty());
        let mut loaded = LoadedDeferredTools::default();
        assert!(
            execute_tool_search(
                "session",
                "select:ipython",
                &definitions,
                &mut loaded,
                &loading,
            )
            .is_ok()
        );
        assert!(
            execute_tool_search(
                "session",
                "select:mcp__session_tools__echo",
                &definitions,
                &mut loaded,
                &loading,
            )
            .is_err()
        );
    }
}
