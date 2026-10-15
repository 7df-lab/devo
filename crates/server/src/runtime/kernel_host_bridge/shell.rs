//! Approval-mediated shell execution for the RLM kernel.
use super::*;
use devo_core::tools::{
    command_prefix_for_tool_input, generalize_command_pattern, safe_shell_argv,
    sandbox_permission_request_from_input,
};

pub(crate) async fn handle_bash(bridge: &HostBridge, params: &Value) -> Value {
    if plan_mode_denies_mutation(bridge) {
        return deny_plan_mutation("bash");
    }
    let command = params
        .get("command")
        .or_else(|| params.get("cmd"))
        .cloned()
        .unwrap_or(Value::Null);
    let command_str = command.as_str().unwrap_or("").to_string();
    if command_str.is_empty() {
        return error_reply("missing bash command");
    }
    let mut input = params.clone();
    if input.get("command").is_none() {
        input["command"] = json!(command_str.clone());
    }
    let tool_call_id = synthetic_tool_call_id("bash");
    let mut req = permission_request(
        bridge,
        tool_call_id.clone(),
        "shell_command",
        input.clone(),
        ResourceKind::ShellExec,
        format!("Run shell command: {command_str}"),
        /*path*/ None,
        Some(command_str),
    );
    req.sandbox_permissions = match sandbox_permission_request_from_input(&input) {
        Ok(permissions) => permissions,
        Err(error) => return error_reply(format!("invalid sandbox permission request: {error}")),
    };
    req.justification = input
        .get("justification")
        .and_then(Value::as_str)
        .map(str::to_string);
    req.command_prefix = command_prefix_for_tool_input("shell_command", &input);
    req.command_argv = safe_shell_argv(command.as_str().unwrap_or_default());
    req.command_pattern =
        generalize_command_pattern(command.as_str().unwrap_or_default(), &bridge.cwd);
    let grant = match check_or_grant(bridge, req).await {
        Ok(grant) => grant,
        Err(reply) => return reply,
    };
    let mut ctx = tool_context_from_bridge(bridge, tool_call_id.clone());
    if grant.bypass_sandbox {
        ctx.sandbox_profile = Some("off".to_string());
    }
    ctx.sandbox_permission_overlay = grant.sandbox_permission_overlay.clone();
    let outcome = ShellCommandHandler::new()
        .handle(ctx, input.clone(), /*_progress*/ None)
        .await;
    // UnlessTrusted retry (mirrors `ToolRuntime::execute_single`): a
    // user-approved shell call the OS sandbox refused retries once unsandboxed
    // — without this, kernel `bash` approvals visibly succeed while the
    // command still fails with EACCES. Policy-allows (no approval) keep the
    // denial so the model can explicitly request escalation through rlm.shell().
    let outcome = match outcome {
        Ok(ref output)
            if grant.already_approved
                && !grant.bypass_sandbox
                && grant.sandbox_permission_overlay.is_none()
                && tool_result_is_sandbox_denied_bridge(output)
                && devo_sandbox::unsandboxed_execution_allowed(
                    bridge.sandbox_profile.as_deref(),
                    &bridge.cwd,
                ) =>
        {
            let mut ctx = tool_context_from_bridge(bridge, tool_call_id);
            ctx.sandbox_profile = Some("off".to_string());
            ShellCommandHandler::new()
                .handle(ctx, input, /*_progress*/ None)
                .await
        }
        other => other,
    };
    map_tool_outcome(outcome)
}

/// Bridge twin of the router's `tool_result_is_sandbox_denied` (private there):
/// only the structured `SANDBOX_DENIED:` prefix triggers the retry.
fn tool_result_is_sandbox_denied_bridge(output: &ToolResult) -> bool {
    let text = match &output.content {
        devo_core::tools::contracts::ToolResultContent::Text(text) => text.as_str(),
        devo_core::tools::contracts::ToolResultContent::Mixed {
            text: Some(text), ..
        } => text.as_str(),
        devo_core::tools::contracts::ToolResultContent::Json(_)
        | devo_core::tools::contracts::ToolResultContent::Mixed { text: None, .. } => "",
    };
    text.starts_with("SANDBOX_DENIED:") || output.result_summary.starts_with("SANDBOX_DENIED:")
}

#[cfg(test)]
mod tests {
    use super::*;
    use devo_core::tools::PermissionGrant;
    use pretty_assertions::assert_eq;
    use std::sync::Mutex;

