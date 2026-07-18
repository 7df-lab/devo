//! Filesystem path tables for sandbox profiles.
//!
//! Collects device files, temp directories, sensitive deny-paths, and
//! ecosystem (package-manager / toolchain) writable paths into helpers
//! consumed by [`super::profiles`].

use anyhow::Context;
use std::path::{Path, PathBuf};

// ── Devo state directory ────────────────────────────────────────────────────

/// Devo state directory — always writable (`$DEVO_HOME` or `~/.devo`).
pub(crate) fn devo_home() -> anyhow::Result<PathBuf> {
    devo_home_with(devo_util_paths::find_devo_home)
}

fn devo_home_with(resolver: impl FnOnce() -> std::io::Result<PathBuf>) -> anyhow::Result<PathBuf> {
    resolver().context("failed to resolve Devo home directory")
}

// ── Device files & directories ──────────────────────────────────────────────

/// Device files that need write access for normal tool operation.
///
/// Without write access to these, common programs (git, curl, ssh, compilers)
/// break because they can't open `/dev/null` as an output sink, allocate PTYs,
/// or seed RNGs.
///
/// These are individual files (use `allow_file`, not `allow_path`).
/// `/dev/pts` is a directory (PTY slaves on Linux) so it uses `allow_path`.
#[cfg(all(feature = "enforce", unix))]
pub(crate) const DEVICE_FILES: &[&str] = &[
    "/dev/null",    // output sink — used by virtually every CLI tool
    "/dev/zero",    // zero source — used by memory allocators
    "/dev/random",  // entropy — used by crypto/TLS
    "/dev/urandom", // entropy — used by crypto/TLS
    "/dev/tty",     // controlling terminal — used by git, ssh, gpg
    "/dev/ptmx",    // PTY allocation — used by terminal spawning
    "/dev/fd",      // file descriptor access (symlink to /proc/self/fd on Linux)
];

/// Device directories that need write access.
#[cfg(all(feature = "enforce", unix))]
pub(crate) const DEVICE_DIRS: &[&str] = &[
    "/dev/pts", // PTY slaves (Linux)
];

// ── Temporary directories ───────────────────────────────────────────────────

/// Temporary directories that need write access.
///
/// On Linux, `/tmp` is the standard temp directory.
/// On macOS, programs use both `/tmp` (symlink to `/private/tmp`) and
/// `/private/var/folders/` (the real `TMPDIR` / `NSTemporaryDirectory()`).
/// git, compilers, and other tools write temp files to `$TMPDIR` which
/// resolves to `/private/var/folders/xx/.../T/` on macOS.
pub(crate) fn temp_writable_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("/tmp"), PathBuf::from("/var/tmp")];

    // macOS: /tmp → /private/tmp, but the real TMPDIR is under /private/var/folders.
    // Also include /private/tmp since Seatbelt may resolve the symlink.
    if cfg!(target_os = "macos") {
        for p in ["/private/tmp", "/private/var/tmp", "/private/var/folders"] {
            let pb = PathBuf::from(p);
            if pb.exists() && pb.is_dir() {
                paths.push(pb);
            }
        }
    }

    // Respect $TMPDIR if it points somewhere else (e.g. custom Linux setups).
    if let Ok(tmpdir) = std::env::var("TMPDIR") {
        let pb = PathBuf::from(&tmpdir);
        if pb.exists() && pb.is_dir() && !paths.contains(&pb) {
            paths.push(pb);
        }
    }

    paths
}

// ── Essential writable paths ────────────────────────────────────────────────

/// Writable directory paths for profiles that allow workspace writes (workspace, devbox, strict).
/// Device files are handled separately via `allow_file` in `to_capability_set_with_config`.
pub(crate) fn essential_writable_paths(workspace: &Path) -> anyhow::Result<Vec<PathBuf>> {
    essential_writable_paths_with_home(workspace, devo_home())
}

fn essential_writable_paths_with_home(
    workspace: &Path,
    home: anyhow::Result<PathBuf>,
) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = vec![workspace.to_path_buf(), home?];
    paths.extend(temp_writable_paths());
    Ok(paths)
}

/// Writable directory paths for the read-only profile (minimal: just ~/.devo + temp).
/// Device files are handled separately via `allow_file` in `to_capability_set_with_config`.
pub(crate) fn essential_writable_paths_minimal() -> anyhow::Result<Vec<PathBuf>> {
    essential_writable_paths_minimal_with_home(devo_home())
}

fn essential_writable_paths_minimal_with_home(
    home: anyhow::Result<PathBuf>,
) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = vec![home?];
    paths.extend(temp_writable_paths());
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn unresolved_home() -> std::io::Result<PathBuf> {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "test home unavailable",
        ))
    }

    #[test]
    fn devo_home_resolution_is_fallible_without_environment_mutation() {
        let error = devo_home_with(unresolved_home).expect_err("home resolution must fail");

        assert!(error.to_string().contains("failed to resolve Devo home"));
    }

    #[test]
    fn essential_paths_propagate_home_resolution_errors() {
        let workspace = Path::new("/workspace");
        let error = essential_writable_paths_with_home(
            workspace,
            Err(anyhow::anyhow!("test home unavailable")),
        )
        .expect_err("essential paths require a resolved home");
        let minimal_error = essential_writable_paths_minimal_with_home(Err(anyhow::anyhow!(
            "test minimal home unavailable"
        )))
        .expect_err("minimal essential paths require a resolved home");

        assert_eq!(error.to_string(), "test home unavailable");
        assert_eq!(minimal_error.to_string(), "test minimal home unavailable");
    }
}
