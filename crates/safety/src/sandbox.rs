//! OS-level sandbox enforcement.
//!
//! Implements L3-BEH-SAFETY-001. Defines the Sandbox trait for process,
//! filesystem, and network boundary enforcement. Platform-specific
//! implementations live here or in platform modules.

use std::path::Path;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ── Sandbox Trait ────────────────────────────────────────────────────

/// OS-level sandbox that constrains processes and network access.
///
/// Platform implementations: seccomp (Linux), Seatbelt/sandbox-exec (macOS),
/// job objects (Windows). A no-op implementation is used when sandboxing is
/// not available.
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Constrain a command before execution.
    ///
    /// Returns the (possibly modified) command to execute, or an error
    /// if the command is blocked by sandbox policy.
    async fn constrain_command(
        &self,
        command: &str,
        working_dir: &Path,
        policy: &SandboxPolicy,
    ) -> Result<String, SandboxError>;

    /// Check whether network access is allowed for the given host.
    async fn allow_network(
        &self,
        host: &str,
        port: u16,
        filter: &NetworkEgressFilter,
    ) -> Result<bool, SandboxError>;

    /// Return the current sandbox mode.
    fn mode(&self) -> SandboxMode;

    /// Whether this sandbox implementation provides real enforcement
    /// (vs being a no-op stub).
    fn is_effective(&self) -> bool;
}

// ── Sandbox Policy ───────────────────────────────────────────────────

/// Active sandbox policy applied to a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// Overall sandbox mode.
    pub mode: SandboxMode,
    /// Whether workspace writes are allowed.
    pub workspace_write: bool,
    /// Allowed read paths outside workspace.
    pub extra_read_paths: Vec<String>,
    /// Command allowlist (empty = all commands blocked except workspace).
    pub command_allowlist: Vec<String>,
    /// Command denylist (takes precedence over allowlist).
    pub command_denylist: Vec<String>,
    /// Network egress filter.
    pub network_filter: NetworkEgressFilter,
    /// Maximum process runtime in seconds.
    pub max_runtime_seconds: Option<u64>,
    /// Maximum output size in bytes.
    pub max_output_bytes: Option<u64>,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            mode: SandboxMode::Restricted,
            workspace_write: true,
            extra_read_paths: vec![],
            command_allowlist: vec![],
            command_denylist: vec!["rm".into(), "dd".into(), "mkfs".into(), "shutdown".into()],
            network_filter: NetworkEgressFilter::default(),
            max_runtime_seconds: Some(300),
            max_output_bytes: Some(1_048_576), // 1MB
        }
    }
}

/// Sandbox enforcement mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    /// No sandbox restrictions (dangerous, requires escalation).
    Unrestricted,
    /// Standard sandbox with workspace access.
    Restricted,
    /// External resource access mode.
    External,
}

// ── Network Egress Filter ────────────────────────────────────────────

/// Controls which network targets a process may access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkEgressFilter {
    /// Whether any network access is allowed.
    pub enabled: bool,
    /// Allowed host patterns (glob or exact).
    pub allowed_hosts: Vec<String>,
    /// Denied host patterns (takes precedence).
    pub denied_hosts: Vec<String>,
    /// Allowed port ranges.
    pub allowed_ports: Vec<PortRange>,
    /// Whether localhost is always allowed.
    pub allow_localhost: bool,
}

impl Default for NetworkEgressFilter {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_hosts: vec![],
            denied_hosts: vec![],
            allowed_ports: vec![],
            allow_localhost: true,
        }
    }
}

/// A port range for network egress filtering.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PortRange {
    pub start: u16,
    pub end: u16,
}

// ── Sandbox Error ────────────────────────────────────────────────────

/// Error from sandbox enforcement operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SandboxError {
    #[error("command blocked: {0}")]
    CommandBlocked(String),
    #[error("network blocked: {host}:{port}")]
    NetworkBlocked { host: String, port: u16 },
    #[error("sandbox platform error: {0}")]
    PlatformError(String),
    #[error("sandbox not available on this platform")]
    Unavailable,
    #[error("sandbox policy violation: {0}")]
    PolicyViolation(String),
}

/// Machine-readable error codes for sandbox operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxErrorCode {
    CommandBlocked,
    NetworkBlocked,
    PlatformError,
    Unavailable,
    PolicyViolation,
}

impl SandboxError {
    pub fn code(&self) -> SandboxErrorCode {
        match self {
            Self::CommandBlocked(_) => SandboxErrorCode::CommandBlocked,
            Self::NetworkBlocked { .. } => SandboxErrorCode::NetworkBlocked,
            Self::PlatformError(_) => SandboxErrorCode::PlatformError,
            Self::Unavailable => SandboxErrorCode::Unavailable,
            Self::PolicyViolation(_) => SandboxErrorCode::PolicyViolation,
        }
    }
}

// ── No-Op Sandbox (fallback) ─────────────────────────────────────────

/// A sandbox implementation that performs no enforcement.
/// Used when platform sandboxing is not available or configured.
#[derive(Debug, Clone)]
pub struct NoOpSandbox;

#[async_trait]
impl Sandbox for NoOpSandbox {
    async fn constrain_command(
        &self,
        command: &str,
        _working_dir: &Path,
        _policy: &SandboxPolicy,
    ) -> Result<String, SandboxError> {
        Ok(command.to_string())
    }

    async fn allow_network(
        &self,
        _host: &str,
        _port: u16,
        _filter: &NetworkEgressFilter,
    ) -> Result<bool, SandboxError> {
        Ok(true) // no-op allows all network
    }

    fn mode(&self) -> SandboxMode {
        SandboxMode::Unrestricted
    }

    fn is_effective(&self) -> bool {
        false
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sandbox_policy_denies_dangerous_commands() {
        let policy = SandboxPolicy::default();
        assert!(policy.command_denylist.contains(&"rm".to_string()));
        assert!(policy.command_denylist.contains(&"dd".to_string()));
        assert_eq!(policy.mode, SandboxMode::Restricted);
        assert!(policy.workspace_write);
    }

    #[test]
    fn default_network_filter_disabled() {
        let filter = NetworkEgressFilter::default();
        assert!(!filter.enabled);
        assert!(filter.allow_localhost);
    }

    #[test]
    fn sandbox_error_codes() {
        let err = SandboxError::CommandBlocked("rm -rf".into());
        assert_eq!(err.code(), SandboxErrorCode::CommandBlocked);

        let err = SandboxError::NetworkBlocked {
            host: "evil.com".into(),
            port: 443,
        };
        assert_eq!(err.code(), SandboxErrorCode::NetworkBlocked);
    }

    #[test]
    fn noop_sandbox_allows_everything() {
        let sandbox = NoOpSandbox;
        assert!(!sandbox.is_effective());
        assert_eq!(sandbox.mode(), SandboxMode::Unrestricted);
    }

    #[tokio::test]
    async fn noop_sandbox_passes_commands_through() {
        let sandbox = NoOpSandbox;
        let result = sandbox
            .constrain_command(
                "ls -la",
                std::path::Path::new("/tmp"),
                &SandboxPolicy::default(),
            )
            .await
            .expect("should succeed");
        assert_eq!(result, "ls -la");
    }

    #[test]
    fn sandbox_mode_serde_roundtrip() {
        for mode in &[
            SandboxMode::Unrestricted,
            SandboxMode::Restricted,
            SandboxMode::External,
        ] {
            let json = serde_json::to_string(mode).expect("serialize");
            let restored: SandboxMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *mode);
        }
    }
}
