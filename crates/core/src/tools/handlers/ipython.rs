//! Model-facing `ipython` tool — executes code in the session RLM kernel.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgress, ToolProgressSender, ToolResult, ToolResultContent,
    ToolResultImage,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolSpec};
use devo_kernel::CellWaitOutcome;
use devo_tools::python_cell_watch::{
    PYTHON_CELL_CODE_PREVIEW_CHARS, PYTHON_CELL_MAX_CONTINUE_RENEWALS,
    PYTHON_CELL_OUTPUT_TAIL_CHARS, PythonCellCompletionEvent, PythonCellWatchAction,
    PythonCellWatchDecision, PythonCellWatchInput, effective_first_wait_ms, output_tail,
};

pub struct IpythonHandler {
    spec: ToolSpec,
}

impl Default for IpythonHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl IpythonHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "ipython".into(),
                description: "Execute Python code in the session RLM kernel. Namespace persists across cells and turns.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([(
                        "code".to_string(),
                        JsonSchema::string(Some("Python source to execute")),
                    )]),
                    Some(vec!["code".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::StructuredJson,
                execution_mode: ToolExecutionMode::Mutating,
                capability_tags: vec![],
                supports_parallel: false,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
                display_name: Some("ipython".into()),
                supports_cancellation: Some(true),
                supports_streaming: Some(true),
            },
        }
    }
}

