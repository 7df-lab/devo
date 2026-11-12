//! Approval-mediated shell execution for the RLM kernel.
use super::*;
use devo_core::tools::{
    command_prefix_for_tool_input, generalize_command_pattern, safe_shell_argv,
    sandbox_permission_request_from_input,
};

async fn prepare_bash(
    bridge: &HostBridge,
    params: &Value,
) -> Result<
    (
        ToolContext,
        Value,
        devo_core::tools::host_bash::SandboxRetry,
    ),
    Value,
> {
    let mode = match if params.get("mode").is_some() {
        devo_core::tools::host_bash::BashMode::parse(params)
    } else {
        Ok(devo_core::tools::host_bash::BashMode::Pipe)
    } {
        Ok(mode) => mode,
        Err(message) => return Err(error_reply(message)),
    };
    let interactive = mode != devo_core::tools::host_bash::BashMode::Pipe;
    let command = params
        .get("command")
        .or_else(|| params.get("cmd"))
        .cloned()
        .unwrap_or(Value::Null);
    let command_str = command.as_str().unwrap_or("").to_string();
    if command_str.is_empty() {
        return Err(error_reply("missing bash command"));
    }
    let mut input = params.clone();
    if input.get("command").is_none() {
        input["command"] = json!(command_str.clone());
    }
    // Resolve once so approval, command matching, and execution use the same cwd.
    let workdir = match input.get("workdir") {
        Some(Value::String(path)) => {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                bridge.cwd.join(path)
            }
        }
        None => bridge.cwd.clone(),
        Some(_) => return Err(error_reply("bash workdir must be a string")),
    };
    let workdir = match std::fs::canonicalize(&workdir) {
        Ok(path) if path.is_dir() => path,
        Ok(_) => return Err(error_reply("bash workdir must be a directory")),
        Err(error) => return Err(error_reply(format!("invalid bash workdir: {error}"))),
    };
    input["workdir"] = json!(workdir);
    let tool_call_id = synthetic_tool_call_id("bash");
    let mut req = permission_request(
        bridge,
        tool_call_id.clone(),
        "shell_command",
        input.clone(),
        ResourceKind::ShellExec,
        if interactive {
            format!(
                "Start persistent interactive process (subsequent input may execute commands): {command_str}"
            )
        } else {
            format!("Run shell command: {command_str}")
        },
        /*path*/ None,
        Some(command_str),
    );
    req.cwd = workdir.clone();
    req.sandbox_permissions = match sandbox_permission_request_from_input(&input) {
        Ok(permissions) => permissions,
        Err(error) => {
            return Err(error_reply(format!(
                "invalid sandbox permission request: {error}"
            )));
        }
    };
    req.justification = input
        .get("justification")
        .and_then(Value::as_str)
        .map(str::to_string);
    req.command_prefix = command_prefix_for_tool_input("shell_command", &input);
    req.command_argv = safe_shell_argv(command.as_str().unwrap_or_default());
    req.command_pattern =
        generalize_command_pattern(command.as_str().unwrap_or_default(), &workdir);
    if interactive {
        // A static command prefix does not describe future interactive input.
        req.command_prefix = None;
        req.command_pattern = None;
    }
    let kernel_stopped = async {
        if let Some(lifetime) = &bridge.kernel_lifetime {
            lifetime.cancelled().await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    if bridge.cancel_token.is_cancelled() {
        return Err(error_reply("originating turn cancelled"));
    }
    let authorization = check_or_grant(bridge, req);
    tokio::pin!(authorization);
    let grant = tokio::select! {
        biased;
        _ = kernel_stopped => {
            // Drive the checker's existing cancellation cleanup instead of dropping
            // its pending controller request when the owning kernel disappears.
            bridge.cancel_token.cancel();
            authorization.await
        }
        result = &mut authorization => result,
    }?;
    let mut ctx = tool_context_from_bridge(bridge, tool_call_id.clone());
    if grant.bypass_sandbox {
        ctx.sandbox_profile = Some("off".to_string());
    }
    ctx.sandbox_permission_overlay = grant.sandbox_permission_overlay.clone();
    let retry = if !interactive
        && grant.already_approved
        && !grant.bypass_sandbox
        && grant.sandbox_permission_overlay.is_none()
        && devo_sandbox::unsandboxed_execution_allowed(bridge.sandbox_profile.as_deref(), &workdir)
    {
        devo_core::tools::host_bash::SandboxRetry::ApprovedDenial
    } else {
        devo_core::tools::host_bash::SandboxRetry::Never
    };
    Ok((ctx, input, retry))
}

pub(crate) async fn handle_bash(bridge: &HostBridge, params: &Value) -> Value {
    let (ctx, input, retry) = match prepare_bash(bridge, params).await {
        Ok(prepared) => prepared,
        Err(reply) => return reply,
    };
    map_tool_outcome(
        devo_core::tools::host_bash::execute(
            ctx,
            input,
            retry,
            None,
            devo_core::tools::handlers::ShellExecutionMode::Foreground,
        )
        .await,
    )
}

pub(crate) async fn handle_bash_start(bridge: &HostBridge, params: &Value) -> Value {
    if let Err(message) = devo_core::tools::host_bash::BashMode::parse(params) {
        return error_reply(message);
    }
    let (ctx, input, retry) = match prepare_bash(bridge, params).await {
        Ok(prepared) => prepared,
        Err(reply) => return reply,
    };
    let lifetime = match bridge.runtime() {
        Some(runtime) => match runtime.session(bridge.session_id).await {
            Some(session) => Some(session.lifetime_token()),
            None => return error_reply("originating session is closed"),
        },
        None => None, // Standalone/test callers retain context cancellation.
    };
    devo_core::tools::host_bash::start_with_session_lifetime(
        ctx,
        input,
        retry,
        bridge.kernel_lifetime.clone(),
        lifetime,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use devo_core::tools::PermissionGrant;
    use pretty_assertions::assert_eq;
    use std::sync::Mutex;

    async fn finished(bridge: &HostBridge, id: &Value) -> Value {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let result = devo_core::tools::host_bash::control(
                    bridge.session_id.as_str(),
                    "bash.poll",
                    id,
                );
                if result["result"]["running"] == false {
                    return result;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("shell completion")
    }

    #[tokio::test]
    async fn lifecycle_authorizes_once_and_scopes_handles_across_bridges() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let workspace = tempfile::tempdir().unwrap();
        let mut bridge = HostBridge::for_tests(SessionId::new());
        bridge.cwd = workspace.path().to_path_buf();
        bridge.sandbox_profile = Some("off".into());
        let count = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&count);
        bridge.permission = PermissionChecker::new(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(PermissionGrant::default()) })
        });
        let started = handle_bash_start(&bridge, &json!({"command":"echo HOST_LIFECYCLE"})).await;
        let id = json!({"handle_id":started["result"]["handle_id"]});
        assert_eq!(started["result"]["running"], true);
        assert_eq!(
            devo_core::tools::host_bash::control("other-session", "bash.kill", &id),
            json!({"status":"error", "error":"unknown bash handle"})
        );
        let fresh_bridge = HostBridge::for_tests(bridge.session_id);
        let done = finished(&fresh_bridge, &id).await;
        assert_eq!(
            (
                done["result"]["exit_code"].clone(),
                count.load(Ordering::SeqCst)
            ),
            (json!(0), 1)
        );
        assert!(
            done["result"]["output"]
                .as_str()
                .unwrap()
                .contains("HOST_LIFECYCLE")
        );
        assert_eq!(
            devo_core::tools::host_bash::control(bridge.session_id.as_str(), "bash.forget", &id),
            json!({"status":"ok", "result":{"forgotten":true}})
        );
        assert_eq!(
            devo_core::tools::host_bash::control(bridge.session_id.as_str(), "bash.poll", &id),
            json!({"status":"error", "error":"unknown bash handle"})
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn lifecycle_streams_and_preserves_origin_cancellation() {
        let mut bridge = HostBridge::for_tests(SessionId::new());
        bridge.sandbox_profile = Some("off".into());
        let started =
            handle_bash_start(&bridge, &json!({"command":"printf streaming; sleep 30"})).await;
        let id = json!({"handle_id":started["result"]["handle_id"]});
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let snapshot = devo_core::tools::host_bash::control(
                    bridge.session_id.as_str(),
                    "bash.poll",
                    &id,
                );
                if snapshot["result"]["output"]
                    .as_str()
                    .unwrap()
                    .contains("streaming")
                {
                    assert_eq!(snapshot["result"]["running"], true);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        bridge.cancel_token.cancel();
        let done = finished(&bridge, &id).await;
        assert!(
            done["result"]["output"]
                .as_str()
                .unwrap()
                .contains("cancelled")
        );
        devo_core::tools::host_bash::cancel_session(bridge.session_id.as_str());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn background_default_has_no_deadline_but_explicit_timeout_expires() {
        let mut bridge = HostBridge::for_tests(SessionId::new());
        bridge.sandbox_profile = Some("off".into());
        let started = handle_bash_start(
            &bridge,
            &json!({"command":"printf ready; sleep 30", "login":false}),
        )
        .await;
        let id = json!({"handle_id":started["result"]["handle_id"]});
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let snapshot = devo_core::tools::host_bash::control(
                    bridge.session_id.as_str(),
                    "bash.poll",
                    &id,
                );
                if snapshot["result"]["output"]
                    .as_str()
                    .unwrap()
                    .contains("ready")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(121)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            devo_core::tools::host_bash::control(bridge.session_id.as_str(), "bash.poll", &id)["result"]
                ["running"],
            true
        );
        tokio::time::resume();
        devo_core::tools::host_bash::control(bridge.session_id.as_str(), "bash.kill", &id);
        let cancelled = finished(&bridge, &id).await;
        assert_eq!(cancelled["result"]["exit_code"], Value::Null);
        assert!(cancelled["result"]["error"].is_string());
        let started = handle_bash_start(
            &bridge,
            &json!({"command":"sleep 30", "timeout_ms":1, "login":false}),
        )
        .await;
        let id = json!({"handle_id":started["result"]["handle_id"]});
        let done = finished(&bridge, &id).await;
        assert!(
            done["result"]["output"]
                .as_str()
                .unwrap()
                .contains("timed out"),
            "{done}"
        );
        devo_core::tools::host_bash::cancel_session(bridge.session_id.as_str());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kernel_lifetime_cancels_without_cancelling_originating_turn() {
        let mut bridge = HostBridge::for_tests(SessionId::new());
        bridge.sandbox_profile = Some("off".into());
        let lifetime = CancellationToken::new();
        bridge.kernel_lifetime = Some(lifetime.clone());
        let started = handle_bash_start(&bridge, &json!({"command":"sleep 30"})).await;
        let id = json!({"handle_id":started["result"]["handle_id"]});
        lifetime.cancel();
        let done = finished(&bridge, &id).await;
        assert!(!bridge.cancel_token.is_cancelled());
        assert!(done["result"]["error"].is_string(), "{done}");
        devo_core::tools::host_bash::cancel_session(bridge.session_id.as_str());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn lifecycle_workspace_sandbox_denies_outside_write() {
        let workspace = tempfile::tempdir().unwrap();
        // The system temporary directory is intentionally writable by workspace
        // profiles, so put this disposable probe under the user's home instead.
        let home = std::env::var_os("HOME").expect("macOS home");
        let outside = tempfile::tempdir_in(home).unwrap();
        let forbidden = outside.path().join("sandbox-probe");
        let mut bridge = HostBridge::for_tests(SessionId::new());
        bridge.cwd = workspace.path().to_path_buf();
        bridge.sandbox_profile = Some("workspace".into());
        let path = forbidden.to_string_lossy().replace('\'', "'\"'\"'");
        let command = format!("printf blocked > '{path}'");
        let started = handle_bash_start(&bridge, &json!({"command":command, "login":false})).await;
        let id = json!({"handle_id":started["result"]["handle_id"]});
        let done = finished(&bridge, &id).await;
        assert!(
            !forbidden.exists(),
            "workspace sandbox allowed outside write: {done}"
        );
        assert!(done["result"]["error"].is_string(), "{done}");
        assert!(
            done["result"]["output"]
                .as_str()
                .unwrap()
                .contains("SANDBOX_DENIED"),
            "{done}"
        );
        devo_core::tools::host_bash::cancel_session(bridge.session_id.as_str());
    }

    #[tokio::test]
    async fn completed_handles_do_not_block_sequential_commands() {
        let mut bridge = HostBridge::for_tests(SessionId::new());
        bridge.sandbox_profile = Some("off".into());
        for _ in 0..33 {
            let started =
                handle_bash_start(&bridge, &json!({"command":"echo quota", "login":false})).await;
            assert_eq!(started["status"], "ok", "{started}");
            let id = json!({"handle_id":started["result"]["handle_id"]});
            assert_eq!(finished(&bridge, &id).await["result"]["exit_code"], 0);
        }
        devo_core::tools::host_bash::cancel_session(bridge.session_id.as_str());
    }

    #[tokio::test]
    async fn cancelled_origin_releases_pending_authorization() {
        let mut bridge = HostBridge::for_tests(SessionId::new());
        let entered = Arc::new(tokio::sync::Notify::new());
        let seen = Arc::clone(&entered);
        let checker_cancel = bridge.cancel_token.clone();
        bridge.permission = PermissionChecker::new(move |_| {
            let seen = Arc::clone(&seen);
            let checker_cancel = checker_cancel.clone();
            Box::pin(async move {
                seen.notify_one();
                checker_cancel.cancelled().await;
                Err("originating turn cancelled".into())
            })
        });
        let cancel = bridge.cancel_token.clone();
        let input = json!({"command":"echo never"});
        let request = handle_bash_start(&bridge, &input);
        let interrupt = async {
            entered.notified().await;
            cancel.cancel();
        };
        let (reply, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(request, interrupt)
        })
        .await
        .expect("approval cancellation");
        assert_eq!(
            reply,
            json!({"status":"error", "error":"originating turn cancelled"})
        );
    }

    #[tokio::test]
    async fn lifecycle_denial_does_not_allocate_handle() {
        let mut bridge = HostBridge::for_tests(SessionId::new());
        bridge.permission = PermissionChecker::new(|_| Box::pin(async { Err("denied".into()) }));
        assert_eq!(
            handle_bash_start(&bridge, &json!({"command":"echo never"})).await,
            json!({"status":"error", "error":"denied"})
        );
    }

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
                {
                    let mut expected = input;
                    expected["workdir"] = json!(std::fs::canonicalize(workspace.path()).unwrap());
                    expected
                },
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
    async fn approval_uses_the_execution_directory() {
        let workspace = tempfile::tempdir().expect("workspace");
        let child = workspace.path().join("child");
        std::fs::create_dir(&child).expect("child directory");
        let expected = std::fs::canonicalize(child).expect("canonical directory");
        let mut bridge = HostBridge::for_tests("ses_shell_workdir");
        bridge.cwd = workspace.path().to_path_buf();
        bridge.permission = PermissionChecker::new(move |request| {
            let expected = expected.clone();
            Box::pin(async move {
                assert_eq!(
                    (request.cwd, request.input["workdir"].clone()),
                    (expected.clone(), json!(expected)),
                );
                Err("stop before execution".to_string())
            })
        });
        assert_eq!(
            handle_bash(&bridge, &json!({"command":"echo probe", "workdir":"child"})).await,
            json!({"status":"error", "error":"stop before execution"}),
        );
    }

    #[tokio::test]
    async fn malformed_directory_never_reaches_approval() {
        let mut bridge = HostBridge::for_tests("ses_shell_bad_workdir");
        bridge.permission = PermissionChecker::new(|_| {
            Box::pin(async { panic!("invalid workdir reached approval") })
        });
        assert_eq!(
            handle_bash(&bridge, &json!({"command":"echo probe", "workdir":42})).await,
            json!({"status":"error", "error":"bash workdir must be a string"}),
        );
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
    async fn invalid_permissions_never_authorize() {
        let mut bridge = HostBridge::for_tests("ses_shell_invalid");
        bridge.permission = PermissionChecker::new(|_| {
            Box::pin(async { panic!("invalid request reached authorization") })
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
    }

    #[tokio::test]
    async fn plan_mode_invokes_configured_policy_and_respects_denial() {
        let mut bridge = HostBridge::for_tests("ses_shell_plan_denied");
        bridge.collaboration_mode = CollaborationMode::Plan;
        bridge.sandbox_profile = Some("off".to_string());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&calls);
        bridge.permission = PermissionChecker::new(move |request| {
            let seen = Arc::clone(&seen);
            Box::pin(async move {
                seen.lock()
                    .unwrap()
                    .push((request.tool_name, request.input["command"].clone()));
                Err("denied by configured policy".to_string())
            })
        });
        assert_eq!(
            handle_bash_start(&bridge, &json!({"command":"echo should-not-run"})).await,
            json!({"status":"error", "error":"denied by configured policy"})
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec![("shell_command".to_string(), json!("echo should-not-run"))]
        );
    }

    #[tokio::test]
    async fn plan_mode_allow_starts_normal_host_execution() {
        let mut bridge = HostBridge::for_tests("ses_shell_plan_allow");
        bridge.collaboration_mode = CollaborationMode::Plan;
        bridge.sandbox_profile = Some("off".to_string());
        bridge.permission = PermissionChecker::always_allow();
        let started = handle_bash_start(&bridge, &json!({"command":"echo PLAN_ALLOW_OK"})).await;
        assert_eq!(started["status"], json!("ok"), "{started}");
        let handle_id = json!({"handle_id": started["result"]["handle_id"]});
        let done = finished(&bridge, &handle_id).await;
        assert_eq!(done["result"]["exit_code"], json!(0), "{done}");
        assert!(
            done["result"]["output"]
                .as_str()
                .unwrap_or_default()
                .contains("PLAN_ALLOW_OK"),
            "{done}"
        );
        assert_eq!(
            devo_core::tools::host_bash::control(
                bridge.session_id.as_str(),
                "bash.forget",
                &handle_id
            ),
            json!({"status":"ok", "result":{"forgotten":true}})
        );
    }

    #[tokio::test]
    async fn plan_mode_auto_allow_keeps_configured_sandbox_profile() {
        let mut bridge = HostBridge::for_tests("ses_shell_plan_sandbox");
        bridge.collaboration_mode = CollaborationMode::Plan;
        bridge.sandbox_profile = Some("workspace".to_string());
        bridge.permission = PermissionChecker::always_allow();
        let (ctx, _input, _retry) = prepare_bash(&bridge, &json!({"command":"echo sandbox"}))
            .await
            .expect("prepared Plan shell");
        assert_eq!(ctx.sandbox_profile, Some("workspace".to_string()));
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
