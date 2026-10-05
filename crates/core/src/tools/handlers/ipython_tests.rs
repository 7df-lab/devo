//! Unit tests for the ipython tool handler (RLM spike).

use pretty_assertions::assert_eq;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::contracts::{ToolAgentScope, ToolBudgets, ToolContext, ToolResultContent};
use crate::tool_handler::ToolHandler;
use crate::tools::handlers::ipython::{
    IpythonHandler, KernelNamespaceRestoreState, ensure_kernel, ensure_kernel_with_restore_state,
    kernel_namespace_manifest_path, kernel_namespace_snapshot_path,
};

fn ctx_with_kernel(kernel: std::sync::Arc<devo_kernel::KernelSession>) -> ToolContext {
    ToolContext {
        output_store: None,
        tool_call_id: crate::invocation::ToolCallId("call-1".into()),
        session_id: "sess".into(),
        turn_id: Some("turn".into()),
        workspace_root: std::env::temp_dir(),
        budgets: ToolBudgets {
            output_limit_bytes: 64 * 1024,
            wall_time_limit_ms: None,
        },
        cancel_token: CancellationToken::new(),
        agent_scope: ToolAgentScope::Parent,
        collaboration_mode: devo_protocol::CollaborationMode::Build,
        agent_coordinator: None,
        client_filesystem: None,
        file_read_ledger: None,
        network_proxy: None,
        network_no_proxy: None,
        sandbox_profile: None,
        sandbox_permission_overlay: None,
        kernel: Some(kernel),
        python_cell_first_wait_ms: None,
        python_cell_watch: None,
        python_cell_completion: None,
        session_dir: None,
    }
}

/// Trace: L2-DES-RLM-001
/// Verifies: IpythonHandler executes code and returns the kernel details JSON shape.
#[tokio::test]
async fn ipython_handler_executes_and_returns_details() {
    let kernel = match ensure_kernel(
        &None,
        &std::env::temp_dir(),
        None,
        devo_protocol::CollaborationMode::Build,
    )
    .await
    {
        Ok(k) => k,
        Err(e) => {
            eprintln!("skip: no kernel ({e})");
            return;
        }
    };
    let handler = IpythonHandler::new();
    let result = handler
        .handle(ctx_with_kernel(kernel), json!({ "code": "1 + 1" }), None)
        .await
        .expect("handle");
    match result.content {
        ToolResultContent::Json(value) => {
            let details = value.get("details").expect("details");
            assert_eq!(details["status"], "ok");
            assert!(
                details["durationMs"].as_u64().is_some(),
                "expected durationMs in details, got {details}"
            );
            let body = value["content"][0]["text"].as_str().unwrap_or("");
            assert!(
                body.contains('2') || details["result"].as_str().is_some_and(|r| r.contains('2')),
                "expected 2 in output, text={body:?} details={details}"
            );
        }
        other => panic!("unexpected content {other:?}"),
    }
}

/// Trace: L2-DES-RLM-001
/// Verifies: namespace survives across two IpythonHandler invocations (cross-turn).
#[tokio::test]
async fn ipython_handler_namespace_survives_two_calls() {
    let kernel = match ensure_kernel(
        &None,
        &std::env::temp_dir(),
        None,
        devo_protocol::CollaborationMode::Build,
    )
    .await
    {
        Ok(k) => k,
        Err(e) => {
            eprintln!("skip: no kernel ({e})");
            return;
        }
    };
    let handler = IpythonHandler::new();
    let ctx = ctx_with_kernel(std::sync::Arc::clone(&kernel));
    handler
        .handle(ctx.clone(), json!({ "code": "x = 41" }), None)
        .await
        .expect("turn1");
    let second = handler
        .handle(ctx, json!({ "code": "print(x + 1)" }), None)
        .await
        .expect("turn2");
    match second.content {
        ToolResultContent::Json(value) => {
            let details = value.get("details").expect("details");
            assert_eq!(details["status"], "ok");
            let stdout = details["stdout"].as_str().unwrap_or("");
            let body = value["content"][0]["text"].as_str().unwrap_or("");
            assert!(
                stdout.contains("42") || body.contains("42"),
                "expected 42, stdout={stdout:?} body={body:?}"
            );
        }
        other => panic!("unexpected {other:?}"),
    }
}