#[async_trait]
impl ToolHandler for IpythonHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let code = input
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'code' field".into()))?;

        let kernel = ctx.kernel.as_ref().ok_or_else(|| {
            ToolCallError::InternalError(
                "ipython tool requires a session kernel (execution_surface=Rlm)".into(),
            )
        })?;

        let started = std::time::Instant::now();
        let first_wait = std::time::Duration::from_millis(effective_first_wait_ms(
            ctx.python_cell_first_wait_ms,
        ));

        let inflight = tokio::select! {
            biased;
            () = ctx.cancel_token.cancelled() => {
                let _ = kernel.interrupt(None).await;
                return Err(ToolCallError::Cancelled);
            }
            result = kernel.begin_execute(code.to_string()) => {
                result.map_err(|e| ToolCallError::InternalError(e.to_string()))?
            }
        };

        let mut renewals_remaining = PYTHON_CELL_MAX_CONTINUE_RENEWALS;
        let mut wait_budget = first_wait;
        let output = loop {
            let wait_outcome = tokio::select! {
                biased;
                () = ctx.cancel_token.cancelled() => {
                    let _ = inflight.interrupt().await;
                    let _ = inflight.wait_until_done().await;
                    return Err(ToolCallError::Cancelled);
                }
                outcome = inflight.wait_for(wait_budget) => {
                    outcome.map_err(|e| ToolCallError::InternalError(e.to_string()))?
                }
            };

            match wait_outcome {
                CellWaitOutcome::Done(output) => break output,
                CellWaitOutcome::TimedOut { snapshot } => {
                    if let Some(progress) = &progress {
                        let _ = progress.send(ToolProgress::StatusUpdate {
                            message: "Python cell still running; consulting wait policy…".into(),
                            percent: None,
                        });
                    }

                    let decision = if renewals_remaining == 0 {
                        PythonCellWatchDecision {
                            action: PythonCellWatchAction::Background,
                            rationale: Some(
                                "max continue_fg renewals reached; forcing background".into(),
                            ),
                        }
                    } else if let Some(watch) = &ctx.python_cell_watch {
                        watch
                            .decide(PythonCellWatchInput {
                                cell_id: inflight.cell_id().to_string(),
                                code_preview: output_tail(code, PYTHON_CELL_CODE_PREVIEW_CHARS),
                                elapsed_ms: started.elapsed().as_millis() as u64,
                                stdout_tail: output_tail(
                                    &snapshot.stdout,
                                    PYTHON_CELL_OUTPUT_TAIL_CHARS,
                                ),
                                stderr_tail: output_tail(
                                    &snapshot.stderr,
                                    PYTHON_CELL_OUTPUT_TAIL_CHARS,
                                ),
                                renewals_remaining,
                            })
                            .await
                    } else {
                        // No decider (unit tests): keep extending first-wait without parking.
                        wait_budget = first_wait;
                        continue;
                    };

                    match decision.action {
                        PythonCellWatchAction::ContinueFg { wait_seconds } => {
                            if renewals_remaining == 0 {
                                // Should have been forced to background above.
                                renewals_remaining = 0;
                            } else {
                                renewals_remaining = renewals_remaining.saturating_sub(1);
                            }
                            wait_budget = std::time::Duration::from_secs(wait_seconds);
                            if let Some(progress) = &progress {
                                let _ = progress.send(ToolProgress::StatusUpdate {
                                    message: format!(
                                        "Continuing foreground wait for {wait_seconds}s…"
                                    ),
                                    percent: None,
                                });
                            }
                        }
                        PythonCellWatchAction::Cancel => {
                            let _ = inflight.interrupt().await;
                            let duration_ms = started.elapsed().as_millis() as u64;
                            // Interrupt is best-effort; do not block the tool on Done.
                            // Park the collector so the execute gate still releases.
                            inflight.park(|_id, _result| {});
                            let mut cancelled = snapshot;
                            cancelled.status = "cancelled".into();
                            return Ok(build_ipython_result(
                                cancelled,
                                duration_ms,
                                Some("cancelled by wait policy"),
                            ));
                        }
                        PythonCellWatchAction::Background => {
                            let cell_id = inflight.cell_id().to_string();
                            let duration_ms = started.elapsed().as_millis() as u64;
                            let completion = ctx.python_cell_completion.clone();
                            let session_id = ctx.session_id.to_string();
                            let session_dir = ctx.session_dir.clone();
                            let kernel_for_snap = Arc::clone(kernel);
                            inflight.park(move |completed_id, result| {
                                let status_ok = matches!(&result, Ok(out) if out.status == "ok");
                                if let Some(hook) = completion {
                                    let (status, stdout, stderr, error) = match &result {
                                        Ok(out) => (
                                            out.status.clone(),
                                            output_tail(&out.stdout, PYTHON_CELL_OUTPUT_TAIL_CHARS),
                                            output_tail(&out.stderr, PYTHON_CELL_OUTPUT_TAIL_CHARS),
                                            out.error_value.clone(),
                                        ),
                                        Err(err) => (
                                            "error".into(),
                                            String::new(),
                                            String::new(),
                                            Some(err.to_string()),
                                        ),
                                    };
                                    hook.completed(PythonCellCompletionEvent {
                                        session_id,
                                        cell_id: completed_id,
                                        status,
                                        stdout_tail: stdout,
                                        stderr_tail: stderr,
                                        error,
                                    });
                                }
                                if status_ok {
                                    let dir = session_dir;
                                    let kernel = kernel_for_snap;
                                    tokio::spawn(async move {
                                        persist_kernel_namespace_snapshot(
                                            kernel.as_ref(),
                                            dir.as_deref(),
                                        )
                                        .await;
                                    });
                                }
                            });
                            let provisional = json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!(
                                        "Python cell {cell_id} parked in background after wait budget. \
                                         A completion follow-up will arrive when it finishes."
                                    )
                                }],
                                "details": {
                                    "status": "background",
                                    "cellId": cell_id,
                                    "durationMs": duration_ms,
                                    "stdout": snapshot.stdout,
                                    "stderr": snapshot.stderr,
                                    "result": snapshot.result,
                                    "rationale": decision.rationale,
                                }
                            });
                            return Ok(ToolResult::success(
                                ToolResultContent::Json(provisional),
                                "ipython backgrounded",
                            ));
                        }
                    }
                }
            }
        };

        if ctx.cancel_token.is_cancelled() {
            let _ = kernel.interrupt(None).await;
            return Err(ToolCallError::Cancelled);
        }
        let duration_ms = started.elapsed().as_millis() as u64;
        let status_ok = output.status == "ok";
        let result = build_ipython_result(output, duration_ms, None);
        if status_ok {
            persist_kernel_namespace_snapshot(kernel.as_ref(), ctx.session_dir.as_deref()).await;
        }
        Ok(result)
    }
}

