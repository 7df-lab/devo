use claw_permissions::{PermissionDecision, PermissionRequest, ResourceKind};
use crate::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::json;
use std::process::Stdio;
use tokio::process::Command;
use tracing::info;

/// Execute shell commands.
///
/// This is the most powerful built-in tool. It runs commands in a child
/// process and captures stdout/stderr. Permission checks gate execution
/// since shell commands can have arbitrary side effects.
pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command in the shell. Use this for running scripts, installing packages, \
         searching files, git operations, and any other command-line tasks."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Optional timeout in milliseconds (default: 30000)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        input: serde_json::Value,
    ) -> anyhow::Result<ToolOutput> {
        let command = input["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'command' field"))?;

        let timeout_ms = input["timeout_ms"].as_u64().unwrap_or(30_000);

        // Permission check
        let perm_request = PermissionRequest {
            tool_name: "bash".into(),
            resource: ResourceKind::ShellExec,
            description: format!("execute: {}", command),
            target: Some(command.to_string()),
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

        let shell = platform_shell();

        info!(command, shell = shell.program, "executing shell command");

        let result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), {
            let mut child = Command::new(shell.program);
            child
                .args(shell.args)
                .arg(command)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .current_dir(&ctx.cwd);
            child.output()
        })
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                let mut result_text = String::new();
                if !stdout.is_empty() {
                    result_text.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result_text.is_empty() {
                        result_text.push('\n');
                    }
                    result_text.push_str("[stderr]\n");
                    result_text.push_str(&stderr);
                }

                if result_text.is_empty() {
                    result_text = "(no output)".to_string();
                }

                if output.status.success() {
                    Ok(ToolOutput::success(result_text))
                } else {
                    let code = output.status.code().unwrap_or(-1);
                    Ok(ToolOutput::error(format!(
                        "exit code {}\n{}",
                        code, result_text
                    )))
                }
            }
            Ok(Err(e)) => Ok(ToolOutput::error(format!("failed to spawn process: {}", e))),
            Err(_) => Ok(ToolOutput::error(format!(
                "command timed out after {}ms",
                timeout_ms
            ))),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

struct ShellSpec {
    program: &'static str,
    args: &'static [&'static str],
}

fn platform_shell() -> ShellSpec {
    if cfg!(windows) {
        ShellSpec {
            program: "powershell",
            args: &["-NoProfile", "-Command"],
        }
    } else {
        ShellSpec {
            program: "bash",
            args: &["-lc"],
        }
    }
}
