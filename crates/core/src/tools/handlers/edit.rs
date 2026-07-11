//! Exact string-replacement edit tool (`edit`).

use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use devo_tools::ClientTextFileRead;
use devo_tools::ClientTextFileWrite;
use tracing::info;

use super::file_change_metadata::write_tool_result;
use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::read::is_binary_file;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolCapabilityTag, ToolExecutionMode, ToolOutputMode, ToolSpec};

const EDIT_DESCRIPTION: &str = include_str!("../edit.txt");

pub struct EditHandler {
    spec: ToolSpec,
}

impl Default for EditHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl EditHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "edit".into(),
                description: EDIT_DESCRIPTION.into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "filePath".to_string(),
                            JsonSchema::string(Some("The absolute path to the file to modify")),
                        ),
                        (
                            "oldString".to_string(),
                            JsonSchema::string(Some(
                                "The exact text to replace. Must be non-empty and unique unless replaceAll is true.",
                            )),
                        ),
                        (
                            "newString".to_string(),
                            JsonSchema::string(Some(
                                "The text to replace oldString with. May be empty to delete text.",
                            )),
                        ),
                        (
                            "replaceAll".to_string(),
                            JsonSchema::boolean(Some(
                                "Replace every occurrence of oldString. Defaults to false.",
                            )),
                        ),
                    ]),
                    Some(vec![
                        "filePath".to_string(),
                        "oldString".to_string(),
                        "newString".to_string(),
                    ]),
                    Some(/*additional_properties*/ false),
                ),
                output_mode: ToolOutputMode::Mixed,
                execution_mode: ToolExecutionMode::Mutating,
                capability_tags: vec![ToolCapabilityTag::WriteFiles],
                supports_parallel: false,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::LiveOnly,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
        }
    }
}

#[async_trait]
impl ToolHandler for EditHandler {
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
        let old_string = input["oldString"]
            .as_str()
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'oldString' field".into()))?;
        let new_string = input["newString"]
            .as_str()
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'newString' field".into()))?;
        let replace_all = input["replaceAll"].as_bool().unwrap_or(false);

        if old_string.is_empty() {
            return Ok(ToolResult::error(
                ToolResultContent::Text(
                    "oldString must be non-empty. Use the Write tool to create new files.".into(),
                ),
                "Invalid oldString",
                ToolCallError::InvalidInput("empty oldString".into()),
            ));
        }
        if old_string == new_string {
            return Ok(ToolResult::error(
                ToolResultContent::Text("oldString and newString must be different".into()),
                "No-op edit",
                ToolCallError::InvalidInput("oldString equals newString".into()),
            ));
        }

        let path = resolve_path(&ctx.workspace_root, path_str);
        info!(path = %path.display(), replace_all, "editing file");

        let previous = match read_text_file(&ctx, &path).await? {
            Some(content) => content,
            None => {
                return Ok(ToolResult::error(
                    ToolResultContent::Text(format!(
                        "File not found: {}. Use the Write tool to create new files.",
                        path.display()
                    )),
                    "File not found",
                    ToolCallError::ExecutionFailed(format!("file not found: {}", path.display())),
                ));
            }
        };

        if is_binary_file(&path).unwrap_or(false) {
            return Ok(ToolResult::error(
                ToolResultContent::Text(format!("Cannot edit binary file: {}", path.display())),
                "Binary file",
                ToolCallError::ExecutionFailed("binary file".into()),
            ));
        }

        let match_count = previous.matches(old_string).count();
        if match_count == 0 {
            return Ok(ToolResult::error(
                ToolResultContent::Text("oldString not found in content".into()),
                "No match",
                ToolCallError::ExecutionFailed("oldString not found".into()),
            ));
        }
        let content = if replace_all {
            previous.replace(old_string, new_string)
        } else {
            previous.replacen(old_string, new_string, 1)
        };

        write_text_file(&ctx, &path, &content).await?;

        let summary = if replace_all {
            format!(
                "edited {} (replaced {match_count} occurrence{})",
                path.display(),
                if match_count == 1 { "" } else { "s" }
            )
        } else {
            format!("edited {}", path.display())
        };
        Ok(write_tool_result(
            &path,
            Some(previous.as_str()),
            &content,
            summary,
        ))
    }
}

fn resolve_path(cwd: &Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() { p } else { cwd.join(p) }
}