fn build_ipython_result(
    output: devo_kernel::CellOutput,
    duration_ms: u64,
    status_override: Option<&str>,
) -> ToolResult {
    let status = status_override.unwrap_or(output.status.as_str());
    // The TUI IPythonCellComponent expects ToolExecution result
    // shape: `{ content: TextBlock[], details: { status, durationMs, … } }`.
    let mut details = json!({
        "stdout": output.stdout,
        "stderr": output.stderr,
        "result": output.result,
        "status": status,
        "durationMs": duration_ms,
    });
    if !output.diffs.is_empty() {
        details["diffs"] = json!(
            output
                .diffs
                .iter()
                .map(|d| {
                    json!({
                        "path": d.path,
                        "oldStr": d.old_str,
                        "newStr": d.new_str,
                        "startLine": d.start_line,
                    })
                })
                .collect::<Vec<_>>()
        );
    }
    if !output.attachments.is_empty() {
        details["attachments"] = json!(
            output
                .attachments
                .iter()
                .map(|a| {
                    json!({
                        "mimeType": a.mime_type,
                        "path": a.path,
                    })
                })
                .collect::<Vec<_>>()
        );
    }
    if let Some(ename) = output.error_name.as_ref() {
        details["errorEname"] = json!(ename);
        details["error"] = json!({
            "ename": ename,
            "evalue": output.error_value.clone().unwrap_or_default(),
            "traceback": output.traceback.clone(),
        });
    }

    let mut text = String::new();
    if !output.stdout.is_empty() {
        text.push_str(&output.stdout);
    }
    if let Some(result) = &output.result {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(result);
    }
    if !output.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&output.stderr);
    }
    if status != "ok" && !output.traceback.is_empty() {
        let tb = output.traceback.join("\n");
        if !tb.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&tb);
        }
    }
    if text.is_empty() {
        text = format!("(status: {status})");
    }

    let images: Vec<ToolResultImage> = output
        .attachments
        .into_iter()
        .map(|a| ToolResultImage {
            mime_type: a.mime_type,
            data_base64: a.data,
        })
        .collect();

    let content = ToolResultContent::Json(json!({
        "content": [{ "type": "text", "text": text }],
        "details": details,
    }));

    if status != "ok" {
        let mut result = ToolResult::error(
            content,
            if status_override.is_some() {
                "ipython cell cancelled"
            } else {
                "ipython cell failed"
            },
            ToolCallError::InternalError(
                output
                    .error_value
                    .clone()
                    .unwrap_or_else(|| status.to_string()),
            ),
        );
        result.images = images;
        return result;
    }

    let mut result = ToolResult::success(content, "ipython ok");
    result.images = images;
    result
}

