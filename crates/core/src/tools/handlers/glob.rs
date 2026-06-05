use std::ffi::OsString;

use async_trait::async_trait;
use tracing::debug;

use super::ripgrep::RG_NO_MATCH_EXIT_CODE;
use super::ripgrep::run_rg;
use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolSpec};

pub struct GlobHandler {
    spec: ToolSpec,
}

impl Default for GlobHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "find".into(),
                description: "Fast filename and path search backed by ripgrep. Use only for literal file/path discovery. When code_search is available, prefer it for codebase investigation."
                    .into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "pattern".to_string(),
                            JsonSchema::string(Some(
                                "The ripgrep glob pattern to match file paths against",
                            )),
                        ),
                        (
                            "path".to_string(),
                            JsonSchema::string(Some(
                                "The directory to search in. Defaults to the workspace root.",
                            )),
                        ),
                    ]),
                    Some(vec!["pattern".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![crate::tool_spec::ToolCapabilityTag::SearchWorkspace],
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
impl ToolHandler for GlobHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let pattern = input["pattern"]
            .as_str()
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'pattern' field".into()))?;

        let path = input["path"].as_str().unwrap_or(".");
        debug!(pattern, path, "find search");

        let output = run_rg(
            &ctx,
            [
                OsString::from("--files"),
                OsString::from("--glob"),
                OsString::from(pattern),
                OsString::from("--"),
                OsString::from(path),
            ],
        )
        .await?;

        let exit_code = output.status.code().unwrap_or(i32::MAX);
        if exit_code == RG_NO_MATCH_EXIT_CODE {
            return Ok(ToolResult::success(
                ToolResultContent::Text("(no matches)".into()),
                "No matches",
            ));
        }
        if exit_code != 0 {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if stderr.is_empty() {
                format!("ripgrep exited with status {exit_code}")
            } else {
                stderr
            };
            return Ok(ToolResult::error(
                ToolResultContent::Text(message.clone()),
                "Find failed",
                ToolCallError::ExecutionFailed(message),
            ));
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = text.lines().collect();
        if lines.is_empty() {
            return Ok(ToolResult::success(
                ToolResultContent::Text("(no matches)".into()),
                "No matches",
            ));
        }

        Ok(ToolResult::success(
            ToolResultContent::Text(lines.join("\n")),
            format!("{} matches", lines.len()),
        ))
    }
}
