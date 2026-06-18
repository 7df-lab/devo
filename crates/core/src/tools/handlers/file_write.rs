use std::path::PathBuf;

use async_trait::async_trait;
use diffy::PatchFormatter;
use diffy::create_patch;
use serde_json::json;
use tracing::info;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolCapabilityTag, ToolExecutionMode, ToolOutputMode, ToolSpec};

const WRITE_DESCRIPTION: &str = include_str!("../write.txt");

pub struct WriteHandler {
    spec: ToolSpec,
}

impl Default for WriteHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "write".into(),
                description: WRITE_DESCRIPTION.into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "filePath".to_string(),
                            JsonSchema::string(Some("The absolute path to the file to write")),
                        ),
                        (
                            "content".to_string(),
                            JsonSchema::string(Some("The content to write to the file")),
                        ),
                    ]),
                    Some(vec!["filePath".to_string(), "content".to_string()]),
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
impl ToolHandler for WriteHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let path_str = input["filePath"]
            .as_str()
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'filePath' field".into()))?;
        let content = input["content"]
            .as_str()
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'content' field".into()))?;

        let path = resolve_path(&ctx.workspace_root, path_str);
        info!(path = %path.display(), bytes = content.len(), "writing file");
        let previous = tokio::fs::read_to_string(&path).await.ok();

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ToolCallError::ExecutionFailed(format!("failed to create directories: {e}"))
            })?;
        }

        tokio::fs::write(&path, content)
            .await
            .map_err(|e| ToolCallError::ExecutionFailed(format!("failed to write file: {e}")))?;

        let metadata = build_write_metadata(&path, previous.as_deref(), content);
        let summary = format!("wrote {} bytes to {}", content.len(), path.display());
        let mut result = ToolResult::success(
            ToolResultContent::Mixed {
                text: Some(summary.clone()),
                json: Some(metadata),
            },
            &summary,
        );
        result.display_content = Some(summary);
        Ok(result)
    }
}

fn resolve_path(cwd: &std::path::Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() { p } else { cwd.join(p) }
}

fn build_write_metadata(
    path: &std::path::Path,
    previous: Option<&str>,
    content: &str,
) -> serde_json::Value {
    match previous {
        None => {
            let additions = content.lines().count();
            let mut added_content = String::with_capacity(content.len() + additions);
            for (index, line) in content.lines().enumerate() {
                if index > 0 {
                    added_content.push('\n');
                }
                added_content.push('+');
                added_content.push_str(line);
            }
            json!({
                "diff": format!(
                    "diff --git a/{0} b/{0}\nnew file mode 100644\n--- /dev/null\n+++ b/{0}\n@@ -0,0 +1,{1} @@\n{2}",
                    path.display(),
                    additions,
                    added_content
                ),
                "files": [{
                    "path": path.display().to_string(),
                    "kind": "add",
                    "content": content,
                    "additions": additions,
                    "deletions": 0
                }]
            })
        }
        Some(old) => {
            let patch = create_patch(old, content);
            let patch_text = PatchFormatter::new().fmt_patch(&patch).to_string();
            let additions = content.lines().count();
            let deletions = old.lines().count();
            json!({
                "diff": format!(
                    "diff --git a/{0} b/{0}\n{1}",
                    path.display(),
                    patch_text
                ),
                "files": [{
                    "path": path.display().to_string(),
                    "kind": "update",
                    "content": content,
                    "postContent": content,
                    "post_content": content,
                    "oldContent": old,
                    "preContent": old,
                    "pre_content": old,
                    "additions": additions,
                    "deletions": deletions
                }]
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn build_write_metadata_for_new_file_marks_add() {
        let metadata =
            build_write_metadata(std::path::Path::new("foo.txt"), None, "hello\nworld\n");
        assert_eq!(metadata["files"][0]["kind"], "add");
        assert_eq!(metadata["files"][0]["additions"], 2);
    }

    #[test]
    fn build_write_metadata_for_existing_file_marks_update() {
        let metadata =
            build_write_metadata(std::path::Path::new("foo.txt"), Some("old\n"), "new\n");
        assert_eq!(metadata["files"][0]["kind"], "update");
        assert_eq!(metadata["files"][0]["content"], "new\n");
        assert_eq!(metadata["files"][0]["postContent"], "new\n");
        assert_eq!(metadata["files"][0]["post_content"], "new\n");
        assert_eq!(metadata["files"][0]["oldContent"], "old\n");
        assert_eq!(metadata["files"][0]["preContent"], "old\n");
        assert_eq!(metadata["files"][0]["pre_content"], "old\n");
        assert!(
            metadata["diff"]
                .as_str()
                .unwrap_or_default()
                .contains("diff --git a/foo.txt b/foo.txt")
        );
        assert!(
            metadata["diff"]
                .as_str()
                .unwrap_or_default()
                .contains("@@ -1 +1 @@")
        );
    }
}
