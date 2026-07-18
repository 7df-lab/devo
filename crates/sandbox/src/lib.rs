//! OS-level sandboxing for Devo via [nono](https://crates.io/crates/nono).
//!
//! Applied once at process startup. Covers in-process `tokio::fs` calls and
//! child processes. Network is left open at the process level (the agent needs
//! LLM API access); child network is blocked per-subprocess via seccomp.
//!
//! The `enforce` feature (on by default) pulls in `nono` for kernel-enforced
//! sandboxing (Landlock/Seatbelt). When disabled, the crate still provides
//! lightweight helpers that compile on all targets including musl.
//!
//! ```rust,no_run
//! use devo_sandbox::{ProfileName, SandboxManager};
//! use std::path::Path;
//!
//! let workspace = Path::new("/home/user/project");
//! let mut sandbox = SandboxManager::new(ProfileName::Workspace, workspace);
//! sandbox
//!     .apply_required(workspace)
//!     .expect("required sandbox enforcement failed");
//! sandbox.install();
//! ```
mod bwrap;
pub mod child_net;
mod deny;
mod logging;
mod network_policy;
mod paths;
mod profiles;
mod types;

#[cfg(target_os = "linux")]
pub use bwrap::bwrap_reexec_for_profile;
pub use bwrap::{
    bwrap_reexec_command, is_inside_bwrap, requires_read_deny, trust_bwrap_marker_for_devbox,
};
pub use logging::SandboxLogger;
pub use network_policy::{
    ChildNetworkPolicy, NETWORK_POLICY_SNAPSHOT_VERSION, NetworkPolicySnapshot,
    NetworkPolicySnapshotError, WebsiteAction, WebsiteOrigin, WebsiteOriginError, WebsitePolicy,
};
pub use profiles::{
    ProfileName, SandboxConfig, SandboxProfile, load_sandbox_config, sandbox_profile_conflicts,
};
pub use types::{SandboxEvent, SandboxEventType, SandboxMetrics};

#[cfg(all(feature = "enforce", unix))]
use nono::Sandbox;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

static SANDBOX: OnceLock<GlobalSandboxState> = OnceLock::new();
static CONFIGURED_PROFILE: OnceLock<String> = OnceLock::new();
static AUTO_ALLOW_BASH: AtomicBool = AtomicBool::new(false);

struct GlobalSandboxState {
    profile: String,
    logger: SandboxLogger,
    applied: bool,
    restrict_network_at_known_linux_launches: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyRequirement {
    Graceful,
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ApplyOutcome {
    Disabled,
    #[cfg_attr(not(all(feature = "enforce", unix)), allow(dead_code))]
    Applied,
    Unavailable(String),
}

fn validate_apply_outcome(
    profile: &ProfileName,
    requirement: ApplyRequirement,
    outcome: ApplyOutcome,
) -> anyhow::Result<()> {
    match (requirement, outcome) {
        (_, ApplyOutcome::Disabled | ApplyOutcome::Applied)
        | (ApplyRequirement::Graceful, ApplyOutcome::Unavailable(_)) => Ok(()),
        (ApplyRequirement::Required, ApplyOutcome::Unavailable(details)) => anyhow::bail!(
            "sandbox profile '{profile}' requires enforcement, but it was not applied: {details}"
        ),
    }
}

fn restrict_network_at_known_linux_launches(applied: bool, configured: bool) -> bool {
    applied && configured && cfg!(target_os = "linux")
}

/// Whether known Linux child launch paths should install the seccomp network filter.
pub fn should_restrict_child_network() -> bool {
    SANDBOX
        .get()
        .is_some_and(|state| state.restrict_network_at_known_linux_launches)
}

/// Whether bash commands should be auto-approved when the sandbox is active.
pub fn should_auto_allow_bash() -> bool {
    AUTO_ALLOW_BASH.load(Ordering::Relaxed) && is_active()
}

pub fn set_auto_allow_bash(enabled: bool) {
    AUTO_ALLOW_BASH.store(enabled, Ordering::Relaxed);
}

/// Record the resolved sandbox profile at process startup (including `"off"`).
pub fn set_configured_profile(name: impl Into<String>) {
    let _ = CONFIGURED_PROFILE.set(name.into());
}

/// Resolved sandbox profile from startup, or `None` if it was never set.
pub fn configured_profile_name() -> Option<&'static str> {
    CONFIGURED_PROFILE.get().map(|name| name.as_str())
}

/// Whether the sandbox was successfully applied to this process.
pub fn is_active() -> bool {
    SANDBOX.get().is_some_and(|state| state.applied)
}

/// The active sandbox profile name, or `None` if sandbox is not applied.
pub fn profile_name() -> Option<&'static str> {
    SANDBOX
        .get()
        .filter(|state| state.applied)
        .map(|state| state.profile.as_str())
}