/// Ensure a kernel exists for RLM sessions (created on the turn task, not the actor).
///
/// `session_dir` is the session **artifact** directory
/// (`session-artifacts/<id>/` for roots), not the parent of the rollout JSONL.
/// When provided, the kernel receives harness env so `rlm.harness` and
/// `await refine.run()` share the host's local store.
///
/// Build the kernel OS-fence spec on the `rlm-kernel` base profile (design doc
/// §5.1, P4): **no default read** — the kernel reads only the system/
/// interpreter base trees plus the roots granted here (workspace read-only in
/// Plan mode, read-write in Build; temp and session artifacts rw, global
/// harness rw, devo home r). Reads and writes anywhere else are OS-refused,
/// so model code escalates through the rlm front door (`rlm.read`/`rlm.write`)
/// and the approval pipeline instead of the fence silently allowing it.
pub(crate) fn resolve_kernel_fence(
    cwd: &std::path::Path,
    session_dir: Option<&std::path::Path>,
    collaboration_mode: devo_protocol::CollaborationMode,
) -> Option<devo_kernel::KernelFenceSpec> {
    let mut writable_roots = Vec::new();
    if collaboration_mode == devo_protocol::CollaborationMode::Build {
        writable_roots.push(cwd.to_path_buf());
    }
    // Unix's base profile grants shared scratch roots too: retain them here
    // so the Plan overlap check sees every writable root in the actual fence.
    // Windows session kernels instead use owned scratch below the artifact
    // root, avoiding recursive ACL propagation across large shared temp trees.
    if session_dir.is_none() || !cfg!(windows) {
        writable_roots.extend(devo_sandbox::temp_writable_paths());
    }
    if let Some(dir) = session_dir {
        writable_roots.push(dir.to_path_buf());
    }
    // Global harness state is agent-owned memory/prompt/skill data — the same
    // trust class as session artifacts. The kernel harness API writes it
    // directly (`rlm.harness.create_memory(..., global_=True)` and friends),
    // and its own guidance points models there, so refusing it broke every
    // global create/update/delete with EACCES. Create the directory host-side
    // first: fence roots resolve to path-beneath rules against real
    // directories, and a missing root would downgrade the whole fence.
    let global_harness = global_harness_state_dir();
    if std::fs::create_dir_all(&global_harness).is_ok() && global_harness.is_dir() {
        writable_roots.push(global_harness);
    }
    Some(devo_kernel::KernelFenceSpec {
        // Devo home is READ-granted so the kernel can inspect other agents'
        // rollouts/artifacts directly. The grant is composed per-subdirectory
        // instead of `~/.devo` wholesale so credentials (`auth.json` — a
        // root-level file) stay OUTSIDE it; root-level files are not granted
        // at all because fence read roots must be directories (the capability
        // builder maps them to path-beneath rules; a file root would fail
        // resolution and downgrade the whole fence to unfenced).
        readable_roots: {
            let mut roots = devo_home_read_roots();
            if collaboration_mode == devo_protocol::CollaborationMode::Plan {
                roots.push(cwd.to_path_buf());
            }
            roots
        },
        writable_roots,
        deny_read: Vec::new(),
        // RLM kernel fence: network is restricted by default (§5.2 — the fence
        // spec carries the RLM default, not the shell profile's; kernel network
        // goes through the proxy when granted, not through profile inheritance).
        restrict_network: true,
    })
}

/// Read roots inside the Devo home for the kernel fence: every subdirectory.
/// Root-level files (notably `auth.json` with the API keys, but also
/// config.toml / providers.json) are excluded — read roots are directory
/// grants, and credential files must stay unreadable by the kernel.
fn devo_home_read_roots() -> Vec<std::path::PathBuf> {
    let Ok(home) = devo_util_paths::find_devo_home() else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&home) {
        for entry in entries.flatten() {
            let path = entry.path();
            // Follow only real directories (skip dot-directories and
            // non-directory entries such as lock/socket files).
            if path.is_dir() && !path.is_symlink() {
                roots.push(path);
            }
        }
    }
    roots.sort();
    roots
}

/// Host-owned site dir where `dill` is provisioned for the kernel interpreter.
fn kernel_site_dir() -> Option<std::path::PathBuf> {
    devo_util_paths::find_devo_home()
        .ok()
        .map(|home| home.join("kernel-site"))
}

/// Whether the kernel interpreter imports `module` under its PYTHONPATH.
fn interpreter_imports(module: &str, config: &devo_kernel::KernelSessionConfig) -> bool {
    let mut cmd = std::process::Command::new(&config.python);
    cmd.arg("-c").arg(format!("import {module}"));
    if let Ok(paths) = std::env::join_paths(&config.python_path_entries) {
        cmd.env("PYTHONPATH", paths);
    }
    matches!(cmd.output(), Ok(output) if output.status.success())
}

