use std::path::PathBuf;

use claw_permissions::{PermissionDecision, PermissionRequest, ResourceKind};
use crate::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::json;
use tracing::info;

/// Perform an exact string replacement in a file.
///
/// Mirrors Claude Code's FileEditTool: replaces the first occurrence of
/// `old_string` with `new_string`. The replacement must be unique in the file
/// to avoid ambiguous edits.
pub struct FileEditTool;

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &str {
        "file_edit"
    }

    fn description(&self) -> &str {
        "Replace an exact string in a file. `old_string` must appear exactly once \
         in the file. Use `file_read` first to confirm the exact text to replace. \
         Prefer this over `file_write` for targeted edits."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path to edit (absolute or relative to cwd)"
                },
                "old_string": {
                    "type": "string",
                    "description": "Exact string to find and replace (must be unique in the file)"
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement string"
                }
            },
            "required": ["path", "old_string", "new_string"]
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
        let old_string = input["old_string"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'old_string' field"))?;
        let new_string = input["new_string"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'new_string' field"))?;

        let path = resolve_path(&ctx.cwd, path_str);

        let perm_request = PermissionRequest {
            tool_name: "file_edit".into(),
            resource: ResourceKind::FileWrite,
            description: format!("edit file: {}", path.display()),
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

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::error(format!("failed to read file: {}", e))),
        };

        let count = content.matches(old_string).count();
        if count == 0 {
            return Ok(ToolOutput::error(
                "old_string not found in file".to_string(),
            ));
        }
        if count > 1 {
            return Ok(ToolOutput::error(format!(
                "old_string appears {} times — provide more context to make it unique",
                count
            )));
        }

        let new_content = content.replacen(old_string, new_string, 1);

        info!(path = %path.display(), "editing file");

        match tokio::fs::write(&path, &new_content).await {
            Ok(_) => Ok(ToolOutput::success(format!(
                "edited {}",
                path.display()
            ))),
            Err(e) => Ok(ToolOutput::error(format!("failed to write file: {}", e))),
        }
    }

    fn is_read_only(&self) -> bool {
        false
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