    #[tokio::test]
    async fn denied_escalation_never_executes_command() {
        let workspace = tempfile::tempdir().expect("workspace");
        let mut bridge = HostBridge::for_tests("ses_shell_denied");
        bridge.cwd = workspace.path().to_path_buf();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&seen);
        bridge.permission = PermissionChecker::new(move |request| {
            let captured = Arc::clone(&captured);
            Box::pin(async move {
                captured.lock().unwrap().push((
                    request.tool_name,
                    request.input,
                    request.justification,
                    request.sandbox_permissions,
                    request.target,
                    request.command_prefix,
                    request.command_argv,
                    request.command_pattern,
                ));
                Err("declined by user".to_string())
            })
        });
        let input = json!({
            "command": "echo denied > should-not-exist.txt",
            "sandbox_permissions": "require_escalated",
            "justification": "May I run the shell probe?"
        });
        assert_eq!(
            handle_bash(&bridge, &input).await,
            json!({"status":"error", "error":"declined by user"})
        );
        assert_eq!(
            *seen.lock().unwrap(),
            vec![(
                "shell_command".to_string(),
                input,
                Some("May I run the shell probe?".to_string()),
                SandboxPermissionRequest::FullEscalation,
                Some("echo denied > should-not-exist.txt".to_string()),
                None,
                None,
                None,
            )]
        );
        assert!(!workspace.path().join("should-not-exist.txt").exists());
    }

    #[tokio::test]
    async fn approved_escalation_runs_without_the_original_sandbox() {
        let workspace = tempfile::tempdir().expect("workspace");
        let mut bridge = HostBridge::for_tests("ses_shell_approved");
        bridge.cwd = workspace.path().to_path_buf();
        // This deliberately invalid profile would fail if the approved bypass were lost.
        bridge.sandbox_profile = Some("missing-test-profile".to_string());
        bridge.permission = PermissionChecker::new(|request| {
            Box::pin(
                async move { Ok(PermissionGrant::from_approval(&request.sandbox_permissions)) },
            )
        });
        let reply = handle_bash(
            &bridge,
            &json!({
                "command":"echo RLM_ESCALATION_OK",
                "sandbox_permissions":"require_escalated",
                "justification":"May I run a harmless echo outside the sandbox?"
            }),
        )
        .await;
        assert_eq!(reply["status"], json!("ok"), "{reply}");
        assert!(
            reply["stdout"]
                .as_str()
                .unwrap_or_default()
                .contains("RLM_ESCALATION_OK"),
            "{reply}"
        );
    }

    #[tokio::test]
    async fn invalid_permissions_and_plan_mode_never_authorize() {
        let mut bridge = HostBridge::for_tests("ses_shell_invalid");
        bridge.permission = PermissionChecker::new(|_| {
            Box::pin(async { panic!("invalid or Plan request reached authorization") })
        });
        assert_eq!(
            handle_bash(
                &bridge,
                &json!({
                    "command":"echo invalid", "sandbox_permissions":"invalid"
                })
            )
            .await,
            json!({"status":"error", "error":"invalid sandbox permission request: unsupported sandbox_permissions value: invalid"})
        );
        bridge.collaboration_mode = CollaborationMode::Plan;
        assert_eq!(
            handle_bash(
                &bridge,
                &json!({
                    "command":"echo invalid", "sandbox_permissions":"require_escalated"
                })
            )
            .await,
            json!({"status":"error", "error":"denied: bash blocked in Plan mode"})
        );
    }

    #[tokio::test]
    async fn narrow_permissions_reach_authorization_without_full_escalation() {
        let mut bridge = HostBridge::for_tests("ses_shell_narrow");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&seen);
        bridge.permission = PermissionChecker::new(move |request| {
            let captured = Arc::clone(&captured);
            Box::pin(async move {
                captured.lock().unwrap().push(request.sandbox_permissions);
                Err("test stops before execution".to_string())
            })
        });
        let input = json!({"command":"echo narrow", "sandbox_permissions":"with_additional_permissions", "additional_permissions":{"network":{"enabled":true}}});
        assert_eq!(
            handle_bash(&bridge, &input).await,
            json!({"status":"error", "error":"test stops before execution"})
        );
        assert_eq!(
            *seen.lock().unwrap(),
            vec![SandboxPermissionRequest::AdditionalPermissions(
                devo_core::tools::AdditionalSandboxPermissions {
                    network: devo_core::tools::NetworkPermission::Enabled,
                    read_paths: Vec::new(),
                    write_paths: Vec::new(),
                }
            )]
        );
    }
}