/// Provision `dill` for the kernel: namespace snapshots/restores are
/// host-guaranteed (`repl.py` treats dill as host-provisioned, not a hard
/// dependency). When the interpreter does not already provide it, installs it
/// into `~/.devo/kernel-site` via `uv` (first use only) and prepends that dir
/// to the kernel PYTHONPATH. Failure stays a warning — snapshots degrade, the
/// kernel itself never fails to spawn.
///
/// The install is atomic against concurrent provisioning (two sessions
/// spawning kernels into a fresh Devo home): each racer installs into a
/// private temp dir and renames it into place; the loser keeps the winner's
/// install instead of interleaving two `--target` installs.
fn ensure_kernel_dill(config: &mut devo_kernel::KernelSessionConfig) {
    // Release bundles contain locked dependencies. Never provision over the
    // network or shadow them with a user-site installation on first launch.
    if devo_util_paths::runtime::RuntimeBundle::current().is_some() {
        return;
    }
    let Some(site) = kernel_site_dir() else {
        tracing::warn!(
            "could not resolve the Devo home; kernel namespace snapshots will be skipped \
             (dill unavailable)"
        );
        return;
    };
    if site.join("dill").is_dir() {
        // Steady state: provisioned on an earlier spawn — no probe needed.
        config.python_path_entries.insert(0, site);
        return;
    }
    if interpreter_imports("dill", config) {
        return;
    }
    let Some(parent) = site.parent() else {
        return;
    };
    let staging = parent.join(format!(
        ".kernel-site.tmp.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0),
    ));
    let install = std::process::Command::new("uv")
        .arg("pip")
        .arg("install")
        .arg("--quiet")
        .arg("--target")
        .arg(&staging)
        .arg("dill")
        .output();
    match install {
        Ok(output) if output.status.success() => {
            if std::fs::rename(&staging, &site).is_err() {
                // Lost the race (target exists now) or rename failed: either
                // the winner's install or no install is in place.
                let _ = std::fs::remove_dir_all(&staging);
                if !site.join("dill").is_dir() {
                    tracing::warn!(
                        site = %site.display(),
                        "kernel-site staging rename failed; kernel namespace snapshots \
                         will be skipped (dill unavailable)"
                    );
                    return;
                }
            }
        }
        _ => {
            let _ = std::fs::remove_dir_all(&staging);
            tracing::warn!(
                site = %site.display(),
                "`uv pip install dill` failed; kernel namespace snapshots will be skipped \
                 (dill unavailable)"
            );
            return;
        }
    }
    config.python_path_entries.insert(0, site);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelNamespaceRestoreState {
    Fresh,
    Restored,
}

pub async fn ensure_kernel(
    existing: &Option<Arc<devo_kernel::KernelSession>>,
    cwd: &std::path::Path,
    session_dir: Option<&std::path::Path>,
    collaboration_mode: devo_protocol::CollaborationMode,
) -> Result<Arc<devo_kernel::KernelSession>, ToolCallError> {
    ensure_kernel_with_restore_state(existing, cwd, session_dir, collaboration_mode)
        .await
        .map(|(kernel, _)| kernel)
}

pub async fn ensure_kernel_with_restore_state(
    existing: &Option<Arc<devo_kernel::KernelSession>>,
    cwd: &std::path::Path,
    session_dir: Option<&std::path::Path>,
    collaboration_mode: devo_protocol::CollaborationMode,
) -> Result<
    (
        Arc<devo_kernel::KernelSession>,
        Option<KernelNamespaceRestoreState>,
    ),
    ToolCallError,
> {
    let access = match collaboration_mode {
        devo_protocol::CollaborationMode::Build => devo_kernel::WorkspaceAccess::ReadWrite,
        devo_protocol::CollaborationMode::Plan => devo_kernel::WorkspaceAccess::ReadOnly,
    };
    if let Some(k) = existing {
        let workspace = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        if k.workspace_root() == workspace
            && k.workspace_access() == access
            && (collaboration_mode != devo_protocol::CollaborationMode::Plan
                || k.fence_state() == devo_kernel::FenceState::Fenced)
        {
            return Ok((Arc::clone(k), None));
        }
        // Old Build cells (including parked background cells) must not keep
        // writing after a Plan transition. A new fence cannot restrict an
        // already-running process, even when its Arc is dropped by the actor.
        if let Err(error) = k.shutdown().await
            && collaboration_mode == devo_protocol::CollaborationMode::Plan
        {
            return Err(ToolCallError::InternalError(format!(
                "Plan kernel unavailable: previous kernel shutdown failed: {error}"
            )));
        }
    }
    let startup_started = std::time::Instant::now();
    let mut config = devo_kernel::KernelSessionConfig {
        cwd: cwd.to_path_buf(),
        ..Default::default()
    };
    if let Some(runtime) = resolve_runtime_pythonpath() {
        config.python_path_entries.push(runtime);
    }
    // Prefer installed system skills (`~/.devo/skills/.system/*/src`) over the
    // in-tree `rlm_skills` stubs so API names match bundled SKILL.md.
    config
        .python_path_entries
        .extend(resolve_system_python_skill_src_paths());
    if let Some(skills) = resolve_rlm_skills_pythonpath() {
        config.python_path_entries.push(skills);
    }
    // dill is host-provisioned (see `ensure_kernel_dill`); must ride the
    // PYTHONPATH before the fence derives its interpreter read roots.
    ensure_kernel_dill(&mut config);
    config.extra_env.extend(harness_kernel_env(session_dir));
    if let Some(dir) = session_dir {
        let temp = dir.join("tmp");
        std::fs::create_dir_all(&temp).map_err(|error| {
            ToolCallError::InternalError(format!(
                "Cannot create Python kernel temporary directory {}: {error}",
                temp.display()
            ))
        })?;
        for key in ["TEMP", "TMP", "TMPDIR"] {
            config
                .extra_env
                .push((key.to_string(), temp.to_string_lossy().into_owned()));
        }
    }

    // OS fence (design doc §5, P0): request the `rlm-kernel` sandbox profile
    // for the kernel. The kernel crate owns the explicit-downgrade path when
    // the fence cannot be raised (unprovisioned sandbox, missing platform
    // wiring).
    if let Some(fence) = resolve_kernel_fence(cwd, session_dir, collaboration_mode) {
        if collaboration_mode == devo_protocol::CollaborationMode::Plan {
            // A writable ancestor or descendant overlaps the workspace.
            // Sandbox grant unions cannot turn that path read-only.
            let workspace = std::fs::canonicalize(cwd).map_err(|error| {
                ToolCallError::InternalError(format!(
                    "Plan kernel cannot resolve workspace {}: {error}",
                    cwd.display()
                ))
            })?;
            for root in &fence.writable_roots {
                let writable = std::fs::canonicalize(root).map_err(|error| {
                    ToolCallError::InternalError(format!(
                        "Plan kernel cannot verify writable root {}: {error}",
                        root.display()
                    ))
                })?;
                if workspace.starts_with(&writable) || writable.starts_with(&workspace) {
                    return Err(ToolCallError::InternalError(format!(
                        "Plan kernel unavailable: writable root {} overlaps workspace {}",
                        writable.display(),
                        workspace.display()
                    )));
                }
            }
        }
        config.fence = Some(fence);
    }
    config.workspace_access = access;

    let prepare_ms = startup_started.elapsed().as_millis() as u64;
    let spawn_started = std::time::Instant::now();
    let kernel = devo_kernel::KernelSession::spawn(config)
        .await
        .map_err(|e| ToolCallError::InternalError(e.to_string()))?;
    if collaboration_mode == devo_protocol::CollaborationMode::Plan
        && kernel.fence_state() != devo_kernel::FenceState::Fenced
    {
        return Err(ToolCallError::InternalError(format!(
            "Plan kernel unavailable: OS workspace read-only fence not active ({:?})",
            kernel.fence_state()
        )));
    }
    tracing::info!(
        cwd = %cwd.display(),
        spawn_handshake_ms = spawn_started.elapsed().as_millis() as u64,
        "RLM kernel spawned and handshake completed"
    );

    // Revive prior namespace before skill bootstrap so bootstrap overwrites
    // live handles (rlm, skills) on top of restored user bindings.
    let restore_started = std::time::Instant::now();
    let mut namespace_state = KernelNamespaceRestoreState::Fresh;
    if let Some(dir) = session_dir {
        let dill = kernel_namespace_snapshot_path(dir);
        if dill.is_file() {
            match kernel.restore(&dill).await {
                Ok(out) if out.status == "ok" => {
                    namespace_state = KernelNamespaceRestoreState::Restored;
                    tracing::info!(
                        path = %dill.display(),
                        "restored RLM kernel namespace from session artifact"
                    );
                }
                Ok(out) => {
                    tracing::warn!(
                        path = %dill.display(),
                        status = %out.status,
                        stderr = %out.stderr,
                        "RLM kernel namespace restore finished with non-ok status"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        path = %dill.display(),
                        %error,
                        "RLM kernel namespace restore failed"
                    );
                }
            }
        }
    }

    // Pre-import built-in skill modules so prompts that say they are available match
    // the live kernel (the runtime PYTHONPATHs system skills
    // `src/` dirs plus in-tree `rlm_skills` stubs). Callable skills with `run`
    // are wrapped so `await attach_image(...)` calls `run`.
    let restore_ms = restore_started.elapsed().as_millis() as u64;
    let bootstrap_started = std::time::Instant::now();
    let bootstrap = concat!(
        "import asyncio\n",
        "import rlm\n",
        "import rlm.mcp as mcp\n",
        "import importlib as _devo_importlib\n",
        "import inspect as _devo_inspect\n",
        "import sys as _devo_sys\n",
        "import types as _devo_types\n",
        "class _DevoCallableSkillModule(_devo_types.ModuleType):\n",
        "    async def __call__(self, *args, **kwargs):\n",
        "        result = self.run(*args, **kwargs)\n",
        "        if _devo_inspect.isawaitable(result):\n",
        "            return await result\n",
        "        return result\n",
        "def _devo_wrap_skill_module(module):\n",
        "    run = getattr(module, 'run', None)\n",
        "    if not callable(run):\n",
        "        return module\n",
        "    if isinstance(module, _DevoCallableSkillModule):\n",
        "        return module\n",
        "    wrapped = _DevoCallableSkillModule(module.__name__)\n",
        "    wrapped.__dict__.update(module.__dict__)\n",
        "    try:\n",
        "        wrapped.__signature__ = _devo_inspect.signature(run)\n",
        "    except Exception:\n",
        "        pass\n",
        "    doc = getattr(run, '__doc__', None)\n",
        "    if doc:\n",
        "        wrapped.__doc__ = doc\n",
        "    _devo_sys.modules[module.__name__] = wrapped\n",
        "    return wrapped\n",
        "def _devo_import_skill(name):\n",
        "    try:\n",
        "        module = _devo_wrap_skill_module(_devo_importlib.import_module(name))\n",
        "        globals()[name] = module\n",
        "        return module\n",
        "    except Exception as err:\n",
        "        globals()[f'_devo_{name}_import_error'] = str(err)\n",
        "        return None\n",
        "for _devo_skill in (\n",
        "    'compact', 'refine', 'goal', 'agent_observe', 'agent_message',\n",
        "    'edit', 'websearch', 'rlm_heartbeat', 'linear', 'notion', 'attach_image',\n",
        "):\n",
        "    _devo_import_skill(_devo_skill)\n",
        "try:\n",
        "    from rlm.bash import bash  # noqa: F401\n",
        "except ImportError:\n",
        "    pass\n",
    );
    if let Err(error) = kernel.execute(bootstrap).await {
        tracing::warn!(%error, "RLM skill bootstrap import failed");
    }
    tracing::info!(
        cwd = %cwd.display(),
        prepare_ms,
        restore_ms,
        bootstrap_ms = bootstrap_started.elapsed().as_millis() as u64,
        ensure_kernel_total_ms = startup_started.elapsed().as_millis() as u64,
        "RLM kernel ready (prepare + spawn + restore + skill bootstrap)"
    );

    Ok((kernel, Some(namespace_state)))
}

/// Canonical dill path under a session artifact directory.
pub fn kernel_namespace_snapshot_path(session_dir: &std::path::Path) -> PathBuf {
    session_dir.join("kernel.dill")
}

/// Canonical JSON manifest path next to [`kernel_namespace_snapshot_path`].
pub fn kernel_namespace_manifest_path(session_dir: &std::path::Path) -> PathBuf {
    session_dir.join("kernel.json")
}

async fn persist_kernel_namespace_snapshot(
    kernel: &devo_kernel::KernelSession,
    session_dir: Option<&std::path::Path>,
) {
    let Some(dir) = session_dir else {
        return;
    };
    if let Err(error) = std::fs::create_dir_all(dir) {
        tracing::warn!(%error, path = %dir.display(), "could not create session artifact dir for kernel snapshot");
        return;
    }
    let dill = kernel_namespace_snapshot_path(dir);
    let manifest = kernel_namespace_manifest_path(dir);
    match kernel.snapshot(&dill, &manifest).await {
        Ok(out) if out.status == "ok" => {
            tracing::debug!(
                path = %dill.display(),
                "persisted RLM kernel namespace snapshot"
            );
        }
        Ok(out) => {
            // repl.py reports snapshot errors (e.g. "dill unavailable") in the
            // result payload carried by stdout — include it, stderr alone has
            // hid the real cause before.
            let stdout = out.stdout.trim();
            let detail: String = if !stdout.is_empty() {
                stdout.chars().take(300).collect()
            } else if !out.stderr.trim().is_empty() {
                out.stderr.trim().chars().take(300).collect()
            } else {
                out.error_value
                    .unwrap_or_default()
                    .chars()
                    .take(300)
                    .collect()
            };
            tracing::warn!(
                path = %dill.display(),
                status = %out.status,
                detail = %detail,
                "RLM kernel namespace snapshot finished with non-ok status"
            );
        }
        Err(error) => {
            tracing::warn!(
                path = %dill.display(),
                %error,
                "RLM kernel namespace snapshot failed"
            );
        }
    }
}

fn harness_kernel_env(session_dir: Option<&std::path::Path>) -> Vec<(String, String)> {
    let mut env = Vec::new();
    let global = global_harness_state_dir();
    env.push((
        "RLM_GLOBAL_HARNESS_STATE_DIR".into(),
        global.display().to_string(),
    ));
    if let Some(dir) = session_dir {
        let _ = std::fs::create_dir_all(dir.join("harness"));
        env.push(("RLM_SESSION_DIR".into(), dir.display().to_string()));
        env.push((
            "RLM_HARNESS_STATE_DIR".into(),
            dir.join("harness").display().to_string(),
        ));
    }
    // Skills (websearch/linear/notion) and `rlm.harness` read auth from this
    // dir. Always point at Devo home — not only when DEVO_HOME is set — so
    // `~/.devo/auth.json` is used under InteractiveMode.
    let agent_dir = std::env::var_os("DEVO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|h| PathBuf::from(h).join(".devo"))
        })
        .unwrap_or_else(|| PathBuf::from(".devo"));
    env.push((
        "DEVO_CODING_AGENT_DIR".into(),
        agent_dir.display().to_string(),
    ));
    env
}