/// Trace: L2-DES-RLM-001
/// Verifies: ensure_kernel reuses an existing Arc and two executes share namespace.
#[tokio::test]
async fn ensure_kernel_reuses_arc_across_two_executes() {
    let first = match ensure_kernel(
        &None,
        &std::env::temp_dir(),
        None,
        devo_protocol::CollaborationMode::Build,
    )
    .await
    {
        Ok(k) => k,
        Err(e) => {
            eprintln!("skip: no kernel ({e})");
            return;
        }
    };
    let second = ensure_kernel(
        &Some(std::sync::Arc::clone(&first)),
        &std::env::temp_dir(),
        None,
        devo_protocol::CollaborationMode::Build,
    )
    .await
    .expect("reuse");
    assert!(
        std::sync::Arc::ptr_eq(&first, &second),
        "ensure_kernel must reuse the session Arc"
    );
    first.execute("reuse_n = 7").await.expect("bind");
    let out = second.execute("print(reuse_n)").await.expect("read");
    assert_eq!(out.status, "ok", "stderr={}", out.stderr);
    assert!(
        out.stdout.contains('7'),
        "expected shared namespace, stdout={:?} result={:?}",
        out.stdout,
        out.result
    );
}

/// Trace: L2-DES-RLM-001
/// Verifies: cancel token during a long cell interrupts the kernel and returns Cancelled.
#[tokio::test]
async fn ipython_handler_cancel_interrupts_long_cell() {
    let kernel = match ensure_kernel(
        &None,
        &std::env::temp_dir(),
        None,
        devo_protocol::CollaborationMode::Build,
    )
    .await
    {
        Ok(k) => k,
        Err(e) => {
            eprintln!("skip: kernel unavailable: {e}");
            return;
        }
    };
    let cancel = CancellationToken::new();
    let mut ctx = ctx_with_kernel(std::sync::Arc::clone(&kernel));
    ctx.cancel_token = cancel.clone();
    let handler = IpythonHandler::new();
    let code = concat!(
        "import time\n",
        "for _ in range(200):\n",
        "    time.sleep(0.05)\n",
    );
    let run = handler.handle(ctx, json!({ "code": code }), None);
    tokio::pin!(run);
    tokio::select! {
        biased;
        result = &mut run => {
            panic!("expected cancel before cell finished: {result:?}");
        }
        () = tokio::time::sleep(std::time::Duration::from_millis(150)) => {
            cancel.cancel();
        }
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), run)
        .await
        .expect("cancel should finish promptly");
    assert!(
        matches!(result, Err(crate::contracts::ToolCallError::Cancelled)),
        "expected Cancelled, got {result:?}"
    );
}

struct FixedWatch(devo_tools::PythonCellWatchAction);

#[async_trait::async_trait]
impl devo_tools::PythonCellWatch for FixedWatch {
    async fn decide(
        &self,
        _input: devo_tools::PythonCellWatchInput,
    ) -> devo_tools::PythonCellWatchDecision {
        devo_tools::PythonCellWatchDecision {
            action: self.0.clone(),
            rationale: Some("test".into()),
        }
    }
}

