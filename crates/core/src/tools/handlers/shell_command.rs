use std::path::PathBuf;

use async_trait::async_trait;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::shell_exec::{
    ShellExecRequest, default_max_output_tokens, default_timeout_ms, default_yield_time_ms,
    execute_shell_command,
};
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolCapabilityTag, ToolExecutionMode, ToolOutputMode, ToolSpec};
use crate::tools::client_terminal_shell::{
    ClientTerminalShellRequest, execute_with_client_terminal,
};

pub struct ShellCommandHandler {
    spec: ToolSpec,
}

impl Default for ShellCommandHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellCommandHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "shell_command".into(),
                description: "Executes a shell command with optional timeout.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "command".to_string(),
                            JsonSchema::string(Some("The command to execute")),
                        ),
                        (
                            "workdir".to_string(),
                            JsonSchema::string(Some("Working directory")),
                        ),
                        (
                            "timeout_ms".to_string(),
                            JsonSchema::integer(Some("Timeout in milliseconds")),
                        ),
                    ]),
                    Some(vec!["command".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Text,
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
impl ToolHandler for ShellCommandHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let command = input
            .get("command")
            .or_else(|| input.get("cmd"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'command' field".into()))?;

        let workdir = input
            .get("workdir")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.workspace_root.clone());

        let timeout_ms = input["timeout_ms"].as_u64().unwrap_or(default_timeout_ms());

        let login = input["login"].as_bool().unwrap_or(true);
        let terminal_workdir = if workdir.is_absolute() {
            workdir.clone()
        } else {
            ctx.workspace_root.join(&workdir)
        };

        if let Some(result) = execute_with_client_terminal(
            &ctx,
            ClientTerminalShellRequest {
                command: command.to_string(),
                workdir: terminal_workdir,
                description: "shell command".into(),
                shell_override: None,
                login,
                timeout_ms,
                max_output_tokens: default_max_output_tokens(),
            },
            progress,
        )
        .await?
        {
            return Ok(result);
        }

        let output = execute_shell_command(
            ShellExecRequest {
                command: command.to_string(),
                workdir,
                description: "shell command".into(),
                shell_override: None,
                tty: false,
                login,
                timeout_ms,
                yield_time_ms: default_yield_time_ms(),
                max_output_tokens: default_max_output_tokens(),
            },
            None,
            ctx.cancel_token.clone(),
        )
        .await
        .map_err(|e| ToolCallError::ExecutionFailed(e.to_string()))?;

        let text = output.content.into_string();
        if output.is_error {
            Ok(ToolResult::error(
                ToolResultContent::Text(text.clone()),
                "Command failed",
                ToolCallError::ExecutionFailed(text),
            ))
        } else {
            Ok(ToolResult::success(
                ToolResultContent::Text(text),
                "Command executed",
            ))
        }
    }
}
