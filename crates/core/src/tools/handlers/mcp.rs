use std::sync::Arc;

use crate::mcp::McpManager;
use crate::mcp::McpToolInfo;
use async_trait::async_trait;

use crate::contracts::ToolCallError;
use crate::contracts::ToolContext;
use crate::contracts::ToolProgressSender;
use crate::contracts::ToolResult;
use crate::contracts::ToolResultContent;
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolCapabilityTag;
use crate::tool_spec::ToolExecutionMode;
use crate::tool_spec::ToolOutputMode;
use crate::tool_spec::ToolPreparationFeedback;
use crate::tool_spec::ToolSpec;

pub struct McpToolHandler {
    manager: Arc<dyn McpManager>,
    info: McpToolInfo,
    spec: ToolSpec,
}

impl McpToolHandler {
    pub fn new(manager: Arc<dyn McpManager>, info: McpToolInfo) -> Self {
        let spec = mcp_tool_spec(&info);
        Self {
            manager,
            info,
            spec,
        }
    }
}

#[async_trait]
impl ToolHandler for McpToolHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        _ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let result = self
            .manager
            .invoke_tool(&self.info.server_id, &self.info.raw_tool_name, input)
            .await
            .map_err(|err| ToolCallError::ExecutionFailed(err.to_string()))?;

        Ok(ToolResult::success(
            ToolResultContent::Json(result),
            format!("Called MCP tool {}", self.info.raw_tool_name),
        ))
    }
}

pub fn mcp_tool_spec(info: &McpToolInfo) -> ToolSpec {
    ToolSpec {
        name: info.flat_name.clone(),
        description: format!(
            "MCP tool from {}. {}",
            info.server_display_name,
            info.description()
        ),
        input_schema: serde_json::from_value(info.input_schema())
            .unwrap_or_else(|_| JsonSchema::object(Default::default(), None, Some(true))),
        output_mode: ToolOutputMode::StructuredJson,
        execution_mode: if info.is_read_only() {
            ToolExecutionMode::ReadOnly
        } else {
            ToolExecutionMode::Mutating
        },
        capability_tags: Vec::<ToolCapabilityTag>::new(),
        supports_parallel: info.supports_parallel_tool_calls || info.is_read_only(),
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: Some(info.raw_tool_name.clone()),
        supports_cancellation: None,
        supports_streaming: None,
    }
}

pub fn mcp_search_text(info: &McpToolInfo) -> String {
    let mut schema_properties = info
        .input_schema()
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    schema_properties.sort();

    let mut parts = vec![
        info.flat_name.clone(),
        info.callable_name.clone(),
        info.raw_tool_name.clone(),
        info.server_id.0.clone(),
        info.server_display_name.clone(),
        info.description(),
    ];
    if let Some(source_description) = info.source_description.as_deref() {
        let source_description = source_description.trim();
        if !source_description.is_empty() {
            parts.push(source_description.to_string());
        }
    }
    parts.extend(schema_properties);
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;
    use crate::mcp::McpServerId;

    #[test]
    fn mcp_tool_spec_uses_normalized_core_metadata() {
        let info = McpToolInfo::new(
            McpServerId("Docs Server".into()),
            "Docs Server".into(),
            "search-docs".into(),
            Some("Search the docs.".into()),
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
            true,
            false,
        );

        let spec = mcp_tool_spec(&info);

        assert_eq!(
            serde_json::to_value(spec).expect("tool spec should serialize"),
            json!({
                "name": "mcp__docs_server__search_docs",
                "description": "MCP tool from Docs Server. Search the docs.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    },
                    "required": ["query"]
                },
                "output_mode": "StructuredJson",
                "execution_mode": "ReadOnly",
                "capability_tags": [],
                "supports_parallel": true,
                "preparation_feedback": "None",
                "display_name": "search-docs"
            })
        );
    }
}
