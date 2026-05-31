use async_trait::async_trait;

use crate::contracts::{ToolCallError, ToolContext, ToolProgressSender, ToolResult};
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolSpec;

pub struct InvalidHandler {
    spec: ToolSpec,
}

impl Default for InvalidHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl InvalidHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec::new(
                "invalid",
                "Placeholder for invalid or unrecognized tool names. Always returns an error.",
                crate::json_schema::JsonSchema::object(
                    std::collections::BTreeMap::new(),
                    None,
                    None,
                ),
            ),
        }
    }
}

#[async_trait]
impl ToolHandler for InvalidHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        _ctx: ToolContext,
        _input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        Err(ToolCallError::InvalidInput(
            "this is an invalid placeholder tool and should never be called".into(),
        ))
    }
}
