//! OS fence wiring for the RLM kernel.
//!
//! Windows: the kernel argv is wrapped in the restricted-token sandbox launcher
//! (`devo.exe --run-as-windows-sandbox ... python -m rlm.repl`). Linux/macOS
//! enforcement uses a child sandbox plan when available. If enforcement is
//! unavailable, a fence request is an explicit downgrade, never a silent one.
//!
//! Windows provisioning is performed by the wrapper before Python starts; a
//! missing setup marker must never bypass that wrapper. If preparing the
//! wrapped launch fails, the downgrade is loud (`tracing::warn`) and the caller
//! is expected to surface it in the UI and
//! record a `fence-off` event. The one hard exception stays with the approval
//! pipeline: profiles with deny-read paths never drop their sandbox.

#[cfg(windows)]
use std::process::Stdio;
use tokio::process::Command;

/// Outcome of the fence request for a spawned kernel — recorded on the
/// session so callers (core/server) can emit rollout events and UI warnings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceState {
    /// No fence requested; spawn bare by design.
    NotRequested,
    /// Sandbox provisioned; kernel argv wrapped in the sandbox launcher.
    Fenced,
    /// Fence requested but unavailable; kernel runs unfenced with an explicit,
    /// logged downgrade (never silent).
    DowngradedUnfenced,
}

/// Result of the fence application: the launch command, the decision, and —
/// on Windows when fenced — the per-session credential authority whose SID
/// traveled in the kernel's restricted token. Unix platforms carry
/// no authority yet (dirfd delivery channel is follow-up work).
pub(crate) struct FenceOutcome {
    pub command: Command,
    pub state: FenceState,
    #[cfg(windows)]
    pub credentials: Option<devo_windows_sandbox::SessionCredentialAuthority>,
    /// Unix pipe-mode enforcement plan (Landlock + seccomp), applied in the
    /// child via `pre_exec` — the same mechanism the product's shell sandbox
    /// uses when no outer wrapper carries the policy.
    #[cfg(unix)]
    pub child_plan: Option<devo_util_process::sandbox::ResolvedEnforcementPlan>,
    /// Unix credential delivery channel host end (fenced kernels only):
    /// grants travel as fds over SCM_RIGHTS. `None` when unfenced.
    #[cfg(unix)]
    pub grant_channel: Option<crate::credential_unix::GrantChannel>,
}

/// Read roots the interpreter itself needs (`python_ro` profile): the
/// Python runtime prefix (`<prefix>/bin/python*` on Unix, `<prefix>/python.exe`
/// on Windows) plus every PYTHONPATH entry (vendored rlm-runtime, skills).
/// Private Windows installs under AppData need these explicit read/execute
/// grants just as Unix's no-default-read fence needs interpreter access.
fn interpreter_read_roots(config: &crate::session::KernelSessionConfig) -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    let python = if config.python.is_absolute() {
        Some(config.python.clone())
    } else {
        // Bare interpreter name: resolve via PATH so the runtime prefix can be
        // granted (the fence must be able to see the binary it execs).
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(&config.python))
                .find(|candidate| candidate.is_absolute() && candidate.is_file())
        })
    };
    if let Some(python) = python
        && let Some(parent) = python.parent()
    {
        #[cfg(unix)]
        let prefix = parent.parent();
        #[cfg(windows)]
        let prefix = Some(parent);
        if let Some(prefix) = prefix
            && prefix.is_absolute()
        {
            roots.push(prefix.to_path_buf());
        }
    }
    roots.extend(config.python_path_entries.iter().cloned());
    roots
}

