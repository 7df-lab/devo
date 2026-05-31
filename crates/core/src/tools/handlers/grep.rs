use std::path::PathBuf;

use async_trait::async_trait;
use tracing::debug;

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
                description: "Fast content search tool that works with any codebase size.".into(),
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

        let re = {
            let mut builder = regex::RegexBuilder::new(pattern_str);
            builder.case_insensitive(case_insensitive);
            match builder.build() {
                Ok(r) => r,
                Err(e) => {
                    return Ok(ToolResult::error(
                        ToolResultContent::Text(format!("invalid regex: {e}")),
                        "Invalid regex",
                        ToolCallError::InvalidInput(format!("invalid regex: {e}")),
                    ));
                }
            }
        };

        let base = match input["path"].as_str() {
            Some(p) => {
                let pb = PathBuf::from(p);
                if pb.is_absolute() {
                    pb
                } else {
                    ctx.workspace_root.join(pb)
                }
            }
            None => ctx.workspace_root.clone(),
        };

        let glob_pattern = input["glob"].as_str();
        debug!(pattern = pattern_str, base = %base.display(), "grep search");

        let files = collect_files(&base, glob_pattern);
        let mut results: Vec<String> = Vec::new();
        const MAX_RESULTS: usize = 500;

        'outer: for file in &files {
            let content = match tokio::fs::read_to_string(file).await {
                Ok(c) => c,
                Err(_) => continue,
            };
            for (lineno, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    results.push(format!(
                        "{}:{}:{}",
                        file.to_string_lossy(),
                        lineno + 1,
                        line
                    ));
                    if results.len() >= MAX_RESULTS {
                        results.push(format!("(truncated at {} matches)", MAX_RESULTS));
                        break 'outer;
                    }
                }
            }
        }

        if results.is_empty() {
            return Ok(ToolResult::success(
                ToolResultContent::Text("(no matches)".into()),
                "No matches",
            ));
        }

        Ok(ToolResult::success(
            ToolResultContent::Text(results.join("\n")),
            format!("{} matches", results.len()),
        ))
    }
}

fn collect_files(base: &std::path::Path, glob_pattern: Option<&str>) -> Vec<PathBuf> {
    let pattern = match glob_pattern {
        Some(g) => base.join("**").join(g).to_string_lossy().to_string(),
        None => base.join("**").join("*").to_string_lossy().to_string(),
    };

    glob::glob(&pattern)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|p| p.is_file())
        .collect()
}
