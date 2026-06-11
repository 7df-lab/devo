use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::apply_patch::exec_apply_patch;
use crate::contracts::ToolCallError;
use crate::contracts::ToolContext;
use crate::contracts::ToolProgressSender;
use crate::contracts::ToolResult;
use crate::contracts::ToolResultContent;
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolCapabilityTag;
use crate::tool_spec::ToolExecutionMode;
use crate::tool_spec::ToolOutputMode;
use crate::tool_spec::ToolSpec;
use crate::unified_exec::ExecCommandArgs;
use crate::unified_exec::ProcessOutput;
use crate::unified_exec::WARNING_PROCESSES;
use crate::unified_exec::WriteStdinArgs;
use crate::unified_exec::process::UnifiedExecProcess;
use crate::unified_exec::process::collect_output;
use crate::unified_exec::store::ProcessStore;

#[allow(dead_code)]
const MAX_EXEC_OUTPUT_DELTAS_PER_CALL: usize = 10_000;
#[allow(dead_code)]
const UNIFIED_EXEC_OUTPUT_DELTA_MAX_BYTES: usize = 8_192;

pub struct ExecCommandHandler {
    store: Arc<ProcessStore>,
    spec: ToolSpec,
}

impl ExecCommandHandler {
    pub fn new(store: Arc<ProcessStore>) -> Self {
        Self {
            store,
            spec: ToolSpec {
                name: "exec_command".into(),
                description: "Execute a command with PTY support and process management.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "cmd".to_string(),
                            JsonSchema::string(Some("The command to execute")),
                        ),
                        (
                            "workdir".to_string(),
                            JsonSchema::string(Some("Working directory")),
                        ),
                        (
                            "shell".to_string(),
                            JsonSchema::string(Some("Shell override")),
                        ),
                        (
                            "login".to_string(),
                            JsonSchema::boolean(Some("Whether to use login shell")),
                        ),
                        (
                            "tty".to_string(),
                            JsonSchema::boolean(Some("Whether to use PTY")),
                        ),
                    ]),
                    Some(vec!["cmd".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Mixed,
                execution_mode: ToolExecutionMode::Mutating,
                capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
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
impl ToolHandler for ExecCommandHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let args = ExecCommandArgs {
            cmd: input
                .get("cmd")
                .or_else(|| input.get("command"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolCallError::InvalidInput("missing 'cmd' field".into()))?
                .to_string(),
            workdir: input["workdir"].as_str().map(|s| s.to_string()),
            shell: input["shell"].as_str().map(|s| s.to_string()),
            login: input["login"].as_bool().unwrap_or(true),
            tty: input["tty"].as_bool().unwrap_or(false),
            yield_time_ms: input["yield_time_ms"]
                .as_u64()
                .unwrap_or(crate::unified_exec::DEFAULT_YIELD_MS),
            max_output_tokens: input["max_output_tokens"]
                .as_u64()
                .map(|v| v as usize)
                .unwrap_or(crate::unified_exec::MAX_OUTPUT_TOKENS),
        };

        let cwd = input["workdir"]
            .as_str()
            .map(|path| {
                let path = std::path::PathBuf::from(path);
                if path.is_absolute() {
                    path
                } else {
                    ctx.workspace_root.join(path)
                }
            })
            .unwrap_or_else(|| ctx.workspace_root.clone());

        if !cwd.exists() {
            return Ok(ToolResult::error(
                ToolResultContent::Text(format!(
                    "working directory does not exist: {}",
                    cwd.display()
                )),
                "Invalid workdir",
                ToolCallError::ExecutionFailed(format!(
                    "working directory does not exist: {}",
                    cwd.display()
                )),
            ));
        }

        if is_raw_apply_patch_body(&args.cmd) {
            return Ok(ToolResult::error(
                ToolResultContent::Text("apply_patch verification failed: patch detected without explicit call to apply_patch.".into()),
                "Invalid command",
                ToolCallError::InvalidInput("apply_patch verification failed".into()),
            ));
        }

        if let Some((patch_cwd, patch_text)) = apply_patch_command(&args.cmd, &cwd) {
            let output = exec_apply_patch(
                &patch_cwd,
                // &ctx.session_id.to_string(),
                serde_json::json!({ "patchText": patch_text }),
            )
            .await
            .map_err(|e| ToolCallError::ExecutionFailed(e.to_string()))?;
            let content = format_apply_patch_intercept_response(
                output.content.text_part().unwrap_or_default(),
            );
            return if output.is_error {
                Ok(ToolResult::error(
                    ToolResultContent::Text(content.clone()),
                    "Patch failed",
                    ToolCallError::ExecutionFailed(content),
                ))
            } else {
                Ok(ToolResult::success(
                    ToolResultContent::Text(content),
                    "Patch applied",
                ))
            };
        }

        let Some(session_id) = self.store.reserve_process_id().await else {
            return Ok(ToolResult::error(
                ToolResultContent::Text(format!(
                    "max unified exec processes ({}) reached; cannot allocate process",
                    crate::unified_exec::MAX_PROCESSES
                )),
                "Process limit reached",
                ToolCallError::ExecutionFailed(format!(
                    "max unified exec processes ({}) reached",
                    crate::unified_exec::MAX_PROCESSES
                )),
            ));
        };

        let spawned_process = UnifiedExecProcess::spawn(
            session_id,
            &args.cmd,
            &cwd,
            args.shell.as_deref(),
            args.login,
            args.tty,
        )
        .await;
        let (proc, _broadcast_rx) = match spawned_process {
            Ok(spawned) => spawned,
            Err(error) => {
                self.store.release_reserved(session_id).await;
                return Err(ToolCallError::ExecutionFailed(format!(
                    "failed to spawn process: {error}"
                )));
            }
        };

        let proc = Arc::new(proc);
        self.store
            .insert_reserved(session_id, Arc::clone(&proc))
            .await;

        let cancel_token = ctx.cancel_token.clone();
        let store_for_cancel = Arc::clone(&self.store);
        let proc_for_cancel = Arc::clone(&proc);
        let cancel_task = tokio::spawn(async move {
            cancel_token.cancelled().await;
            proc_for_cancel.terminate();
            store_for_cancel.remove(session_id).await;
        });

        let mut rx = proc.subscribe();
        let output = tokio::select! {
            output = collect_output(
                &mut rx,
                &proc,
                crate::unified_exec::clamp_exec_yield_time(args.yield_time_ms),
                args.max_output_tokens,
            ) => output,
            _ = ctx.cancel_token.cancelled() => {
                proc.terminate();
                self.store.remove(session_id).await;
                cancel_task.abort();
                return Err(ToolCallError::Cancelled);
            }
        };
        cancel_task.abort();
        let warning = if output.exit_code.is_some() {
            self.store.remove(session_id).await;
            None
        } else {
            let process_count = self.store.len().await;
            (process_count >= WARNING_PROCESSES).then(|| open_process_warning(process_count))
        };

        let response = format_exec_response(
            &output,
            Some(session_id),
            Some(generate_chunk_id()),
            warning.as_deref(),
        );
        Ok(ToolResult::success(
            ToolResultContent::Text(response),
            "Command executed",
        ))
    }
}

pub struct WriteStdinHandler {
    store: Arc<ProcessStore>,
    spec: ToolSpec,
}

impl WriteStdinHandler {
    pub fn new(store: Arc<ProcessStore>) -> Self {
        Self {
            store,
            spec: ToolSpec {
                name: "write_stdin".into(),
                description: "Write to stdin of a running process.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "session_id".to_string(),
                            JsonSchema::integer(Some("Process session ID")),
                        ),
                        (
                            "chars".to_string(),
                            JsonSchema::string(Some("Characters to write to stdin")),
                        ),
                    ]),
                    Some(vec!["session_id".to_string(), "chars".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Mixed,
                execution_mode: ToolExecutionMode::Mutating,
                capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
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
impl ToolHandler for WriteStdinHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        _ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let args = WriteStdinArgs {
            session_id: input["session_id"]
                .as_i64()
                .ok_or_else(|| ToolCallError::InvalidInput("missing 'session_id' field".into()))?
                as i32,
            chars: input["chars"].as_str().unwrap_or("").to_string(),
            yield_time_ms: input["yield_time_ms"]
                .as_u64()
                .unwrap_or(crate::unified_exec::DEFAULT_POLL_YIELD_MS),
            max_output_tokens: input["max_output_tokens"]
                .as_u64()
                .map(|v| v as usize)
                .unwrap_or(crate::unified_exec::MAX_OUTPUT_TOKENS),
        };

        let proc = self.store.get(args.session_id).await.ok_or_else(|| {
            ToolCallError::ExecutionFailed(format!("Unknown process id {}", args.session_id))
        })?;

        if !args.chars.is_empty() {
            if !proc.tty() {
                return Err(ToolCallError::ExecutionFailed(
                    "stdin is closed for this session".to_string(),
                ));
            }
            if let Err(error) = proc.write_stdin(&args.chars)
                && proc.is_running()
                && proc.exit_code().is_none()
            {
                return Err(ToolCallError::ExecutionFailed(format!(
                    "write_stdin failed: {error}"
                )));
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        let mut rx = proc.subscribe();
        let output = collect_output(
            &mut rx,
            &proc,
            crate::unified_exec::clamp_write_stdin_yield_time(args.yield_time_ms, &args.chars),
            args.max_output_tokens,
        )
        .await;

        if output.exit_code.is_some() {
            self.store.remove(args.session_id).await;
        }

        let response = format_exec_response(
            &output,
            Some(args.session_id),
            Some(generate_chunk_id()),
            /*warning*/ None,
        );
        Ok(ToolResult::success(
            ToolResultContent::Text(response),
            "Input written",
        ))
    }
}

fn format_exec_response(
    output: &ProcessOutput,
    session_id: Option<i32>,
    chunk_id: Option<String>,
    warning: Option<&str>,
) -> String {
    let mut parts = Vec::new();

    if let Some(chunk_id) = chunk_id
        && !chunk_id.is_empty()
    {
        parts.push(format!("Chunk ID: {chunk_id}"));
    }

    parts.push(format!("Wall time: {:.4} seconds", output.wall_time_secs));

    if let Some(code) = output.exit_code {
        parts.push(format!("Process exited with code {code}"));
    }
    if let Some(sid) = session_id
        && output.exit_code.is_none()
    {
        parts.push(format!("Process running with session ID {sid}"));
    }
    if let Some(warning) = warning {
        parts.push(warning.to_string());
    }

    parts.push(format!(
        "Original token count: {}",
        output.original_token_count
    ));
    parts.push("Output:".to_string());
    parts.push(output.output.clone());

    parts.join("\n")
}

fn generate_chunk_id() -> String {
    Uuid::new_v4().to_string().chars().take(6).collect()
}

fn open_process_warning(process_count: usize) -> String {
    format!(
        "Warning: The maximum number of unified exec processes you can keep open is {WARNING_PROCESSES} and you currently have {process_count} processes open. Reuse older processes or close them to prevent automatic pruning of old processes"
    )
}

fn format_apply_patch_intercept_response(content: &str) -> String {
    format!("Wall time: 0.0000 seconds\nOutput:\n{content}")
}

#[allow(dead_code)]
fn progress_delta_chunks(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut chunks = Vec::new();
    let mut remaining = text.as_ref();
    while !remaining.is_empty() {
        let take = floor_char_boundary(
            remaining,
            remaining.len().min(UNIFIED_EXEC_OUTPUT_DELTA_MAX_BYTES),
        );
        let take = if take == 0 {
            remaining
                .char_indices()
                .nth(1)
                .map_or(remaining.len(), |(index, _)| index)
        } else {
            take
        };
        chunks.push(remaining[..take].to_string());
        remaining = &remaining[take..];
    }
    chunks
}

#[allow(dead_code)]
fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn is_raw_apply_patch_body(command: &str) -> bool {
    let trimmed = command.trim();
    trimmed.starts_with("*** Begin Patch") && trimmed.contains("*** End Patch")
}

fn apply_patch_command(
    command: &str,
    cwd: &std::path::Path,
) -> Option<(std::path::PathBuf, String)> {
    let trimmed = command.trim();
    if let Some(argv) = shlex::split(trimmed)
        && let [cmd, patch_text] = argv.as_slice()
        && (cmd == "apply_patch" || cmd == "applypatch")
    {
        return Some((cwd.to_path_buf(), patch_text.clone()));
    }

    let (effective_cwd, script) = if let Some((cd_command, rest)) = trimmed.split_once("&&") {
        let argv = shlex::split(cd_command.trim())?;
        match argv.as_slice() {
            [cmd, dir] if cmd == "cd" => {
                let path = std::path::PathBuf::from(dir);
                let path = if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                };
                (path, rest.trim())
            }
            _ => (cwd.to_path_buf(), trimmed),
        }
    } else {
        (cwd.to_path_buf(), trimmed)
    };

    let mut lines = script.lines();
    let first_line = lines.next()?.trim();
    let command_name = first_line.split_whitespace().next()?;
    if command_name != "apply_patch" && command_name != "applypatch" {
        return None;
    }
    let heredoc_index = first_line.find("<<")?;
    let delimiter = first_line[heredoc_index + 2..].trim();
    let delimiter = delimiter
        .strip_prefix('-')
        .unwrap_or(delimiter)
        .trim()
        .trim_matches('"')
        .trim_matches('\'');
    if delimiter.is_empty() {
        return None;
    }

    let mut patch_lines = Vec::new();
    while let Some(line) = lines.next() {
        if line.trim() == delimiter {
            if lines.any(|remaining| !remaining.trim().is_empty()) {
                return None;
            }
            return Some((effective_cwd, patch_lines.join("\n")));
        }
        patch_lines.push(line);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use devo_tools::contracts::ToolBudgets;
    use pretty_assertions::assert_eq;
    use tokio_util::sync::CancellationToken;

    fn test_ctx(cwd: std::path::PathBuf) -> crate::contracts::ToolContext {
        crate::contracts::ToolContext {
            tool_call_id: crate::invocation::ToolCallId("test".into()),
            session_id: "test-session".into(),
            turn_id: Some("test-turn".into()),
            workspace_root: cwd,
            // permission_profile: crate::contracts::ToolPermissionProfile {
            //     can_read_workspace: true,
            //     can_write_workspace: true,
            //     can_execute_commands: true,
            //     network_enabled: true,
            // },
            // tool_registry: std::sync::Arc::new(crate::contracts::NoopToolRegistry),
            budgets: ToolBudgets {
                wall_time_limit_ms: Some(6_000),
                output_limit_bytes: 32 * 1024,
            },
            cancel_token: CancellationToken::new(),
            agent_scope: crate::contracts::ToolAgentScope::Parent,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            agent_coordinator: None,
            network_proxy: None,
        }
    }

    #[test]
    fn format_exec_response_exited() {
        let output = ProcessOutput {
            output: "hello world".into(),
            exit_code: Some(0),
            wall_time_secs: 1.5,
            truncated: false,
            original_token_count: 3,
        };
        let text = format_exec_response(&output, None, None, /*warning*/ None);
        assert!(text.contains("Wall time: 1.5000"));
        assert!(text.contains("Process exited with code 0"));
        assert!(text.contains("hello world"));
        assert!(!text.contains("session ID"));
        assert!(text.contains("Original token count: 3"));
    }

    #[test]
    fn format_exec_response_running() {
        let output = ProcessOutput {
            output: "building...".into(),
            exit_code: None,
            wall_time_secs: 10.0,
            truncated: false,
            original_token_count: 3,
        };
        let text = format_exec_response(&output, Some(42), None, /*warning*/ None);
        assert!(text.contains("Process running with session ID 42"));
        assert!(!text.contains("exit code"));
    }

    #[test]
    fn format_exec_response_truncated() {
        let output = ProcessOutput {
            output: "long output...".into(),
            exit_code: None,
            wall_time_secs: 5.0,
            truncated: true,
            original_token_count: 3,
        };
        let text = format_exec_response(&output, Some(1), None, /*warning*/ None);
        assert!(text.contains("Output:"));
    }

    #[test]
    fn format_exec_response_with_both_exit_and_session() {
        let output = ProcessOutput {
            output: "done".into(),
            exit_code: Some(0),
            wall_time_secs: 3.0,
            truncated: false,
            original_token_count: 1,
        };
        // When exit_code is Some, session_id is not shown even if provided
        let text = format_exec_response(&output, Some(99), None, /*warning*/ None);
        assert!(text.contains("Process exited with code 0"));
        assert!(!text.contains("session ID"));
    }

    #[test]
    fn format_exec_response_includes_open_process_warning() {
        let output = ProcessOutput {
            output: "building...".into(),
            exit_code: None,
            wall_time_secs: 10.0,
            truncated: false,
            original_token_count: 3,
        };

        let text = format_exec_response(
            &output,
            Some(42),
            None,
            Some(&open_process_warning(WARNING_PROCESSES)),
        );

        assert!(text.contains("currently have 60 processes open"));
        assert!(text.contains("Reuse older processes"));
    }

    #[test]
    fn progress_delta_chunks_caps_chunk_size_on_utf8_boundary() {
        let text = "a".repeat(UNIFIED_EXEC_OUTPUT_DELTA_MAX_BYTES - 1) + "😀tail";

        let chunks = progress_delta_chunks(text.as_bytes());

        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].len() <= UNIFIED_EXEC_OUTPUT_DELTA_MAX_BYTES);
        assert_eq!(chunks.join(""), text);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_command_streams_progress_before_final_output() {
        let root = std::env::temp_dir().join(format!("devo-exec-stream-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp test dir");
        let handler = ExecCommandHandler::new(Arc::new(ProcessStore::new()));

        let output = handler
            .handle(
                test_ctx(root.clone()),
                serde_json::json!({
                    "cmd": "printf 'first\\n'; sleep 0.05; printf 'second\\n'",
                    "login": false,
                    "yield_time_ms": 1000,
                    "max_output_tokens": 1000,
                }),
                None,
            )
            .await
            .expect("handle exec command");

        let text = match &output.content {
            crate::contracts::ToolResultContent::Text(t) => t.clone(),
            other => format!("{other:?}"),
        };
        assert!(
            text.contains("first"),
            "output should contain initial output: {text:?}"
        );
        assert!(
            text.contains("second"),
            "output should contain final output: {text:?}"
        );
        std::fs::remove_dir_all(root).expect("cleanup temp test dir");
    }

    #[test]
    fn exec_command_args_missing_cmd() {
        let args = serde_json::json!({});
        let result = serde_json::from_value::<serde_json::Value>(args);
        assert!(result.is_ok());
        // The cmd field is required but we can't easily test parse failure
        // because there's no deserialize impl for ExecCommandArgs
    }

    #[test]
    fn apply_patch_command_extracts_heredoc() {
        let command = "apply_patch <<'PATCH'\n*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch\nPATCH\n";

        let parsed = apply_patch_command(command, std::path::Path::new("/tmp/root"));

        assert_eq!(
            parsed,
            Some((
                std::path::PathBuf::from("/tmp/root"),
                "*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch".to_string()
            ))
        );
    }

    #[test]
    fn apply_patch_command_extracts_cd_heredoc() {
        let command = "cd sub && apply_patch <<EOF\n*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch\nEOF";

        let parsed = apply_patch_command(command, std::path::Path::new("/tmp/root"));

        assert_eq!(
            parsed,
            Some((
                std::path::PathBuf::from("/tmp/root/sub"),
                "*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch".to_string()
            ))
        );
    }

    #[test]
    fn apply_patch_command_extracts_direct_body() {
        let command =
            "apply_patch '*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch'";

        let parsed = apply_patch_command(command, std::path::Path::new("/tmp/root"));

        assert_eq!(
            parsed,
            Some((
                std::path::PathBuf::from("/tmp/root"),
                "*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch".to_string()
            ))
        );
    }

    #[test]
    fn apply_patch_command_rejects_trailing_commands_after_heredoc() {
        let command = "apply_patch <<'PATCH'\n*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch\nPATCH\necho done";

        assert_eq!(
            apply_patch_command(command, std::path::Path::new("/tmp/root")),
            None
        );
    }

    #[tokio::test]
    async fn exec_command_rejects_raw_apply_patch_body() {
        let root = std::env::temp_dir().join(format!("devo-apply-patch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp test dir");
        let handler = ExecCommandHandler::new(Arc::new(ProcessStore::new()));
        let command = "*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch\n";

        let output = handler
            .handle(
                test_ctx(root.clone()),
                serde_json::json!({ "cmd": command }),
                None,
            )
            .await
            .expect("handle exec command");

        assert!(matches!(
            output.structured_status,
            crate::contracts::ToolTerminalStatus::Failed(_)
        ));
        let text = match &output.content {
            crate::contracts::ToolResultContent::Text(t) => t.as_str(),
            _ => "",
        };
        assert!(text.contains("patch detected without explicit call to apply_patch"));
        assert!(!root.join("file.txt").exists());
        std::fs::remove_dir_all(root).expect("cleanup temp test dir");
    }

    #[tokio::test]
    async fn exec_command_intercepts_apply_patch_heredoc() {
        let root = std::env::temp_dir().join(format!("devo-apply-patch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp test dir");
        let handler = ExecCommandHandler::new(Arc::new(ProcessStore::new()));
        let command = "apply_patch <<'PATCH'\n*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch\nPATCH\n";

        let output = handler
            .handle(
                test_ctx(root.clone()),
                serde_json::json!({ "cmd": command }),
                None,
            )
            .await
            .expect("handle exec command");

        let text = match &output.content {
            crate::contracts::ToolResultContent::Text(t) => t.clone(),
            other => format!("{other:?}"),
        };
        assert!(text.starts_with("Wall time: 0.0000 seconds\nOutput:\n"));
        assert!(text.contains("Success. Updated the following files:"));
        assert!(!text.contains("\"diagnostics\""));
        assert_eq!(
            std::fs::read_to_string(root.join("file.txt")).expect("read patched file"),
            "hello\n"
        );
        std::fs::remove_dir_all(root).expect("cleanup temp test dir");
    }

    #[tokio::test]
    async fn exec_command_intercepts_apply_patch_after_cd() {
        let root = std::env::temp_dir().join(format!("devo-apply-patch-{}", Uuid::new_v4()));
        let subdir = root.join("sub");
        std::fs::create_dir_all(&subdir).expect("create temp test dir");
        let handler = ExecCommandHandler::new(Arc::new(ProcessStore::new()));
        let command = "cd sub && apply_patch <<'PATCH'\n*** Begin Patch\n*** Add File: nested.txt\n+hello\n*** End Patch\nPATCH\n";

        let output = handler
            .handle(
                test_ctx(root.clone()),
                serde_json::json!({ "cmd": command }),
                None,
            )
            .await
            .expect("handle exec command");

        let text = match &output.content {
            crate::contracts::ToolResultContent::Text(t) => t.clone(),
            other => format!("{other:?}"),
        };
        assert!(text.starts_with("Wall time: 0.0000 seconds\nOutput:\n"));
        assert!(text.contains("Success. Updated the following files:"));
        assert!(!text.contains("\"diagnostics\""));
        assert_eq!(
            std::fs::read_to_string(subdir.join("nested.txt")).expect("read patched file"),
            "hello\n"
        );
        std::fs::remove_dir_all(root).expect("cleanup temp test dir");
    }
}
