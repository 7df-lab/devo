use std::path::PathBuf;

use async_trait::async_trait;
use devo_tools::ClientTextFileRead;
use devo_tools::ClientTextFileWrite;
use diffy::PatchFormatter;
use diffy::create_patch;
use serde_json::json;
use tracing::debug;
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

        if let Some(client_filesystem) = ctx.client_filesystem.clone() {
            let previous = match client_filesystem
                .clone()
                .read_text_file(
                    ctx.session_id.clone(),
                    path.clone(),
                    None,
                    None,
                    ctx.cancel_token.clone(),
                )
                .await
            {
                Ok(ClientTextFileRead::Content(previous)) => Some(previous),
                Ok(ClientTextFileRead::Unsupported) => tokio::fs::read_to_string(&path).await.ok(),
                Err(error) => {
                    debug!(
                        path = %path.display(),
                        %error,
                        "failed to read previous client file before write"
                    );
                    None
                }
            };
            match client_filesystem
                .write_text_file(
                    ctx.session_id.clone(),
                    path.clone(),
                    content.to_string(),
                    ctx.cancel_token.clone(),
                )
                .await?
            {
                ClientTextFileWrite::Written => {
                    return Ok(write_tool_result(&path, previous.as_deref(), content));
                }
                ClientTextFileWrite::Unsupported => {}
            }
        }

        let previous = tokio::fs::read_to_string(&path).await.ok();

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ToolCallError::ExecutionFailed(format!("failed to create directories: {e}"))
            })?;
        }

        tokio::fs::write(&path, content)
            .await
            .map_err(|e| ToolCallError::ExecutionFailed(format!("failed to write file: {e}")))?;

        Ok(write_tool_result(&path, previous.as_deref(), content))
    }
}

fn resolve_path(cwd: &std::path::Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() { p } else { cwd.join(p) }
}

fn write_tool_result(path: &std::path::Path, previous: Option<&str>, content: &str) -> ToolResult {
    let metadata = build_write_metadata(path, previous, content);
    let summary = format!("wrote {} bytes to {}", content.len(), path.display());
    let mut result = ToolResult::success(
        ToolResultContent::Mixed {
            text: Some(summary.clone()),
            json: Some(metadata),
        },
        &summary,
    );
    result.display_content = Some(summary);
    result
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
    use std::sync::Arc;
    use std::sync::Mutex;

    use devo_tools::ClientFilesystem;
    use pretty_assertions::assert_eq;
    use tokio_util::sync::CancellationToken;

    use super::*;

    #[test]
    fn build_write_metadata_for_new_file_marks_add() {
        let metadata =
            build_write_metadata(std::path::Path::new("foo.txt"), None, "hello\nworld\n");
        assert_eq!(metadata["files"][0]["kind"], "add");
        assert_eq!(metadata["files"][0]["additions"], 2);
    }

    #[tokio::test]
    async fn client_write_runs_when_previous_client_read_fails() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("new.txt");
        let writes = Arc::new(Mutex::new(Vec::new()));
        let client_filesystem = Arc::new(ReadFailingClientFilesystem {
            writes: Arc::clone(&writes),
        });

        let result = WriteHandler::new()
            .handle(
                ToolContext {
                    tool_call_id: crate::invocation::ToolCallId("call-1".to_string()),
                    session_id: "session-1".to_string(),
                    turn_id: Some("turn-1".to_string()),
                    workspace_root: root.path().to_path_buf(),
                    budgets: crate::contracts::ToolBudgets {
                        output_limit_bytes: 32_768,
                        wall_time_limit_ms: None,
                    },
                    cancel_token: CancellationToken::new(),
                    agent_scope: crate::contracts::ToolAgentScope::Parent,
                    agent_context_mode: devo_protocol::AgentContextMode::CodingAgent,
                    collaboration_mode: devo_protocol::CollaborationMode::Build,
                    agent_coordinator: None,
                    client_filesystem: Some(client_filesystem),
                    client_terminal: None,
                    network_proxy: None,
                    network_no_proxy: None,
                },
                serde_json::json!({
                    "filePath": path.clone(),
                    "content": "hello\n"
                }),
                None,
            )
            .await
            .expect("client write succeeds");

        assert_eq!(
            writes.lock().expect("writes lock").as_slice(),
            &[(path, "hello\n".to_string())]
        );
        match result.content {
            ToolResultContent::Mixed {
                json: Some(json), ..
            } => {
                assert_eq!(json["files"][0]["kind"], "add");
            }
            other => panic!("unexpected result content: {other:?}"),
        }
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

    struct ReadFailingClientFilesystem {
        writes: Arc<Mutex<Vec<(PathBuf, String)>>>,
    }

    #[async_trait::async_trait]
    impl ClientFilesystem for ReadFailingClientFilesystem {
        async fn read_text_file(
            self: Arc<Self>,
            _session_id: String,
            _path: PathBuf,
            _line: Option<u64>,
            _limit: Option<u64>,
            _cancel_token: CancellationToken,
        ) -> Result<ClientTextFileRead, ToolCallError> {
            Err(ToolCallError::ExecutionFailed("file not found".to_string()))
        }

        async fn write_text_file(
            self: Arc<Self>,
            _session_id: String,
            path: PathBuf,
            content: String,
            _cancel_token: CancellationToken,
        ) -> Result<ClientTextFileWrite, ToolCallError> {
            self.writes
                .lock()
                .expect("writes lock")
                .push((path, content));
            Ok(ClientTextFileWrite::Written)
        }
    }
}