/// Unix fence: enforcement is entirely the Landlock/seccomp
/// child plan applied in `pre_exec` on the kernel process itself — the same
/// mechanism the product's pipe-mode shell sandbox uses. No outer wrapper:
/// a PipeComposed bwrap only adds read-deny bind-overs (rlm-kernel has none)
/// and network plumbing, and applying the Landlock plan to the bwrap parent
/// breaks its user-namespace setup (uid_map writes are refused). A missing
/// plan is an explicit downgrade, never a silent one.
#[cfg(unix)]
pub(super) fn fence_or_bare(
    config: &crate::session::KernelSessionConfig,
    bare: Command,
) -> FenceOutcome {
    let Some(spec) = config.fence.as_ref() else {
        return FenceOutcome {
            command: bare,
            state: FenceState::NotRequested,
            child_plan: None,
            grant_channel: None,
        };
    };
    let overlay = devo_sandbox::SandboxPermissionOverlay {
        read_paths: {
            let mut roots = interpreter_read_roots(config);
            roots.extend(spec.readable_roots.iter().cloned());
            roots
        },
        write_paths: spec.writable_roots.clone(),
        // `Unchanged` keeps the rlm-kernel profile's network restriction;
        // `Enabled` would opt the kernel OUT of it.
        network: if spec.restrict_network {
            devo_sandbox::SandboxNetworkPermission::Unchanged
        } else {
            devo_sandbox::SandboxNetworkPermission::Enabled
        },
    };
    // `rlm-kernel` has no default read: the fence
    // derives from the session's permission state, so reads outside the
    // granted roots are OS-refused and `rlm.read` escalates to the host
    // approval pipeline instead of silently succeeding.
    match devo_util_process::sandbox::resolve_profile_for_spawn_with_overlay(
        Some("rlm-kernel"),
        &config.cwd,
        Some(&overlay),
    ) {
        Ok(Some(plan)) => FenceOutcome {
            command: bare,
            state: FenceState::Fenced,
            child_plan: Some(plan),
            grant_channel: crate::credential_unix::GrantChannel::new().ok(),
        },
        Ok(None) => {
            tracing::warn!(
                "RLM kernel fence requested but no enforcement plan resolved; kernel runs \
                 UNFENCED with full user permissions (explicit downgrade)"
            );
            FenceOutcome {
                command: bare,
                state: FenceState::DowngradedUnfenced,
                child_plan: None,
                grant_channel: None,
            }
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "RLM kernel fence enforcement plan resolution failed; kernel runs UNFENCED \
                 with full user permissions (explicit downgrade)"
            );
            FenceOutcome {
                command: bare,
                state: FenceState::DowngradedUnfenced,
                child_plan: None,
                grant_channel: None,
            }
        }
    }
}

