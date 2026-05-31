use async_trait::async_trait;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolSpec;

pub struct LspHandler {
    spec: ToolSpec,
}

impl Default for LspHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl LspHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec::new(
                "lsp",
                "Language Server Protocol integration for code intelligence operations.",
                JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "operation".to_string(),
                            JsonSchema::string(Some("The LSP operation to perform")),
                        ),
                        (
                            "file_path".to_string(),
                            JsonSchema::string(Some("The file path to operate on")),
                        ),
                    ]),
                    Some(vec!["operation".to_string()]),
                    None,
                ),
            ),
        }
    }
}

#[async_trait]
impl ToolHandler for LspHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        _ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let operation = input["operation"].as_str().unwrap_or("unknown");
        Ok(ToolResult::success(
            ToolResultContent::Text(format!("LSP request received for {operation}")),
            "LSP request processed",
        ))
    }
}