/// Log a sandbox violation. Immediately flushed to disk. No-op if inactive.
pub fn log_violation(target: &str, operation: &str) {
    if let Some(state) = SANDBOX.get() {
        state.logger.log(SandboxEvent::fs_violation(
            &state.profile,
            target,
            operation,
        ));
        let _ = state.logger.flush_to_disk();
    }
}

/// Flush sandbox events to disk. No-op if not initialized.
pub fn flush() {
    if let Some(state) = SANDBOX.get()
        && let Err(error) = state.logger.flush_to_disk()
    {
        tracing::warn!(error = %error, "Failed to flush sandbox events to disk");
    }
}

/// Violation metrics, or `None` if sandbox is not active.
pub fn metrics() -> Option<&'static SandboxMetrics> {
    SANDBOX.get().map(|state| state.logger.metrics())
}

/// Manages the OS-level sandbox. Call `apply_required()` when continuing
/// without enforcement would be unsafe, then call `install()`.
pub struct SandboxManager {
    profile: ProfileName,
    logger: SandboxLogger,
    net_restricted: bool,
    applied: bool,
}

impl SandboxManager {
    /// Create a sandbox manager. Does not apply until `apply()` is called.
    pub fn new(profile: ProfileName, _workspace: &Path) -> Self {
        let net_restricted = profile.restricts_network();
        Self {
            profile,
            logger: SandboxLogger::new(),
            net_restricted,
            applied: false,
        }
    }

    /// Attempt to apply the sandbox to the current process. **Irreversible.**
    ///
    /// This compatibility API degrades gracefully: `Ok(())` does not mean
    /// enforcement is active. Call [`Self::is_applied`] to inspect the outcome,
    /// or use [`Self::apply_required`] when enforcement is mandatory.
    pub fn apply(&mut self, workspace: &Path) -> anyhow::Result<()> {
        self.apply_with_requirement(workspace, ApplyRequirement::Graceful)
    }

    /// Apply the sandbox and fail unless the selected non-`off` profile is
    /// kernel-enforced. **Irreversible.**
    ///
    /// This is the fail-closed entry point for child runners and other callers
    /// that must not continue when enforcement is unsupported or application
    /// fails. Selecting `off` remains an explicit successful opt-out.
    pub fn apply_required(&mut self, workspace: &Path) -> anyhow::Result<()> {
        self.apply_with_requirement(workspace, ApplyRequirement::Required)
    }

    #[cfg(all(feature = "enforce", unix))]
    fn apply_with_requirement(
        &mut self,
        workspace: &Path,
        requirement: ApplyRequirement,
    ) -> anyhow::Result<()> {
        if self.profile == ProfileName::Off {
            tracing::info!("Sandbox disabled (profile: off)");
            return validate_apply_outcome(&self.profile, requirement, ApplyOutcome::Disabled);
        }
        let config = profiles::load_sandbox_config(workspace)?;
        let mut resolved = self.profile.resolve_profile(workspace, &config)?;
        self.net_restricted = resolved.restrict_network;
        let support = Sandbox::support_info();
        if !support.is_supported {
            tracing::warn!(
                details = %support.details,
                "Sandbox not supported on this platform, continuing without sandbox"
            );
            self.logger.log(SandboxEvent::apply_failed(
                &self.profile.to_string(),
                workspace,
                &support.details,
            ));
            return validate_apply_outcome(
                &self.profile,
                requirement,
                ApplyOutcome::Unavailable(support.details),
            );
        }
        let caps = ProfileName::capability_set_from_profile(workspace, &resolved)?;
        resolved.deny = deny::effective_deny_paths(workspace, &resolved.deny);
        match Sandbox::apply(&caps) {
            Ok(_) => {
                self.applied = true;
                self.logger.log(SandboxEvent::profile_applied(
                    &self.profile.to_string(),
                    workspace,
                    &resolved,
                ));
                tracing::info!(
                    profile = %self.profile,
                    workspace = %workspace.display(),
                    restrict_network_configured = self.net_restricted,
                    "Sandbox applied (kernel-enforced, irreversible)"
                );
                validate_apply_outcome(&self.profile, requirement, ApplyOutcome::Applied)
            }
            Err(error) => {
                tracing::warn!(
                    profile = %self.profile,
                    error = %error,
                    "Sandbox could not be applied, continuing without sandbox"
                );
                self.logger.log(SandboxEvent::apply_failed(
                    &self.profile.to_string(),
                    workspace,
                    &error,
                ));
                validate_apply_outcome(
                    &self.profile,
                    requirement,
                    ApplyOutcome::Unavailable(error.to_string()),
                )
            }
        }
    }

