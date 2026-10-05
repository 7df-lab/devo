//! OS fence wiring for the RLM kernel.
//!
//! Windows: the kernel argv is wrapped in the restricted-token sandbox launcher
//! (`devo.exe --run-as-windows-sandbox ... python -m rlm.repl`). Linux/macOS
//! enforcement uses a child sandbox plan when available. If enforcement is
//! unavailable, a fence request is an explicit downgrade, never a silent one.
//!
//! Downgrade semantics: if the sandbox is not provisioned — or the
//! wrapped launch fails — the kernel still spawns unfenced, but the downgrade is
//! loud (`tracing::warn`) and the caller is expected to surface it in the UI and
//! record a `fence-off` event. The one hard exception stays with the approval
//! pipeline: profiles with deny-read paths never drop their sandbox.

#[cfg(any(windows, test))]
use crate::session::KernelFenceSpec;
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

/// Pure decision for the fence path — testable without touching the machine's
/// provisioning state. (Windows path; the unix branch decides structurally.)
#[cfg(any(windows, test))]
pub(crate) fn decide_fence(requested: Option<&KernelFenceSpec>, sandbox_ready: bool) -> FenceState {
    match requested {
        None => FenceState::NotRequested,
        Some(_) if sandbox_ready => FenceState::Fenced,
        Some(_) => FenceState::DowngradedUnfenced,
    }
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
/// Python runtime prefix (`<prefix>/bin/python*` layout) plus every PYTHONPATH
/// entry (vendored rlm-runtime, skills). Without these a no-default-read fence
/// would break interpreter startup.
#[cfg(unix)]
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
        && let Some(prefix) = python.parent().and_then(|bin| bin.parent())
        && prefix.is_absolute()
    {
        roots.push(prefix.to_path_buf());
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
) -> FenceOutcome {
    let Some(spec) = config.fence.as_ref() else {
        return FenceOutcome {
            command: bare,
            state: FenceState::NotRequested,
            credentials: None,
        };
    };
    let devo_home = match devo_util_paths::find_devo_home() {
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
    let ready = devo_windows_sandbox::sandbox_setup_is_complete(&devo_home);
    match decide_fence(config.fence.as_ref(), ready) {
        FenceState::NotRequested => FenceOutcome {
            command: bare,
            state: FenceState::NotRequested,
            credentials: None,
        },
        FenceState::DowngradedUnfenced => {
            tracing::warn!(
                "RLM kernel fence requested but the Windows sandbox is not provisioned; \
                 kernel runs UNFENCED with full user permissions (explicit downgrade). \
                 Run the Windows sandbox setup to enable the fence."
            );
            FenceOutcome {
                command: bare,
                state: FenceState::DowngradedUnfenced,
                credentials: None,
            }
        }
        FenceState::Fenced => {
            // P2: mint the per-session credential authority. Its SID travels in
            // the kernel's restricted token (identity marker, no access by
            // itself); the retained authority delivers later grants as ACEs.
            let session_id = format!("kernel-{}", uuid::Uuid::new_v4());
            // The SID is only recoverable from the running token, so log it:
            // crash recovery, manual grants, and audits all key off it.
            tracing::info!(session_id = %session_id, "RLM kernel session credential pending mint");
            let credentials = match devo_windows_sandbox::SessionCredentialAuthority::new(
                &session_id,
                &devo_home,
            ) {
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
                readable_roots: spec.readable_roots.clone(),
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
            let inherited_env = std::env::vars()
                .filter(|(key, value)| !crate::session::is_credential_env_var(key, value))
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
    }
}

#[cfg(test)]
mod tests {
    use super::FenceState;
    use super::decide_fence;
    use crate::session::KernelFenceSpec;
    use std::path::PathBuf;

    fn spec() -> KernelFenceSpec {
        KernelFenceSpec {
            readable_roots: vec![PathBuf::from(r"C:\proj")],
            writable_roots: vec![PathBuf::from(r"C:\proj")],
            deny_read: Vec::new(),
            restrict_network: true,
        }
    }

    #[test]
    fn no_request_spawns_bare_by_design() {
        assert_eq!(decide_fence(None, false), FenceState::NotRequested);
        assert_eq!(decide_fence(None, true), FenceState::NotRequested);
    }

    #[test]
    fn provisioned_sandbox_fences() {
        assert_eq!(decide_fence(Some(&spec()), true), FenceState::Fenced);
    }

    #[test]
    fn unprovisioned_sandbox_is_an_explicit_downgrade_never_silent() {
        assert_eq!(
            decide_fence(Some(&spec()), false),
            FenceState::DowngradedUnfenced
        );
    }
}
