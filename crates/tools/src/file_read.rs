use std::path::PathBuf;

use claw_permissions::{PermissionDecision, PermissionRequest, ResourceKind};
use crate::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::json;
use tracing::debug;

/// Read file contents, optionally with line range.
pub struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. You can optionally specify a line offset and limit \
         to read only a portion of the file."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to read (absolute or relative to cwd)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Starting line number (1-based)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Number of lines to read"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        input: serde_json::Value,
    ) -> anyhow::Result<ToolOutput> {
        let path_str = input["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path' field"))?;

        let path = resolve_path(&ctx.cwd, path_str);
        let offset = input["offset"].as_u64().map(|v| v as usize);
        let limit = input["limit"].as_u64().map(|v| v as usize);

        let perm_request = PermissionRequest {
            tool_name: "file_read".into(),
            resource: ResourceKind::FileRead,
            description: format!("read file: {}", path.display()),
            target: Some(path.to_string_lossy().to_string()),
        };

        match ctx.permissions.check(&perm_request).await {
            PermissionDecision::Allow => {}
            PermissionDecision::Deny { reason } => {
                return Ok(ToolOutput::error(format!("permission denied: {}", reason)));
            }
            PermissionDecision::Ask { message } => {
                return Ok(ToolOutput::error(format!("permission required — run with --permission interactive to approve: {}", message)));
            }
        }

        debug!(path = %path.display(), "reading file");

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::error(format!("failed to read file: {}", e))),
        };

        let lines: Vec<&str> = content.lines().collect();
        let start = offset.unwrap_or(1).saturating_sub(1);
        let end = limit
            .map(|l| (start + l).min(lines.len()))
            .unwrap_or(lines.len());

        let selected: Vec<String> = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:6}|{}", start + i + 1, line))
            .collect();

        Ok(ToolOutput::success(selected.join("\n")))
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

fn resolve_path(cwd: &std::path::Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        cwd.join(p)
    }
}
