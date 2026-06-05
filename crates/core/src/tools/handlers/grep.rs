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
use crate::tool_spec::{ToolCapabilityTag, ToolExecutionMode, ToolOutputMode, ToolSpec};

pub struct GrepHandler {
    spec: ToolSpec,
}

impl Default for GrepHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl GrepHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "grep".into(),
                description: "Fast exact text and regex content search backed by ripgrep. Use grep for known strings or regexes. When code_search is available, prefer it for codebase investigation.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "pattern".to_string(),
                            JsonSchema::string(Some(
                                "The regex pattern to search for in file contents",
                            )),
                        ),
                        (
                            "path".to_string(),
                            JsonSchema::string(Some("The directory to search in")),
                        ),
                        (
                            "include".to_string(),
                            JsonSchema::string(Some("File pattern to include in the search")),
                        ),
                        (
                            "case_insensitive".to_string(),
                            JsonSchema::boolean(Some("Search without case sensitivity")),
                        ),
                    ]),
                    Some(vec!["pattern".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![ToolCapabilityTag::SearchWorkspace],
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
impl ToolHandler for GrepHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let pattern_str = input["pattern"]
            .as_str()
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'pattern' field".into()))?;

        let case_insensitive = input["case_insensitive"].as_bool().unwrap_or(false);
        let path = input["path"].as_str().unwrap_or(".");
        let include = input["include"]
            .as_str()
            .or_else(|| input["glob"].as_str())
            .filter(|value| !value.is_empty());
        debug!(pattern = pattern_str, path, include, "grep search");

        let mut args = vec![
            OsString::from("--line-number"),
            OsString::from("--with-filename"),
            OsString::from("--no-heading"),
            OsString::from("--color"),
            OsString::from("never"),
        ];
        if case_insensitive {
            args.push(OsString::from("--ignore-case"));
        }
        if let Some(include) = include {
            args.push(OsString::from("--glob"));
            args.push(OsString::from(include));
        }
        args.push(OsString::from("--"));
        args.push(OsString::from(pattern_str));
        args.push(OsString::from(path));

        let output = run_rg(&ctx, args).await?;
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
            let error = if message.to_ascii_lowercase().contains("regex parse error") {
                ToolCallError::InvalidInput(message.clone())
            } else {
                ToolCallError::ExecutionFailed(message.clone())
            };
            return Ok(ToolResult::error(
                ToolResultContent::Text(message),
                "Grep failed",
                error,
            ));
        }

        const MAX_RESULTS: usize = 500;
        let text = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = text.lines().collect();
        if lines.is_empty() {
            return Ok(ToolResult::success(
                ToolResultContent::Text("(no matches)".into()),
                "No matches",
            ));
        }
        let truncated = lines.len() > MAX_RESULTS;
        let mut displayed = lines.iter().take(MAX_RESULTS).copied().collect::<Vec<_>>();
        if truncated {
            displayed.push("(truncated at 500 matches)");
        }
        let summary = if truncated {
            "500+ matches".to_string()
        } else {
            format!("{} matches", lines.len())
        };
        Ok(ToolResult::success(
            ToolResultContent::Text(displayed.join("\n")),
            summary,
        ))
    }
}
