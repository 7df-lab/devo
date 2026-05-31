mod apply_patch;
mod bash;
mod exec_command;
mod file_write;
mod glob;
mod grep;
mod invalid;
mod lsp;
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
pub use plan::PlanHandler;
pub use question::QuestionHandler;
pub use read::ReadHandler;
pub use shell_command::ShellCommandHandler;
pub use skill::SkillHandler;
pub use task::TaskHandler;
pub use tool_search::ToolSearchHandler;
pub use webfetch::WebFetchHandler;
pub use websearch::WebSearchHandler;

use std::sync::Arc;

use crate::deferred_loading::DeferredLoadingConfig;
use crate::deferred_loading::LoadedDeferredTools;
use crate::handler_kind::ToolHandlerKind;
use crate::json_schema::JsonSchema;
use crate::registry::ToolRegistryBuilder;
use crate::registry_plan::{ToolPlanConfig, build_tool_registry_plan};
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolPreparationFeedback, ToolSpec};
use crate::unified_exec::store::ProcessStore;
use std::collections::BTreeMap;

pub fn build_registry_from_plan(config: &ToolPlanConfig) -> crate::registry::ToolRegistry {
    let plan = build_tool_registry_plan(config);
    let mut builder = ToolRegistryBuilder::new();

    for spec in plan.specs {
        builder.push_spec(spec);
    }
    builder.push_spec(tool_search_spec());

    let process_store = Arc::new(ProcessStore::new());
    let loaded_deferred_tools = Arc::new(std::sync::Mutex::new(LoadedDeferredTools::default()));
    builder.set_unified_exec_store(Arc::clone(&process_store));
    builder.set_loaded_deferred_tools(Arc::clone(&loaded_deferred_tools));

    for (kind, name) in plan.handlers {
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
                builder.tool_definitions(),
                Arc::clone(&loaded_deferred_tools),
                DeferredLoadingConfig::default(),
            )),
        };
        builder.register_handler(&name, handler);
    }
    builder.register_handler(
        "ToolSearch",
        Arc::new(ToolSearchHandler::new(
            builder.tool_definitions(),
            Arc::clone(&loaded_deferred_tools),
            DeferredLoadingConfig::default(),
        )),
    );

    builder.build()
}

fn tool_search_spec() -> ToolSpec {
    ToolSpec {
        name: "ToolSearch".to_string(),
        description: "Load schemas for deferred tools so they can be called. Use query \"select:<name>[,<name>...]\" with exact tool names from the Deferred tools reminder.".to_string(),
        input_schema: JsonSchema::object(
            BTreeMap::from([(
                "query".to_string(),
                JsonSchema::string(Some("Tool selection query, for example select:websearch,skill")),
            )]),
            Some(vec!["query".to_string()]),
            Some(false),
        ),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::ReadOnly,
        capability_tags: vec![],
        supports_parallel: true,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: Some("ToolSearch".to_string()),
        supports_cancellation: None,
        supports_streaming: None,
    }
}