pub(crate) fn global_harness_state_dir() -> PathBuf {
    let home = std::env::var_os("DEVO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|h| PathBuf::from(h).join(".devo"))
        })
        .unwrap_or_else(|| PathBuf::from(".devo"));
    home.join("harness")
}

fn resolve_rlm_skills_pythonpath() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidate = manifest.join("rlm_skills");
    if candidate.join("refine").join("__init__.py").is_file() {
        return Some(candidate);
    }
    None
}

/// `~/.devo/skills/.system/<skill>/src` entries that contain a Python package.
fn resolve_system_python_skill_src_paths() -> Vec<PathBuf> {
    let Some(system_root) = resolve_system_skills_root() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&system_root) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for entry in entries.flatten() {
        let src = entry.path().join("src");
        if !src.is_dir() {
            continue;
        }
        // Require at least one package `__init__.py` under src/.
        let has_package = std::fs::read_dir(&src).ok().is_some_and(|children| {
            children.flatten().any(|child| {
                let p = child.path();
                p.is_dir() && p.join("__init__.py").is_file()
            })
        });
        if has_package {
            paths.push(src);
        }
    }
    paths.sort();
    paths
}

fn resolve_system_skills_root() -> Option<PathBuf> {
    let home = std::env::var_os("DEVO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|h| PathBuf::from(h).join(".devo"))
        })?;
    let root = home.join("skills").join(".system");
    root.is_dir().then_some(root)
}

fn resolve_runtime_pythonpath() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("DEVO_RLM_RUNTIME_SRC") {
        let p = std::path::PathBuf::from(path);
        if p.join("rlm").is_dir() {
            return Some(p);
        }
    }
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for base in [&manifest, manifest.parent().unwrap_or(&manifest)] {
        if let Some(p) = devo_kernel::default_runtime_pythonpath(base) {
            return Some(p);
        }
        if let Some(parent) = base.parent()
            && let Some(p) = devo_kernel::default_runtime_pythonpath(parent)
        {
            return Some(p);
        }
    }
    devo_kernel::default_runtime_pythonpath(std::path::Path::new("."))
}

#[cfg(test)]
#[path = "kernel_startup_tests.rs"]
mod kernel_startup_tests;
