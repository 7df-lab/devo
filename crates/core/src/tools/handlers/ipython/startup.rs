//! Trusted product Python startup, namespace persistence, and lifecycle.

use std::path::PathBuf;
use std::sync::Arc;

use crate::contracts::ToolCallError;

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
    use crate::{AppConfigLoader, SkillCatalog};
    let home = devo_util_paths::find_devo_home()
        .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
    let config = crate::FileSystemAppConfigLoader::new(home.clone())
        .load(Some(cwd))
        .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
    let skills = crate::FileSystemSkillCatalog::with_devo_home(
        config.skills,
        home,
        cwd.to_path_buf(),
        config.project_root_markers,
    )
    .python_skills(Some(cwd))
    .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
    ensure_kernel_with_skills(existing, cwd, session_dir, collaboration_mode, &skills)
        .await
        .map(|(kernel, state, _)| (kernel, state))
}

/// Product startup consumes the same enabled catalog selection used for context.
/// Refreshes bindings at a turn boundary without restarting the session kernel.
pub async fn ensure_kernel_with_skills(
    existing: &Option<Arc<devo_kernel::KernelSession>>,
    cwd: &std::path::Path,
    session_dir: Option<&std::path::Path>,
    collaboration_mode: devo_protocol::CollaborationMode,
    skills: &[crate::skills::PythonSkill],
) -> Result<
    (
        Arc<devo_kernel::KernelSession>,
        Option<KernelNamespaceRestoreState>,
        Vec<String>,
    ),
    ToolCallError,
> {
    let access = match collaboration_mode {
        devo_protocol::CollaborationMode::Build => devo_kernel::WorkspaceAccess::ReadWrite,
        devo_protocol::CollaborationMode::Plan => devo_kernel::WorkspaceAccess::ReadOnly,
    };
    if let Some(k) = existing {
        let workspace = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        if k.workspace_root() == workspace && k.workspace_access() == access {
            let imports = sync_kernel_skills(k, skills).await?;
            return Ok((Arc::clone(k), None, imports));
        }
        // Retire the previous process and its background cells when workspace
        // or mode changes. Plan is host policy, not an OS Python restriction.
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
    // dill is host-provisioned for namespace snapshots and restore.
    ensure_kernel_dill(&mut config);
    config.extra_env.extend(kernel_environment(session_dir));
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

    config.workspace_access = access;

    let prepare_ms = startup_started.elapsed().as_millis() as u64;
    let spawn_started = std::time::Instant::now();
    let kernel = devo_kernel::KernelSession::spawn(config)
        .await
        .map_err(|e| ToolCallError::InternalError(e.to_string()))?;
    tracing::info!(
        cwd = %cwd.display(),
        spawn_handshake_ms = spawn_started.elapsed().as_millis() as u64,
        "RLM kernel spawned and handshake completed"
    );

    // Register the exact selected packages before dill resolves module/function
    // references. Ambient PYTHONPATH must not choose a different Skill identity.
    let _ = sync_kernel_skills(&kernel, skills).await?;
    // Restore user aliases, then reapply the selected live bindings below.
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

    let restore_ms = restore_started.elapsed().as_millis() as u64;
    let bootstrap_started = std::time::Instant::now();
    let imports = sync_kernel_skills(&kernel, skills).await?;
    tracing::info!(
        cwd = %cwd.display(),
        prepare_ms,
        restore_ms,
        bootstrap_ms = bootstrap_started.elapsed().as_millis() as u64,
        ensure_kernel_total_ms = startup_started.elapsed().as_millis() as u64,
        "RLM kernel ready (prepare + spawn + restore + skill bootstrap)"
    );

    Ok((kernel, Some(namespace_state), imports))
}

async fn sync_kernel_skills(
    kernel: &devo_kernel::KernelSession,
    skills: &[crate::skills::PythonSkill],
) -> Result<Vec<String>, ToolCallError> {
    // Serialize twice: JSON is data embedded in a Python string, not Python source.
    let manifest = serde_json::to_string(skills)
        .and_then(|json| serde_json::to_string(&json))
        .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
    let bootstrap = format!(
        "import asyncio\nimport rlm\nimport rlm.mcp as mcp\nfrom rlm.bash import bash\n\
         import json as _devo_json\nfrom rlm._skill_bootstrap import sync_skills as _devo_sync_skills\n\
         print(_devo_json.dumps(_devo_sync_skills(globals(), _devo_json.loads({manifest}))))\n"
    );
    let Some(output) = kernel
        .try_execute(&bootstrap)
        .await
        .map_err(|error| ToolCallError::InternalError(error.to_string()))?
    else {
        // A parked cell owns the stream. Do not delay the next model turn or
        // replace its host handler; retry the selection at an idle boundary.
        // Conservatively omit pre-import claims until that refresh succeeds.
        return Ok(Vec::new());
    };
    if output.status != "ok" {
        return Err(ToolCallError::InternalError(format!(
            "RLM bootstrap failed: {}",
            output.stderr
        )));
    }
    serde_json::from_str(output.stdout.lines().last().unwrap_or("")).map_err(|error| {
        ToolCallError::InternalError(format!("invalid RLM skill bootstrap response: {error}"))
    })
}

/// Canonical dill path under a session artifact directory.
pub fn kernel_namespace_snapshot_path(session_dir: &std::path::Path) -> PathBuf {
    session_dir.join("kernel.dill")
}

/// Canonical JSON manifest path next to [`kernel_namespace_snapshot_path`].
pub fn kernel_namespace_manifest_path(session_dir: &std::path::Path) -> PathBuf {
    session_dir.join("kernel.json")
}

pub(super) async fn persist_kernel_namespace_snapshot(
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

fn kernel_environment(session_dir: Option<&std::path::Path>) -> Vec<(String, String)> {
    let mut env = Vec::new();
    if let Some(dir) = session_dir {
        env.push(("RLM_SESSION_DIR".into(), dir.display().to_string()));
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