    #[cfg(not(all(feature = "enforce", unix)))]
    fn apply_with_requirement(
        &mut self,
        _workspace: &Path,
        requirement: ApplyRequirement,
    ) -> anyhow::Result<()> {
        if self.profile == ProfileName::Off {
            tracing::info!("Sandbox disabled (profile: off)");
            return validate_apply_outcome(&self.profile, requirement, ApplyOutcome::Disabled);
        }
        tracing::info!(
            profile = %self.profile,
            "Sandbox enforcement unavailable (built without 'enforce' feature)"
        );
        validate_apply_outcome(
            &self.profile,
            requirement,
            ApplyOutcome::Unavailable("built without the 'enforce' feature".to_string()),
        )
    }

    /// Store globally for session-lifetime violation logging.
    pub fn install(self) {
        let _ = self.logger.flush_to_disk();
        let _ = SANDBOX.set(GlobalSandboxState {
            profile: self.profile.to_string(),
            logger: self.logger,
            applied: self.applied,
            restrict_network_at_known_linux_launches: restrict_network_at_known_linux_launches(
                self.applied,
                self.net_restricted,
            ),
        });
    }

    /// Check whether the current platform supports sandboxing.
    #[cfg(all(feature = "enforce", unix))]
    pub fn support_info() -> nono::SupportInfo {
        Sandbox::support_info()
    }

    /// Whether the sandbox was successfully applied.
    pub fn is_applied(&self) -> bool {
        self.applied
    }

    /// Whether known Linux child launch paths should install the seccomp filter.
    pub fn restrict_child_network(&self) -> bool {
        restrict_network_at_known_linux_launches(self.applied, self.net_restricted)
    }

    /// The active profile name.
    pub fn profile(&self) -> &ProfileName {
        &self.profile
    }

    /// Access the sandbox event logger (before `install()`).
    pub fn logger(&self) -> &SandboxLogger {
        &self.logger
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn configured_profile_is_recorded() {
        set_configured_profile("read-only");
        assert_eq!(configured_profile_name(), Some("read-only"));
    }

    #[test]
    fn known_launch_guard_is_linux_only() {
        assert_eq!(
            restrict_network_at_known_linux_launches(
                /*applied*/ true, /*configured*/ true
            ),
            cfg!(target_os = "linux")
        );
        assert!(!restrict_network_at_known_linux_launches(
            /*applied*/ false, /*configured*/ true
        ));
        assert!(!restrict_network_at_known_linux_launches(
            /*applied*/ true, /*configured*/ false
        ));
    }

    #[test]
    fn required_apply_rejects_unavailable_enforcement() {
        let error = validate_apply_outcome(
            &ProfileName::Strict,
            ApplyRequirement::Required,
            ApplyOutcome::Unavailable("unsupported in test".to_string()),
        )
        .expect_err("required enforcement must fail closed");

        assert_eq!(
            error.to_string(),
            "sandbox profile 'strict' requires enforcement, but it was not applied: unsupported in test"
        );
    }

    #[test]
    fn graceful_apply_accepts_unavailable_enforcement() {
        assert!(
            validate_apply_outcome(
                &ProfileName::Strict,
                ApplyRequirement::Graceful,
                ApplyOutcome::Unavailable("unsupported in test".to_string()),
            )
            .is_ok()
        );
    }

    #[test]
    fn required_apply_accepts_applied_or_explicitly_disabled_outcomes() {
        for (profile, outcome) in [
            (ProfileName::Strict, ApplyOutcome::Applied),
            (ProfileName::Off, ApplyOutcome::Disabled),
        ] {
            assert!(
                validate_apply_outcome(&profile, ApplyRequirement::Required, outcome).is_ok(),
                "unexpected rejection for {profile}"
            );
        }
    }
}
