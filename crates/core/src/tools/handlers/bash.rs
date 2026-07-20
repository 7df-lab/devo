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

pub struct BashHandler {
    spec: ToolSpec,
}

impl Default for BashHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl BashHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "shell_command".into(),
                description: "Executes a shell command in the selected platform shell with optional timeout and output limits. Start the command string with a brief `# ...` comment on its own line explaining the command's purpose, then put the command on the next line.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        ("command".to_string(), JsonSchema::string(Some("The command to execute. Start with a brief `# ...` purpose comment on the line above the command."))),
                        ("cmd".to_string(), JsonSchema::string(Some("Alias for command"))),
                        ("timeout".to_string(), JsonSchema::integer(Some("Optional timeout in milliseconds"))),
                        ("timeout_ms".to_string(), JsonSchema::integer(Some("Alias for timeout"))),
                        ("workdir".to_string(), JsonSchema::string(Some("The working directory to run the command in"))),
                        ("description".to_string(), JsonSchema::string(Some("Clear, concise description of what this command does"))),
                        ("shell".to_string(), JsonSchema::string(Some("Optional shell binary to launch"))),
                        ("tty".to_string(), JsonSchema::boolean(Some("Whether to allocate a TTY"))),
                        ("login".to_string(), JsonSchema::boolean(Some("Whether to use login shell semantics"))),
                        ("yield_time_ms".to_string(), JsonSchema::number(Some("Milliseconds to wait for output before yielding"))),
                        ("max_output_tokens".to_string(), JsonSchema::number(Some("Maximum output tokens to return"))),
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
impl ToolHandler for BashHandler {
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

        let timeout_ms = input["timeout"]
            .as_u64()
            .or_else(|| input["timeout_ms"].as_u64())
            .unwrap_or(default_timeout_ms());
        let workdir = input["workdir"]
            .as_str()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| ctx.workspace_root.clone());
        let description = input["description"]
            .as_str()
            .unwrap_or("shell command")
            .to_string();
        let shell_override = input["shell"].as_str().map(ToOwned::to_owned);
        let tty = input["tty"].as_bool().unwrap_or(false);
        let login = input["login"].as_bool().unwrap_or(true);
        let yield_time_ms = input["yield_time_ms"]
            .as_u64()
            .unwrap_or(default_yield_time_ms());
        let max_output_tokens = input["max_output_tokens"]
            .as_u64()
            .map(|v| v as usize)
            .unwrap_or(default_max_output_tokens());
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
                description: description.clone(),
                shell_override: shell_override.clone(),
                login,
                timeout_ms,
                max_output_tokens,
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
                description,
                shell_override,
                tty,
                login,
                timeout_ms,
                yield_time_ms,
                max_output_tokens,
                sandbox_profile: ctx.sandbox_profile.clone(),
            },
            None,
            ctx.cancel_token.clone(),
        )
        .await
        .map_err(|e| ToolCallError::ExecutionFailed(e.to_string()))?;

        let display = output.display_content;
        let text = output.content.into_string();
        let mut result = if output.is_error {
            ToolResult::error(
                ToolResultContent::Text(text.clone()),
                "Command failed",
                ToolCallError::ExecutionFailed(text),
            )
        } else {
            ToolResult::success(ToolResultContent::Text(text), "Command executed")
        };
        result.display_content = display;
        Ok(result)
    }
}