#[cfg(windows)]
pub(super) fn wrap_or_bare(
    config: &crate::session::KernelSessionConfig,
    bare: Command,
    devo_home: std::io::Result<std::path::PathBuf>,
) -> FenceOutcome {
    let Some(spec) = config.fence.as_ref() else {
        return FenceOutcome {
            command: bare,
            state: FenceState::NotRequested,
            credentials: None,
        };
    };
    let devo_home = match devo_home {
        Ok(home) => home,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "RLM kernel fence requested but devo home is unavailable; kernel runs UNFENCED \
                 with full user permissions (explicit downgrade)"
            );
            return FenceOutcome {
                command: bare,
                state: FenceState::DowngradedUnfenced,
                credentials: None,
            };
        }
    };
    // The wrapper provisions missing/outdated sandbox accounts with UAC consent
    // and refreshes this request's exact ACL roots before starting Python. Do
    // not gate it on setup markers: that skips first-run setup and runs Python
    // with full user permissions. A failed/canceled setup fails the handshake.
    // P2: mint the per-session credential authority. Its SID travels in
    // the kernel's restricted token (identity marker, no access by
    // itself); the retained authority delivers later grants as ACEs.
    let session_id = format!("kernel-{}", uuid::Uuid::new_v4());
    // The SID is only recoverable from the running token, so log it:
    // crash recovery, manual grants, and audits all key off it.
    tracing::info!(session_id = %session_id, "RLM kernel session credential pending mint");
    let credentials =
        match devo_windows_sandbox::SessionCredentialAuthority::new(&session_id, &devo_home) {
            Ok(authority) => authority,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "RLM kernel session credential authority could not be minted; \
                     kernel runs UNFENCED (explicit downgrade)"
                );
                return FenceOutcome {
                    command: bare,
                    state: FenceState::DowngradedUnfenced,
                    credentials: None,
                };
            }
        };
    tracing::info!(
        session_id = %session_id,
        sid = %credentials.sid(),
        "RLM kernel session credential minted"
    );
    let request = devo_windows_sandbox::WindowsSandboxRequest {
        // Shell-shaped fields are ignored by the direct-argv launcher.
        command: String::new(),
        shell_program: String::new(),
        shell_args: Vec::new(),
        cwd: config.cwd.clone(),
        readable_roots: {
            let mut roots = interpreter_read_roots(config);
            roots.extend(spec.readable_roots.iter().cloned());
            roots
        },
        writable_roots: spec.writable_roots.clone(),
        deny_read: spec.deny_read.clone(),
        restrict_network: spec.restrict_network,
        session_credential_sid: Some(credentials.sid().to_string()),
        env_extra: crate::session::kernel_env_overrides(config),
    };
    let argv = vec![
        config.python.to_string_lossy().into_owned(),
        "-m".to_string(),
        "rlm.repl".to_string(),
    ];
    // The sandbox wrapper serializes the child's environment into
    // argv before `cmd.env_remove` can act. Filter the snapshot before
    // it enters either the wrapper argv or the eventual Python kernel.
    let installed_bundle = devo_util_paths::runtime::RuntimeBundle::current().is_some();
    let inherited_env = std::env::vars()
        .filter(|(key, value)| !crate::session::is_credential_env_var(key, value))
        .filter(|(key, _)| {
            !installed_bundle
                || !devo_util_paths::runtime_environment::is_interpreter_env_var(
                    key.as_ref(),
                    devo_util_paths::runtime_environment::Interpreter::Python,
                )
        })
        .collect();
    match devo_windows_sandbox::prepare_windows_sandbox_launch_for_argv_with_env(
        &request,
        argv,
        inherited_env,
    ) {
        Ok(launch) => {
            let mut cmd = Command::new(launch.program);
            cmd.args(launch.args)
                .current_dir(&config.cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            for (key, value) in &launch.env {
                cmd.env(key, value);
            }
            FenceOutcome {
                command: cmd,
                state: FenceState::Fenced,
                credentials: Some(credentials),
            }
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "RLM kernel fence launch failed; kernel runs UNFENCED with full user \
                 permissions (explicit downgrade)"
            );
            FenceOutcome {
                command: bare,
                state: FenceState::DowngradedUnfenced,
                credentials: None,
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::{FenceState, wrap_or_bare};
    use crate::session::{KernelFenceSpec, KernelSessionConfig};
    use pretty_assertions::assert_eq;
    use tokio::process::Command;

    #[test]
    fn no_request_spawns_bare_without_a_home() {
        let outcome = wrap_or_bare(
            &KernelSessionConfig::default(),
            Command::new("python"),
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no home")),
        );
        assert_eq!(outcome.state, FenceState::NotRequested);
        assert_eq!(outcome.command.as_std().get_program(), "python");
        assert_eq!(outcome.credentials.is_some(), false);
    }

    #[test]
    fn first_run_uses_the_setup_capable_wrapper_instead_of_bare_python() {
        let home = tempfile::tempdir().expect("fresh unprovisioned home");
        assert_eq!(
            devo_windows_sandbox::sandbox_setup_is_complete(home.path()),
            false,
        );
        let config = KernelSessionConfig {
            cwd: home.path().to_path_buf(),
            fence: Some(KernelFenceSpec {
                readable_roots: vec![home.path().to_path_buf()],
                writable_roots: vec![home.path().to_path_buf()],
                deny_read: Vec::new(),
                restrict_network: true,
            }),
            ..KernelSessionConfig::default()
        };
        let outcome = wrap_or_bare(
            &config,
            Command::new(&config.python),
            Ok(home.path().to_path_buf()),
        );
        assert_eq!(outcome.state, FenceState::Fenced);
        assert_eq!(outcome.credentials.is_some(), true);
        let args: Vec<_> = outcome.command.as_std().get_args().collect();
        assert_eq!(args[0], "--run-as-windows-sandbox");
        assert_eq!(args.contains(&std::ffi::OsStr::new("elevated")), true);
        // Preparing argv is side-effect free; provisioning happens in the
        // wrapper child before it launches the interpreter.
        assert_eq!(
            devo_windows_sandbox::sandbox_setup_is_complete(home.path()),
            false,
        );
    }

    #[test]
    fn unavailable_home_remains_an_explicit_downgrade() {
        let config = KernelSessionConfig {
            fence: Some(KernelFenceSpec {
                readable_roots: Vec::new(),
                writable_roots: Vec::new(),
                deny_read: Vec::new(),
                restrict_network: true,
            }),
            ..KernelSessionConfig::default()
        };
        let outcome = wrap_or_bare(
            &config,
            Command::new(&config.python),
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no home")),
        );
        assert_eq!(outcome.state, FenceState::DowngradedUnfenced);
        assert_eq!(outcome.command.as_std().get_program(), &config.python);
        assert_eq!(outcome.credentials.is_some(), false);
    }
}

#[cfg(test)]
mod interpreter_tests {
    use super::interpreter_read_roots;
    use crate::session::KernelSessionConfig;
    use pretty_assertions::assert_eq;

    #[test]
    fn private_runtime_grants_are_limited_to_interpreter_and_pythonpath() {
        let installation = tempfile::tempdir().expect("private install");
        let prefix = installation.path().join("runtime/python");
        #[cfg(windows)]
        let python = prefix.join("python.exe");
        #[cfg(unix)]
        let python = prefix.join("bin/python3");
        let python_site = installation.path().join("runtime/python-site");
        let config = KernelSessionConfig {
            python,
            python_path_entries: vec![python_site.clone()],
            ..KernelSessionConfig::default()
        };
        assert_eq!(interpreter_read_roots(&config), vec![prefix, python_site]);
    }
}
