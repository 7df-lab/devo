use crate::WindowsSandboxCancellationToken;
use crate::protocol::models::PermissionProfile;
use anyhow::Result;
use anyhow::bail;
use devo_util_paths::absolute_path::AbsolutePathBuf;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default)]
pub struct CaptureResult {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn run_windows_sandbox_capture(
    _permission_profile: &PermissionProfile,
    _workspace_roots: &[AbsolutePathBuf],
    _devo_home: &Path,
    _command: Vec<String>,
    _cwd: &Path,
    _env_map: HashMap<String, String>,
    _timeout_ms: Option<u64>,
    _cancellation: Option<WindowsSandboxCancellationToken>,
    _use_private_desktop: bool,
) -> Result<CaptureResult> {
    bail!("Windows sandbox is only available on Windows")
}

pub fn run_windows_sandbox_legacy_preflight(
    _permission_profile: &PermissionProfile,
    _workspace_roots: &[AbsolutePathBuf],
    _devo_home: &Path,
    _cwd: &Path,
    _env_map: &HashMap<String, String>,
) -> Result<()> {
    bail!("Windows sandbox is only available on Windows")
}
