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
    let access = match collaboration_mode {
        devo_protocol::CollaborationMode::Build => devo_kernel::WorkspaceAccess::ReadWrite,
        devo_protocol::CollaborationMode::Plan => devo_kernel::WorkspaceAccess::ReadOnly,
    };
    if let Some(k) = existing {
        let workspace = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        if k.workspace_root() == workspace && k.workspace_access() == access {
            return Ok((Arc::clone(k), None));
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
    // Prefer installed system skills (`~/.devo/skills/.system/*/src`) over the
    // in-tree `rlm_skills` stubs so API names match bundled SKILL.md.
    config
        .python_path_entries
        .extend(resolve_system_python_skill_src_paths());
    if let Some(skills) = resolve_rlm_skills_pythonpath() {
        config.python_path_entries.push(skills);
    }
    // dill is host-provisioned for namespace snapshots and restore.
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