/// Trace: L2-DES-RLM-001
/// Verifies: first-wait expiry + continue_fg eventually completes the cell in foreground.
#[tokio::test]
async fn ipython_wait_budget_continue_fg_completes() {
    let kernel = match ensure_kernel(
        &None,
        &std::env::temp_dir(),
        None,
        devo_protocol::CollaborationMode::Build,
    )
    .await
    {
        Ok(k) => k,
        Err(e) => {
            eprintln!("skip: kernel unavailable: {e}");
            return;
        }
    };
    let mut ctx = ctx_with_kernel(std::sync::Arc::clone(&kernel));
    ctx.python_cell_first_wait_ms = Some(80);
    ctx.python_cell_watch = Some(std::sync::Arc::new(FixedWatch(
        devo_tools::PythonCellWatchAction::ContinueFg { wait_seconds: 2 },
    )));
    let handler = IpythonHandler::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        handler.handle(
            ctx,
            json!({ "code": "import time; time.sleep(0.25); print('done-wait')" }),
            None,
        ),
    )
    .await
    .expect("timeout")
    .expect("handle");
    match result.content {
        ToolResultContent::Json(value) => {
            assert_eq!(value["details"]["status"], "ok");
            let body = value["content"][0]["text"].as_str().unwrap_or("");
            assert!(
                body.contains("done-wait")
                    || value["details"]["stdout"]
                        .as_str()
                        .is_some_and(|s| s.contains("done-wait")),
                "body={body:?} details={}",
                value["details"]
            );
        }
        other => panic!("unexpected {other:?}"),
    }
}

/// Trace: L2-DES-RLM-001
/// Verifies: wait policy cancel interrupts the cell.
#[tokio::test]
async fn ipython_wait_budget_cancel_interrupts() {
    let kernel = match ensure_kernel(
        &None,
        &std::env::temp_dir(),
        None,
        devo_protocol::CollaborationMode::Build,
    )
    .await
    {
        Ok(k) => k,
        Err(e) => {
            eprintln!("skip: kernel unavailable: {e}");
            return;
        }
    };
    let mut ctx = ctx_with_kernel(std::sync::Arc::clone(&kernel));
    ctx.python_cell_first_wait_ms = Some(50);
    ctx.python_cell_watch = Some(std::sync::Arc::new(FixedWatch(
        devo_tools::PythonCellWatchAction::Cancel,
    )));
    let handler = IpythonHandler::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        handler.handle(
            ctx,
            json!({ "code": "import time\nwhile True:\n    time.sleep(0.05)\n" }),
            None,
        ),
    )
    .await
    .expect("timeout")
    .expect("handle");
    match result.content {
        ToolResultContent::Json(value) => {
            assert_eq!(value["details"]["status"], "cancelled by wait policy");
        }
        other => panic!("unexpected {other:?}"),
    }
}

/// Trace: L2-DES-RLM-001
/// Verifies: background parks the cell and returns provisional status.
#[tokio::test]
async fn ipython_wait_budget_background_parks() {
    let kernel = match ensure_kernel(
        &None,
        &std::env::temp_dir(),
        None,
        devo_protocol::CollaborationMode::Build,
    )
    .await
    {
        Ok(k) => k,
        Err(e) => {
            eprintln!("skip: kernel unavailable: {e}");
            return;
        }
    };
    let mut ctx = ctx_with_kernel(std::sync::Arc::clone(&kernel));
    ctx.python_cell_first_wait_ms = Some(50);
    ctx.python_cell_watch = Some(std::sync::Arc::new(FixedWatch(
        devo_tools::PythonCellWatchAction::Background,
    )));
    let handler = IpythonHandler::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        handler.handle(
            ctx,
            json!({ "code": "import time; time.sleep(2); print('bg-done')" }),
            None,
        ),
    )
    .await
    .expect("timeout")
    .expect("handle");
    match result.content {
        ToolResultContent::Json(value) => {
            assert_eq!(value["details"]["status"], "background");
            assert!(value["details"]["cellId"].as_str().is_some());
        }
        other => panic!("unexpected {other:?}"),
    }
    // Wait for parked cell to finish so the gate releases for later tests.
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
}

/// Trace: L2-DES-RLM-001
/// Verifies: exhausted continue renewals force background.
#[tokio::test]
async fn ipython_wait_budget_max_renewals_force_background() {
    let kernel = match ensure_kernel(
        &None,
        &std::env::temp_dir(),
        None,
        devo_protocol::CollaborationMode::Build,
    )
    .await
    {
        Ok(k) => k,
        Err(e) => {
            eprintln!("skip: kernel unavailable: {e}");
            return;
        }
    };
    let mut ctx = ctx_with_kernel(std::sync::Arc::clone(&kernel));
    ctx.python_cell_first_wait_ms = Some(40);
    // Always ask to continue with a short wait; after max renewals the handler must park.
    ctx.python_cell_watch = Some(std::sync::Arc::new(FixedWatch(
        devo_tools::PythonCellWatchAction::ContinueFg { wait_seconds: 1 },
    )));
    let handler = IpythonHandler::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        handler.handle(
            ctx,
            json!({ "code": "import time\nwhile True:\n    time.sleep(0.2)\n" }),
            None,
        ),
    )
    .await
    .expect("timeout")
    .expect("handle");
    match result.content {
        ToolResultContent::Json(value) => {
            assert_eq!(value["details"]["status"], "background");
        }
        other => panic!("unexpected {other:?}"),
    }
    let _ = kernel.interrupt(None).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
}

