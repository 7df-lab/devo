use async_trait::async_trait;

use crate::apply_patch::exec_apply_patch;
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
use crate::tool_spec::ToolSpec;

pub struct ApplyPatchHandler {
    spec: ToolSpec,
}

impl Default for ApplyPatchHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ApplyPatchHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "apply_patch".into(),
                description: "Apply a unified diff patch to the filesystem.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([(
                        "patch".to_string(),
                        JsonSchema::string(Some("The unified diff patch to apply")),
                    )]),
                    Some(vec!["patch".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::Mutating,
                capability_tags: vec![ToolCapabilityTag::WriteFiles],
                supports_parallel: false,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
        }
    }
}

#[async_trait]
impl ToolHandler for ApplyPatchHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let output = exec_apply_patch(&ctx.workspace_root, input)
            .await
            .map_err(|e| ToolCallError::ExecutionFailed(e.to_string()))?;

        let text = output.content.into_string();
        if output.is_error {
            Ok(ToolResult::error(
                ToolResultContent::Text(text.clone()),
                "Patch failed",
                ToolCallError::ExecutionFailed(text),
            ))
        } else {
            Ok(ToolResult::success(
                ToolResultContent::Text(text),
                "Patch applied",
            ))
        }
    }
}
