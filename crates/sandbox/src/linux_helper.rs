//! Serializable Linux sandbox helper profile (`--permission-profile` shape,
//! mapped onto Devo profile names).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// JSON payload passed to `devo-linux-sandbox --permission-profile`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinuxSandboxPermissionProfile {
    /// Devo sandbox profile name (`workspace`, `strict`, …).
    pub profile: String,
    /// Workspace root used for profile resolution.
    pub workspace: PathBuf,
    /// When true, helper requests proxy-only networking (bwrap netns + proxy).
    #[serde(default)]
    pub allow_network_for_proxy: bool,
}

impl LinuxSandboxPermissionProfile {
    pub fn new(profile: impl Into<String>, workspace: impl Into<PathBuf>) -> Self {
        Self {
            profile: profile.into(),
            workspace: workspace.into(),
            allow_network_for_proxy: false,
        }
    }

    pub fn with_proxy_network(mut self, allow: bool) -> Self {
        self.allow_network_for_proxy = allow;
        self
    }
}

/// Basename / argv0 alias for the Linux sandbox helper binary.
pub const DEVO_LINUX_SANDBOX_ARG0: &str = "devo-linux-sandbox";

/// Build argv for `devo-linux-sandbox` (excluding the program path itself).
///
/// Builds argv for launching the helper with a permission profile.
/// When `command` is empty, args end at `--` so the spawner can append the
/// user program and its arguments.
pub fn create_linux_sandbox_command_args(
    command: &[String],
    command_cwd: &Path,
    permission_profile: &LinuxSandboxPermissionProfile,
    sandbox_policy_cwd: &Path,
) -> anyhow::Result<Vec<String>> {
    let permission_profile_json = serde_json::to_string(permission_profile)
        .map_err(|error| anyhow::anyhow!("serialize permission profile: {error}"))?;
    let sandbox_policy_cwd = path_utf8(sandbox_policy_cwd)?;
    let command_cwd = path_utf8(command_cwd)?;

    let mut args = vec![
        "--sandbox-policy-cwd".to_string(),
        sandbox_policy_cwd,
        "--command-cwd".to_string(),
        command_cwd,
        "--permission-profile".to_string(),
        permission_profile_json,
    ];
    if permission_profile.allow_network_for_proxy {
        args.push("--allow-network-for-proxy".to_string());
    }
    args.push("--".to_string());
    args.extend(command.iter().cloned());
    Ok(args)
}

fn path_utf8(path: &Path) -> anyhow::Result<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("path must be valid UTF-8: {}", path.display()))
}

/// Locate the `devo-linux-sandbox` helper on PATH or next to the current exe.
pub fn find_linux_sandbox_helper() -> Option<PathBuf> {
    if let Ok(override_path) = std::env::var("DEVO_LINUX_SANDBOX") {
        let path = PathBuf::from(override_path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(path) = which_on_path(DEVO_LINUX_SANDBOX_ARG0) {
        return Some(path);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join(DEVO_LINUX_SANDBOX_ARG0);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::path::Path;

    #[test]
    fn create_linux_sandbox_command_args_serializes_profile_before_separator() {
        let profile = LinuxSandboxPermissionProfile::new("workspace", "/tmp/ws");
        let args = create_linux_sandbox_command_args(
            &["echo".into(), "hi".into()],
            Path::new("/tmp/ws"),
            &profile,
            Path::new("/tmp/ws"),
        )
        .expect("args");
        assert_eq!(args[0], "--sandbox-policy-cwd");
        assert_eq!(args[2], "--command-cwd");
        assert_eq!(args[4], "--permission-profile");
        let parsed: LinuxSandboxPermissionProfile =
            serde_json::from_str(&args[5]).expect("profile json");
        assert_eq!(parsed, profile);
        assert_eq!(args[6], "--");
        assert_eq!(args[7], "echo");
        assert_eq!(args[8], "hi");
    }

    #[test]
    fn create_linux_sandbox_prefix_ends_at_separator_when_command_empty() {
        let profile = LinuxSandboxPermissionProfile::new("strict", "/home/proj");
        let args = create_linux_sandbox_command_args(
            &[],
            Path::new("/home/proj"),
            &profile,
            Path::new("/home/proj"),
        )
        .expect("args");
        assert_eq!(args.last().map(String::as_str), Some("--"));
    }
}