#[test]
fn kernel_namespace_paths_are_canonical_under_session_dir() {
    let dir = std::path::Path::new("/tmp/session-artifacts/abc");
    assert_eq!(kernel_namespace_snapshot_path(dir), dir.join("kernel.dill"));
    assert_eq!(kernel_namespace_manifest_path(dir), dir.join("kernel.json"));
}

/// Trace: L2-DES-RLM-001
/// Verifies: ensure_kernel restores dill from session_dir before bootstrap.
#[tokio::test]
async fn ensure_kernel_restores_namespace_from_session_dir() {
    let session_dir = tempfile::tempdir().expect("session_dir");
    let (first, first_restore_state) = match ensure_kernel_with_restore_state(
        &None,
        &std::env::temp_dir(),
        Some(session_dir.path()),
        devo_protocol::CollaborationMode::Build,
    )
    .await
    {
        Ok(result) => result,
        Err(e) => {
            eprintln!("skip: no kernel ({e})");
            return;
        }
    };
    assert_eq!(
        first_restore_state,
        Some(KernelNamespaceRestoreState::Fresh)
    );
    let bind = first.execute("resume_marker = 4242").await.expect("bind");
    if bind.status != "ok" {
        eprintln!("skip: execute failed ({})", bind.stderr);
        return;
    }
    let dill = kernel_namespace_snapshot_path(session_dir.path());
    let manifest = kernel_namespace_manifest_path(session_dir.path());
    let snap = first.snapshot(&dill, &manifest).await.expect("snapshot");
    if snap.status != "ok" {
        eprintln!(
            "skip: snapshot unsupported (status={} stderr={})",
            snap.status, snap.stderr
        );
        return;
    }
    drop(first);

    let (second, second_restore_state) = ensure_kernel_with_restore_state(
        &None,
        &std::env::temp_dir(),
        Some(session_dir.path()),
        devo_protocol::CollaborationMode::Build,
    )
    .await
    .expect("respawn+restore");
    assert_eq!(
        second_restore_state,
        Some(KernelNamespaceRestoreState::Restored)
    );
    let check = second.execute("print(resume_marker)").await.expect("check");
    assert_eq!(check.status, "ok", "stderr={}", check.stderr);
    assert!(
        check.stdout.contains("4242"),
        "expected restored resume_marker, stdout={:?} result={:?}",
        check.stdout,
        check.result
    );
}

#[tokio::test]
async fn rlm_bootstrap_imports_core_python_skill_modules() {
    use crate::rlm_prompts::RLM_BOOTSTRAP_SKILL_IMPORTS;

    let kernel = match ensure_kernel(
        &None,
        &std::env::temp_dir(),
        None,
        devo_protocol::CollaborationMode::Build,
    )
    .await
    {
        Ok(kernel) => kernel,
        Err(error) => {
            eprintln!("skip: no kernel ({error})");
            return;
        }
    };
    let expected = ["compact", "refine", "goal", "agent_observe"];
    assert!(
        expected
            .iter()
            .all(|name| RLM_BOOTSTRAP_SKILL_IMPORTS.contains(name))
    );
    let expected_json = serde_json::to_string(&expected).expect("skill names serialize");
    let code = format!(
        concat!(
            "expected = {}\n",
            "missing = [name for name in expected ",
            "if name not in globals() or globals()[name] is None]\n",
            "assert not missing, f\"bootstrap import missing: {{missing}}\"\n",
            "unwrapped = []\n",
            "for name in expected:\n",
            "    module = globals()[name]\n",
            "    if callable(getattr(module, 'run', None)) and not callable(module):\n",
            "        unwrapped.append(name)\n",
            "assert not unwrapped, f\"run-capable skills are not callable: {{unwrapped}}\"\n",
        ),
        expected_json
    );
    let output = kernel
        .execute(&code)
        .await
        .expect("execute skill import check");
    assert_eq!(output.status, "ok", "stderr={}", output.stderr);
}

