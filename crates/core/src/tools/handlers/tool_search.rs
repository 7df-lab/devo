use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use devo_protocol::ToolDefinition;

use crate::contracts::ToolCallError;
use crate::contracts::ToolContext;
use crate::contracts::ToolProgressSender;
use crate::contracts::ToolResult;
use crate::contracts::ToolResultContent;
use crate::deferred_loading::DeferredLoadingConfig;
use crate::deferred_loading::LoadedDeferredTools;
use crate::deferred_loading::execute_tool_search;
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolExecutionMode;
use crate::tool_spec::ToolOutputMode;
use crate::tool_spec::ToolSpec;

pub struct ToolSearchHandler {
    definitions: Vec<ToolDefinition>,
    loaded_tools: Arc<Mutex<LoadedDeferredTools>>,
    config: DeferredLoadingConfig,
    spec: ToolSpec,
}

impl ToolSearchHandler {
    pub fn new(
        definitions: Vec<ToolDefinition>,
        loaded_tools: Arc<Mutex<LoadedDeferredTools>>,
        config: DeferredLoadingConfig,
    ) -> Self {
        Self {
            definitions,
            loaded_tools,
            config,
            spec: ToolSpec {
                name: "ToolSearch".into(),
                description: "Load schemas for deferred tools so they can be called.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([(
                        "query".to_string(),
                        JsonSchema::string(Some(
                            "Tool selection query, for example select:websearch,skill",
                        )),
                    )]),
                    Some(vec!["query".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![],
                supports_parallel: true,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
                display_name: Some("ToolSearch".to_string()),
                supports_cancellation: None,
                supports_streaming: None,
            },
        }
    }
}

#[async_trait]
impl ToolHandler for ToolSearchHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        _ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let query = input["query"].as_str().ok_or_else(|| {
            ToolCallError::InvalidInput("Expected query format: select:<name>[,<name>...]".into())
        })?;

        let mut loaded_tools = self.loaded_tools.lock().map_err(|_| {
            ToolCallError::InternalError("loaded deferred tool state lock poisoned".into())
        })?;
        let result = execute_tool_search(query, &self.definitions, &mut loaded_tools, &self.config)
            .map_err(ToolCallError::ExecutionFailed)?;

        Ok(ToolResult::success(
            ToolResultContent::Text(result.summary()),
            "Tools loaded",
        ))
    }
}
