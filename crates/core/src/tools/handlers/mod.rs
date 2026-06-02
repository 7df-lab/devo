mod apply_patch;
mod bash;
mod exec_command;
mod file_write;
mod glob;
mod grep;
mod invalid;
mod lsp;
mod mcp;
mod plan;
mod question;
mod read;
mod shell_command;
mod skill;
mod task;
mod tool_search;
mod webfetch;
mod websearch;

pub use apply_patch::ApplyPatchHandler;
pub use bash::BashHandler;
pub use exec_command::{ExecCommandHandler, WriteStdinHandler};
pub use file_write::WriteHandler;
pub use glob::GlobHandler;
pub use grep::GrepHandler;
pub use invalid::InvalidHandler;
pub use lsp::LspHandler;
pub use mcp::{McpToolHandler, mcp_search_text, mcp_tool_spec};
pub use plan::PlanHandler;
pub use question::QuestionHandler;
pub use read::ReadHandler;
pub use shell_command::ShellCommandHandler;
pub use skill::SkillHandler;
pub use task::TaskHandler;
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
use crate::unified_exec::store::ProcessStore;

pub fn build_registry_from_plan(config: &ToolPlanConfig) -> crate::registry::ToolRegistry {
    let plan = build_tool_registry_plan(config);
    let specs = plan.specs;
    let handlers = plan.handlers;
    let mut builder = ToolRegistryBuilder::new();

    for spec in specs {
        builder.push_spec(spec);
    }
    build_registry_from_builder(handlers, builder, Vec::new())
}

pub async fn build_registry_from_plan_with_mcp(
    config: &ToolPlanConfig,
    mcp_manager: Arc<dyn McpManager>,
) -> crate::registry::ToolRegistry {
    let plan = build_tool_registry_plan(config);
    let specs = plan.specs;
    let handlers = plan.handlers;
    let mut builder = ToolRegistryBuilder::new();

    for spec in specs {
        builder.push_spec(spec);
    }

    let mut mcp_handlers = Vec::new();
    let mcp_tools = match mcp_manager.discover_tools().await {
        Ok(tools) => tools,
        Err(err) => {
            tracing::warn!(error = %err, "failed to discover MCP tools");
            Vec::new()
        }
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

    build_registry_from_builder(handlers, builder, mcp_handlers)
}

fn build_registry_from_builder(
    handlers: Vec<(ToolHandlerKind, String)>,
    mut builder: ToolRegistryBuilder,
    mcp_handlers: Vec<(String, Arc<dyn ToolHandler>)>,
) -> crate::registry::ToolRegistry {
    builder.push_spec(tool_search_spec());

    let process_store = Arc::new(ProcessStore::new());
    let loaded_deferred_tools = Arc::new(std::sync::Mutex::new(LoadedDeferredTools::default()));
    builder.set_unified_exec_store(Arc::clone(&process_store));
    builder.set_loaded_deferred_tools(Arc::clone(&loaded_deferred_tools));

    for (kind, name) in handlers {
        let handler: Arc<dyn ToolHandler> = match kind {
            ToolHandlerKind::Bash => Arc::new(BashHandler::new()),
            ToolHandlerKind::ShellCommand => Arc::new(ShellCommandHandler::new()),
            ToolHandlerKind::Read => Arc::new(ReadHandler::new()),
            ToolHandlerKind::Write => Arc::new(WriteHandler::new()),
            ToolHandlerKind::Glob => Arc::new(GlobHandler::new()),
            ToolHandlerKind::Grep => Arc::new(GrepHandler::new()),
            ToolHandlerKind::ApplyPatch => Arc::new(ApplyPatchHandler::new()),
            ToolHandlerKind::Plan => Arc::new(PlanHandler::new()),
            ToolHandlerKind::Question => Arc::new(QuestionHandler::new()),
            ToolHandlerKind::Task => Arc::new(TaskHandler::new()),
            ToolHandlerKind::WebFetch => Arc::new(WebFetchHandler::new()),
            ToolHandlerKind::WebSearch => Arc::new(WebSearchHandler::new()),
            ToolHandlerKind::Skill => Arc::new(SkillHandler::new()),
            ToolHandlerKind::Lsp => Arc::new(LspHandler::new()),
            ToolHandlerKind::Invalid => Arc::new(InvalidHandler::new()),
            ToolHandlerKind::ExecCommand => {
                Arc::new(ExecCommandHandler::new(Arc::clone(&process_store)))
            }
            ToolHandlerKind::WriteStdin => {
                Arc::new(WriteStdinHandler::new(Arc::clone(&process_store)))
            }
            ToolHandlerKind::ToolSearch => Arc::new(ToolSearchHandler::new(
                builder.tool_search_entries(),
                Arc::clone(&loaded_deferred_tools),
                builder.effective_deferred_loading_config(&DeferredLoadingConfig::default()),
            )),
        };
        builder.register_handler(&name, handler);
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