#[tokio::test]
async fn rlm_bootstrap_exposes_generic_mcp_module() {
    let kernel = match ensure_kernel(
        &None,
        &std::env::temp_dir(),
        None,
        devo_protocol::CollaborationMode::Build,
    )
    .await
    {
        Ok(kernel) => kernel,
        Err(error) => {
            eprintln!("skip: no kernel ({error})");
            return;
        }
    };
    let output = kernel
        .execute(
            "assert mcp.__name__ == 'rlm.mcp'\nassert callable(mcp.list_tools)\nassert callable(mcp.call_tool)\n",
        )
        .await
        .expect("execute MCP bootstrap check");
    assert_eq!(output.status, "ok", "stderr={}", output.stderr);
}

#[test]
fn kernel_fence_grants_global_harness_state_writable() {
    // The global harness store must be kernel-writable: `rlm.harness`
    // create/update/delete with `global_=True` writes it directly, and the
    // kernel's own guidance points models there. The grant must come from the
    // same helper that builds RLM_GLOBAL_HARNESS_STATE_DIR so env and fence
    // can never disagree.
    use crate::tools::handlers::ipython::{global_harness_state_dir, resolve_kernel_fence};

    let fence = resolve_kernel_fence(
        std::path::Path::new("/workspace"),
        Some(std::path::Path::new("/artifacts/session")),
        devo_protocol::CollaborationMode::Build,
    )
    .expect("fence spec");
    assert!(
        fence
            .writable_roots
            .contains(&std::path::PathBuf::from("/workspace"))
    );
    assert!(
        fence
            .writable_roots
            .contains(&std::path::PathBuf::from("/artifacts/session"))
    );
    assert!(
        fence.writable_roots.contains(&global_harness_state_dir()),
        "global harness dir must be writable by the kernel, got {:?}",
        fence.writable_roots
    );
}

#[test]
fn plan_fence_reads_workspace_but_only_build_fence_writes_it() {
    use crate::tools::handlers::ipython::resolve_kernel_fence;
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace = workspace.path();
    let plan = resolve_kernel_fence(workspace, None, devo_protocol::CollaborationMode::Plan)
        .expect("Plan fence");
    let build = resolve_kernel_fence(workspace, None, devo_protocol::CollaborationMode::Build)
        .expect("Build fence");
    assert!(plan.readable_roots.contains(&workspace.to_path_buf()));
    assert!(!plan.writable_roots.contains(&workspace.to_path_buf()));
    assert!(build.writable_roots.contains(&workspace.to_path_buf()));
    assert_eq!(plan.restrict_network, build.restrict_network);
}

