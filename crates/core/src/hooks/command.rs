use std::process::Stdio;
use std::time::Duration;

use devo_config::CommandHookConfig;
use devo_config::HookShell;
use serde_json::Map;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::warn;

use super::HookCommandOutcome;
use super::HookCommandResult;

const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 600;

pub(super) fn command_dedup_key(command: &CommandHookConfig) -> String {
    format!(
        "{:?}\0{}\0{}",
        command.shell.unwrap_or(HookShell::Bash),
        command.command,
        command.condition.as_deref().unwrap_or_default()
    )
}

pub(super) async fn execute_command_hook(
    command: &CommandHookConfig,
    payload: &Value,
) -> HookCommandResult {
    if command.async_hook.unwrap_or(false) || command.async_rewake.unwrap_or(false) {
        return spawn_command_hook(command, payload).await;
    }

    let timeout = Duration::from_secs(command.timeout.unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS));
    let command_text = command.command.clone();
    let output = tokio::time::timeout(timeout, run_command(command, payload)).await;
    match output {
        Ok(Ok(output)) => command_result_from_output(command_text, output),
        Ok(Err(message)) => HookCommandResult {
            command: command_text,
            status: None,
            stdout: String::new(),
            stderr: message.clone(),
            outcome: HookCommandOutcome::NonBlockingError { message },
        },
        Err(_) => HookCommandResult {
            command: command_text,
            status: None,
            stdout: String::new(),
            stderr: "hook command timed out".to_string(),
            outcome: HookCommandOutcome::NonBlockingError {
                message: "hook command timed out".to_string(),
            },
        },
    }
}

async fn spawn_command_hook(command: &CommandHookConfig, payload: &Value) -> HookCommandResult {
    let command_text = command.command.clone();
    let mut child = match build_command(command).spawn() {
        Ok(child) => child,
        Err(error) => {
            let message = format!("failed to spawn hook command: {error}");
            return HookCommandResult {
                command: command_text,
                status: None,
                stdout: String::new(),
                stderr: message.clone(),
                outcome: HookCommandOutcome::NonBlockingError { message },
            };
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let input = hook_input_bytes(payload);
        tokio::spawn(async move {
            let _ = stdin.write_all(&input).await;
        });
    }
    tokio::spawn(async move {
        if let Err(error) = child.wait().await {
            warn!(error = %error, "background hook command failed");
        }
    });

    HookCommandResult {
        command: command_text,
        status: None,
        stdout: String::new(),
        stderr: String::new(),
        outcome: HookCommandOutcome::Spawned,
    }
}

async fn run_command(
    hook: &CommandHookConfig,
    payload: &Value,
) -> Result<std::process::Output, String> {
    let mut child = build_command(hook)
        .spawn()
        .map_err(|error| format!("failed to spawn hook command: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&hook_input_bytes(payload))
            .await
            .map_err(|error| format!("failed to write hook input: {error}"))?;
    }
    child
        .wait_with_output()
        .await
        .map_err(|error| format!("failed to wait for hook command: {error}"))
}

fn build_command(hook: &CommandHookConfig) -> Command {
    let mut command = match hook.shell.unwrap_or(HookShell::Bash) {
        HookShell::Bash => bash_command(&hook.command),
        HookShell::PowerShell => powershell_command(&hook.command),
    };
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);
    command
}

fn bash_command(command_text: &str) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(command_text);
        command
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut command = Command::new(shell);
        command.arg("-lc").arg(command_text);
        command
    }
}

fn powershell_command(command_text: &str) -> Command {
    let mut command = Command::new("pwsh");
    command
        .arg("-NoLogo")
        .arg("-NoProfile")
        .arg("-Command")
        .arg(command_text);
    command
}

fn hook_input_bytes(payload: &Value) -> Vec<u8> {
    let mut input = serde_json::to_vec(payload).unwrap_or_default();
    input.push(b'\n');
    input
}

fn command_result_from_output(command: String, output: std::process::Output) -> HookCommandResult {
    let status = output.status.code();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let outcome = if status == Some(2) {
        HookCommandOutcome::Blocking {
            reason: blocking_reason(&stdout, &stderr, "Blocked by hook"),
        }
    } else if let Some(reason) = json_block_reason(&stdout) {
        HookCommandOutcome::Blocking { reason }
    } else if output.status.success() {
        HookCommandOutcome::Success
    } else {
        HookCommandOutcome::NonBlockingError {
            message: if stderr.trim().is_empty() {
                format!("hook command exited with status {status:?}")
            } else {
                stderr.trim().to_string()
            },
        }
    };

    HookCommandResult {
        command,
        status,
        stdout,
        stderr,
        outcome,
    }
}

fn blocking_reason(stdout: &str, stderr: &str, fallback: &str) -> String {
    json_block_reason(stdout).unwrap_or_else(|| {
        if stderr.trim().is_empty() {
            fallback.to_string()
        } else {
            stderr.trim().to_string()
        }
    })
}

fn json_block_reason(stdout: &str) -> Option<String> {
    let value: Value = serde_json::from_str(stdout.trim()).ok()?;
    let object = value.as_object()?;
    if object.get("decision").and_then(Value::as_str) == Some("block") {
        return Some(
            object
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("Blocked by hook")
                .to_string(),
        );
    }
    if object.get("continue").and_then(Value::as_bool) == Some(false) {
        return Some(
            object
                .get("stopReason")
                .or_else(|| object.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or("Stopped by hook")
                .to_string(),
        );
    }
    hook_specific_block_reason(object)
}

fn hook_specific_block_reason(object: &Map<String, Value>) -> Option<String> {
    let hook_specific = object.get("hookSpecificOutput")?.as_object()?;
    match hook_specific.get("hookEventName").and_then(Value::as_str)? {
        "PreToolUse"
            if hook_specific
                .get("permissionDecision")
                .and_then(Value::as_str)
                == Some("deny") =>
        {
            Some(
                hook_specific
                    .get("permissionDecisionReason")
                    .or_else(|| object.get("reason"))
                    .and_then(Value::as_str)
                    .unwrap_or("Blocked by hook")
                    .to_string(),
            )
        }
        "PermissionRequest" => {
            let decision = hook_specific.get("decision")?.as_object()?;
            (decision.get("behavior").and_then(Value::as_str) == Some("deny")).then(|| {
                decision
                    .get("message")
                    .or_else(|| object.get("reason"))
                    .and_then(Value::as_str)
                    .unwrap_or("Blocked by hook")
                    .to_string()
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn json_block_reason_accepts_hook_specific_outputs() {
        let pre_tool = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"nope"}}"#;
        assert_eq!(json_block_reason(pre_tool).as_deref(), Some("nope"));

        let permission_request = r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny","message":"ask denied"}}}"#;
        assert_eq!(
            json_block_reason(permission_request).as_deref(),
            Some("ask denied")
        );
    }
}
