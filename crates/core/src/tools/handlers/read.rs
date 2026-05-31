use std::path::PathBuf;

use async_trait::async_trait;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::read::{is_binary_file, missing_file_message, read_directory, read_file};
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolSpec};

pub struct ReadHandler {
    spec: ToolSpec,
}

impl Default for ReadHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "read".into(),
                description: "Read a file or directory from the local filesystem.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "filePath".to_string(),
                            JsonSchema::string(Some(
                                "The absolute path to the file or directory to read",
                            )),
                        ),
                        (
                            "offset".to_string(),
                            JsonSchema::integer(Some(
                                "Line number to start reading from (1-indexed)",
                            )),
                        ),
                        (
                            "limit".to_string(),
                            JsonSchema::integer(Some("Maximum number of lines to read")),
                        ),
                    ]),
                    Some(vec!["filePath".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![crate::tool_spec::ToolCapabilityTag::ReadFiles],
                supports_parallel: true,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
        }
    }
}

#[async_trait]
impl ToolHandler for ReadHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let mut filepath = input["filePath"]
            .as_str()
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'filePath' field".into()))?
            .to_string();
        let offset = input["offset"].as_u64().map(|v| v as usize);
        let limit = input["limit"].as_u64().map(|v| v as usize);

        if let Some(offset) = offset
            && offset < 1
        {
            return Ok(ToolResult::error(
                ToolResultContent::Text("offset must be greater than or equal to 1".into()),
                "Invalid offset",
                ToolCallError::InvalidInput("offset must be >= 1".into()),
            ));
        }

        if !PathBuf::from(&filepath).is_absolute() {
            filepath = ctx
                .workspace_root
                .join(&filepath)
                .to_string_lossy()
                .to_string();
        }

        let path = PathBuf::from(&filepath);
        if !path.exists() {
            return Ok(ToolResult::error(
                ToolResultContent::Text(missing_file_message(&filepath)),
                "File not found",
                ToolCallError::ExecutionFailed(format!("file not found: {filepath}")),
            ));
        }

        if path.is_dir() {
            let output = read_directory(&path, limit.unwrap_or(usize::MAX), offset.unwrap_or(1));
            let output = output.map_err(|e| ToolCallError::ExecutionFailed(format!("{e}")))?;
            let text = output.content.into_string();
            let display = output.display_content.clone();
            let mut result = ToolResult::success(ToolResultContent::Text(text), "Directory read");
            result.display_content = display;
            return Ok(result);
        }

        let is_bin =
            is_binary_file(&path).map_err(|e| ToolCallError::ExecutionFailed(format!("{e}")))?;
        if is_bin {
            return Ok(ToolResult::error(
                ToolResultContent::Text(format!("Cannot read binary file: {}", path.display())),
                "Binary file",
                ToolCallError::ExecutionFailed("binary file".into()),
            ));
        }

        let output = read_file(&path, limit.unwrap_or(usize::MAX), offset.unwrap_or(1));
        let output = output.map_err(|e| ToolCallError::ExecutionFailed(format!("{e}")))?;
        let text = output.content.into_string();
        let display = output.display_content.clone();
        let mut result = ToolResult::success(ToolResultContent::Text(text), "File read");
        result.display_content = display;
        Ok(result)
    }
}