async fn read_text_file(ctx: &ToolContext, path: &Path) -> Result<Option<String>, ToolCallError> {
    if let Some(client_filesystem) = ctx.client_filesystem.clone() {
        match client_filesystem
            .read_text_file(
                ctx.session_id.clone(),
                path.to_path_buf(),
                None,
                None,
                ctx.cancel_token.clone(),
            )
            .await
        {
            Ok(ClientTextFileRead::Content(content)) => return Ok(Some(content)),
            Ok(ClientTextFileRead::Unsupported) => {}
            Err(error) => {
                tracing::debug!(
                    path = %path.display(),
                    %error,
                    "client filesystem read failed; falling back to local fs"
                );
            }
        }
    }

    match tokio::fs::read_to_string(path).await {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ToolCallError::ExecutionFailed(format!(
            "failed to read file: {error}"
        ))),
    }
}

async fn write_text_file(
    ctx: &ToolContext,
    path: &Path,
    content: &str,
) -> Result<(), ToolCallError> {
    if let Some(client_filesystem) = ctx.client_filesystem.clone() {
        match client_filesystem
            .write_text_file(
                ctx.session_id.clone(),
                path.to_path_buf(),
                content.to_string(),
                ctx.cancel_token.clone(),
            )
            .await?
        {
            ClientTextFileWrite::Written => return Ok(()),
            ClientTextFileWrite::Unsupported => {}
        }
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            ToolCallError::ExecutionFailed(format!("failed to create directories: {e}"))
        })?;
    }
    tokio::fs::write(path, content)
        .await
        .map_err(|e| ToolCallError::ExecutionFailed(format!("failed to write file: {e}")))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pretty_assertions::assert_eq;
    use tokio_util::sync::CancellationToken;

    use super::super::file_change_metadata::file_mtime;
    use super::*;
    use crate::contracts::{ToolAgentScope, ToolBudgets, ToolTerminalStatus};
    use crate::invocation::ToolCallId;
    use devo_tools::FileReadLedger;

    fn ctx(root: &Path, ledger: Arc<FileReadLedger>) -> ToolContext {
        ToolContext {
            tool_call_id: ToolCallId("call-1".to_string()),
            session_id: "session-1".to_string(),
            turn_id: Some("turn-1".to_string()),
            workspace_root: root.to_path_buf(),
            budgets: ToolBudgets {
                output_limit_bytes: 32_768,
                wall_time_limit_ms: None,
            },
            cancel_token: CancellationToken::new(),
            agent_scope: ToolAgentScope::Parent,
            agent_context_mode: devo_protocol::AgentContextMode::CodingAgent,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            agent_coordinator: None,
            client_filesystem: None,
            client_terminal: None,
            file_read_ledger: Some(ledger),
            network_proxy: None,
            network_no_proxy: None,
        }
    }

    #[tokio::test]
    async fn edit_rejects_empty_old_string() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("a.txt");
        std::fs::write(&path, "").expect("write");
        let ledger = Arc::new(FileReadLedger::new());
        ledger.record_full_read(&path, "", file_mtime(&path));

        let result = EditHandler::new()
            .handle(
                ctx(root.path(), ledger),
                serde_json::json!({
                    "filePath": path,
                    "oldString": "",
                    "newString": "x",
                }),
                None,
            )
            .await
            .expect("handle");
        assert!(matches!(
            result.structured_status,
            ToolTerminalStatus::Failed(_)
        ));
    }

    #[tokio::test]
    async fn consecutive_edits_without_reread_succeed() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("a.txt");
        std::fs::write(&path, "one two three").expect("write");
        let ledger = Arc::new(FileReadLedger::new());
        ledger.record_full_read(&path, "one two three", file_mtime(&path));

        EditHandler::new()
            .handle(
                ctx(root.path(), Arc::clone(&ledger)),
                serde_json::json!({
                    "filePath": path,
                    "oldString": "one",
                    "newString": "1",
                }),
                None,
            )
            .await
            .expect("first edit");

        let second = EditHandler::new()
            .handle(
                ctx(root.path(), ledger),
                serde_json::json!({
                    "filePath": path,
                    "oldString": "two",
                    "newString": "2",
                }),
                None,
            )
            .await
            .expect("second edit");
        assert!(matches!(
            second.structured_status,
            ToolTerminalStatus::Completed
        ));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "1 2 three");
    }
}