#[cfg(unix)]
#[tokio::test]
async fn plan_kernel_fails_closed_when_workspace_is_in_scratch() {
    let workspace = tempfile::tempdir().expect("workspace under /tmp");
    let err = match ensure_kernel(
        &None,
        workspace.path(),
        None,
        devo_protocol::CollaborationMode::Plan,
    )
    .await
    {
        Ok(_) => panic!("Plan must not use a writable scratch ancestor"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("overlaps workspace"),
        "unexpected Plan failure: {err}"
    );
}

/// Native Python file I/O, not just an inspection of the fence spec.
/// Linux is supported when Landlock is provisioned. Unavailable sandbox
/// support must return an error rather than launching a writable Plan kernel.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn plan_kernel_denies_python_workspace_write_but_allows_scratch_and_mode_switch() {
    let workspace = tempfile::Builder::new()
        .prefix(".devo-plan-kernel-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("workspace outside scratch");
    let scratch = tempfile::tempdir().expect("scratch");
    let read_path = workspace.path().join("read.txt");
    let blocked_path = workspace.path().join("blocked.txt");
    let scratch_path = scratch.path().join("scratch.txt");
    std::fs::write(&read_path, "allowed read").expect("seed workspace");

    let plan = match ensure_kernel(
        &None,
        workspace.path(),
        None,
        devo_protocol::CollaborationMode::Plan,
    )
    .await
    {
        Ok(kernel) => kernel,
        Err(error) => {
            assert!(
                error.to_string().contains("Plan kernel unavailable")
                    || error.to_string().contains("failed to spawn")
                    || error.to_string().contains("kernel handshake failed"),
                "unexpected kernel failure: {error}"
            );
            eprintln!("Plan kernel not provisioned; fails closed: {error}");
            return;
        }
    };
    assert_eq!(plan.fence_state(), devo_kernel::FenceState::Fenced);
    assert_eq!(
        plan.workspace_access(),
        devo_kernel::WorkspaceAccess::ReadOnly
    );
    let read_code = format!(
        "print(open({}).read())",
        serde_json::to_string(&read_path.display().to_string()).expect("path JSON")
    );
    let read = plan
        .execute(read_code)
        .await
        .expect("Python workspace read");
    assert_eq!(read.status, "ok", "stderr={}", read.stderr);
    assert!(read.stdout.contains("allowed read"));
    let write_code = format!(
        "open({}, 'w').write('bypass')",
        serde_json::to_string(&blocked_path.display().to_string()).expect("path JSON")
    );
    let denied = plan
        .execute(write_code.clone())
        .await
        .expect("Python workspace write");
    assert_eq!(denied.error_name.as_deref(), Some("PermissionError"));
    assert!(
        !blocked_path.exists(),
        "workspace write bypassed Plan fence"
    );
    let scratch_code = format!(
        "open({}, 'w').write('scratch ok')",
        serde_json::to_string(&scratch_path.display().to_string()).expect("path JSON")
    );
    let scratch_out = plan
        .execute(scratch_code)
        .await
        .expect("Python scratch write");
    assert_eq!(scratch_out.status, "ok", "stderr={}", scratch_out.stderr);
    assert_eq!(
        std::fs::read_to_string(&scratch_path).expect("scratch result"),
        "scratch ok"
    );

    let build = ensure_kernel(
        &Some(std::sync::Arc::clone(&plan)),
        workspace.path(),
        None,
        devo_protocol::CollaborationMode::Build,
    )
    .await
    .expect("switch to Build");
    assert!(!std::sync::Arc::ptr_eq(&plan, &build));
    assert_eq!(
        build.workspace_access(),
        devo_kernel::WorkspaceAccess::ReadWrite
    );
    let allowed = build.execute(write_code).await.expect("Build Python write");
    assert_eq!(allowed.status, "ok", "stderr={}", allowed.stderr);
    assert_eq!(
        std::fs::read_to_string(&blocked_path).expect("Build result"),
        "bypass"
    );

    let plan_again = ensure_kernel(
        &Some(build),
        workspace.path(),
        None,
        devo_protocol::CollaborationMode::Plan,
    )
    .await
    .expect("switch back to Plan");
    assert_eq!(
        plan_again.workspace_access(),
        devo_kernel::WorkspaceAccess::ReadOnly
    );
    let denied_again = plan_again
        .execute(format!(
            "open({}, 'w')",
            serde_json::to_string(&blocked_path.display().to_string()).expect("path JSON")
        ))
        .await
        .expect("Plan write again");
    assert_eq!(denied_again.error_name.as_deref(), Some("PermissionError"));
    // A prior fenced Plan kernel cannot be reused when the workspace moves
    // underneath the writable scratch root.
    let moved = ensure_kernel(
        &Some(plan_again),
        scratch.path(),
        None,
        devo_protocol::CollaborationMode::Plan,
    )
    .await;
    assert!(
        matches!(moved, Err(ref error) if error.to_string().contains("overlaps workspace")),
        "workspace move must fail closed"
    );
}
