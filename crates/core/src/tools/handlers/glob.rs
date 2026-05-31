use async_trait::async_trait;
use tracing::debug;

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
                name: "glob".into(),
                description: "Fast file pattern matching tool that works with any codebase size."
                    .into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "pattern".to_string(),
                            JsonSchema::string(Some("The glob pattern to match files against")),
                        ),
                        (
                            "path".to_string(),
                            JsonSchema::string(Some("The directory to search in")),
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

        let base = match input["path"].as_str() {
            Some(p) => {
                let pb = std::path::PathBuf::from(p);
                if pb.is_absolute() {
                    pb
                } else {
                    ctx.workspace_root.join(pb)
                }
            }
            None => ctx.workspace_root.clone(),
        };

        debug!(pattern, base = %base.display(), "glob search");

        let full_pattern = base.join(pattern);
        let pattern_str = full_pattern.to_string_lossy();

        let mut entries: Vec<(std::path::PathBuf, std::time::SystemTime)> = Vec::new();

        match glob::glob(&pattern_str) {
            Ok(paths) => {
                for entry in paths.flatten() {
                    let mtime = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    entries.push((entry, mtime));
                }
            }
            Err(e) => {
                return Ok(ToolResult::error(
                    ToolResultContent::Text(format!("invalid glob pattern: {e}")),
                    "Invalid pattern",
                    ToolCallError::InvalidInput(format!("invalid glob pattern: {e}")),
                ));
            }
        }

        entries.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));

        if entries.is_empty() {
            return Ok(ToolResult::success(
                ToolResultContent::Text("(no matches)".into()),
                "No matches",
            ));
        }

        let lines: Vec<String> = entries
            .iter()
            .map(|(p, _)| p.to_string_lossy().to_string())
            .collect();

        Ok(ToolResult::success(
            ToolResultContent::Text(lines.join("\n")),
            format!("{} matches", lines.len()),
        ))
    }
}
