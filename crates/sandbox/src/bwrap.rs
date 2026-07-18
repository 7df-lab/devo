//! Linux bwrap re-exec helpers and their fail-closed read-deny placeholders.

#[cfg(any(target_os = "linux", all(feature = "enforce", unix)))]
use crate::profiles;
use crate::profiles::ProfileName;
#[cfg(target_os = "linux")]
use crate::profiles::SandboxConfig;
#[cfg(target_os = "linux")]
use crate::{deny, paths};
#[cfg(target_os = "linux")]
use anyhow::Context;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

const BWRAP_ENV_VAR: &str = "__DEVO_INSIDE_BWRAP";
const BWRAP_MARKER_VALUE: &str = "devo-bwrap-v1";

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MountNamespaceIsolation {
    Isolated,
    SharedWithPidOne,
    Unknown,
}

/// Whether this process is the bwrap re-exec created by this crate.
pub fn is_inside_bwrap() -> bool {
    is_inside_bwrap_with_marker(std::env::var_os(BWRAP_ENV_VAR).as_deref())
}

fn is_inside_bwrap_with_marker(marker: Option<&std::ffi::OsStr>) -> bool {
    #[cfg(target_os = "linux")]
    {
        // A bwrap re-exec always creates a new mount namespace. Reject the
        // marker unless procfs affirmatively proves that isolation boundary.
        return has_bwrap_marker_and_mount_namespace(marker, bwrap_mount_namespace_isolation());
    }

    #[cfg(not(target_os = "linux"))]
    {
        has_exact_bwrap_marker(marker)
    }
}

fn has_exact_bwrap_marker(marker: Option<&std::ffi::OsStr>) -> bool {
    marker.is_some_and(|value| value == std::ffi::OsStr::new(BWRAP_MARKER_VALUE))
}

#[cfg(any(target_os = "linux", test))]
fn has_bwrap_marker_and_mount_namespace(
    marker: Option<&std::ffi::OsStr>,
    namespace_isolation: MountNamespaceIsolation,
) -> bool {
    has_exact_bwrap_marker(marker)
        && matches!(namespace_isolation, MountNamespaceIsolation::Isolated)
}

#[cfg(target_os = "linux")]
fn bwrap_mount_namespace_isolation() -> MountNamespaceIsolation {
    let Ok(current) = std::fs::read_link("/proc/self/ns/mnt") else {
        return MountNamespaceIsolation::Unknown;
    };
    let Ok(pid_one) = std::fs::read_link("/proc/1/ns/mnt") else {
        return MountNamespaceIsolation::Unknown;
    };
    if current == pid_one {
        MountNamespaceIsolation::SharedWithPidOne
    } else {
        MountNamespaceIsolation::Isolated
    }
}

/// Kept closed until a separate devbox-specific trust policy is introduced.
pub fn trust_bwrap_marker_for_devbox() -> bool {
    false
}

/// Build a bwrap command that re-execs the current process with `deny_write`
/// paths mounted read-only and `deny_read` paths bound over with an unreadable
/// placeholder (EPERM on read).
///
/// Returns `Ok(None)` only if already inside bwrap. Caller should `cmd.exec()`
/// the returned command; construction failures are returned as errors.
pub fn bwrap_reexec_command(
    deny_write: &[&str],
    deny_read: &[&str],
) -> anyhow::Result<Option<std::process::Command>> {
    bwrap_reexec_command_with_state(deny_write, deny_read, is_inside_bwrap())
}

fn bwrap_reexec_command_with_state(
    deny_write: &[&str],
    deny_read: &[&str],
    inside_bwrap: bool,
) -> anyhow::Result<Option<std::process::Command>> {
    if inside_bwrap {
        return Ok(None);
    }
    let self_exe = std::env::current_exe()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cmd = std::process::Command::new("bwrap");
    cmd.arg("--bind").arg("/").arg("/");
    for path in deny_write {
        if Path::new(path).exists() {
            cmd.arg("--ro-bind").arg(path).arg(path);
        }
    }
    #[cfg(target_os = "linux")]
    if !deny_read.is_empty() {
        for path in deny_read {
            let blocked = bwrap_blocked_source_for_path(Path::new(path)).with_context(|| {
                format!(
                    "could not create bwrap placeholder for read-deny path {path}; \
                     refusing to start with a partial sandbox"
                )
            })?;
            cmd.arg("--ro-bind").arg(&blocked).arg(path);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = deny_read;
    cmd.arg("--dev-bind").arg("/dev").arg("/dev");
    cmd.arg("--proc").arg("/proc");
    cmd.env(BWRAP_ENV_VAR, BWRAP_MARKER_VALUE);
    cmd.arg("--").arg(self_exe).args(args);
    Ok(Some(cmd))
}

/// Choose file vs directory placeholder for a deny path (existing dirs need a dir bind).
#[cfg(all(feature = "enforce", target_os = "linux"))]
fn bwrap_blocked_source_for_path(path: &Path) -> anyhow::Result<PathBuf> {
    if deny::deny_path_is_dir(path) {
        bwrap_blocked_placeholder(BwrapPlaceholderKind::Directory)
    } else {
        bwrap_blocked_placeholder(BwrapPlaceholderKind::File)
    }
}

/// Without kernel enforcement there are no read-deny placeholders to bind over.
#[cfg(all(not(feature = "enforce"), target_os = "linux"))]
fn bwrap_blocked_source_for_path(_path: &Path) -> anyhow::Result<PathBuf> {
    anyhow::bail!("bwrap read-deny placeholders require the 'enforce' feature")
}

/// chmod a placeholder to mode 000 so a bwrap bind-over yields EPERM on read.
#[cfg(all(feature = "enforce", target_os = "linux"))]
fn chmod_000(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)
        .with_context(|| format!("could not inspect bwrap placeholder {}", path.display()))?
        .permissions();
    permissions.set_mode(0o000);
    std::fs::set_permissions(path, permissions)
        .with_context(|| format!("could not secure bwrap placeholder {}", path.display()))?;
    Ok(())
}

#[cfg(all(feature = "enforce", target_os = "linux"))]
#[derive(Clone, Copy)]
enum BwrapPlaceholderKind {
    File,
    Directory,
}

#[cfg(all(feature = "enforce", target_os = "linux"))]
impl BwrapPlaceholderKind {
    fn name(self) -> &'static str {
        match self {
            Self::File => "sandbox-blocked",
            Self::Directory => "sandbox-blocked-dir",
        }
    }

    fn is_expected_type(self, file_type: std::fs::FileType) -> bool {
        match self {
            Self::File => file_type.is_file(),
            Self::Directory => file_type.is_dir(),
        }
    }
}

/// Zero-permission placeholder (file or dir) in a private random directory
/// under `devo_home`, used by bwrap bind-over.
///
/// The old PID-named files were predictable entries in a writable home and
/// could be replaced with symlinks. Each bwrap launch now creates a mode-0700
/// directory with `mkdtemp`, then creates the placeholder exclusively inside it.
#[cfg(all(feature = "enforce", target_os = "linux"))]
fn bwrap_blocked_placeholder(kind: BwrapPlaceholderKind) -> anyhow::Result<PathBuf> {
    let directory = create_private_bwrap_placeholder_directory()?;
    bwrap_blocked_placeholder_in(&directory, kind)
}

#[cfg(all(feature = "enforce", target_os = "linux"))]
fn create_private_bwrap_placeholder_directory() -> anyhow::Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::PermissionsExt;

    let devo_home = paths::devo_home().context("could not resolve bwrap placeholder root")?;
    match std::fs::symlink_metadata(&devo_home) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => anyhow::bail!(
            "bwrap placeholder root {} is not a real directory",
            devo_home.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&devo_home).with_context(|| {
                format!(
                    "could not create bwrap placeholder root {}",
                    devo_home.display()
                )
            })?;
            let metadata = std::fs::symlink_metadata(&devo_home).with_context(|| {
                format!(
                    "could not inspect bwrap placeholder root {}",
                    devo_home.display()
                )
            })?;
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                anyhow::bail!(
                    "bwrap placeholder root {} is not a real directory",
                    devo_home.display()
                );
            }
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "could not inspect bwrap placeholder root {}",
                    devo_home.display()
                )
            });
        }
    }

    let template = devo_home.join("bwrap-placeholder.XXXXXX");
    let mut template_bytes = template.into_os_string().into_vec();
    if template_bytes.contains(&0) {
        anyhow::bail!("bwrap placeholder template contains an interior NUL");
    }
    template_bytes.push(0);

    // SAFETY: `template_bytes` is mutable, NUL-terminated, and ends in six Xs.
    if unsafe { libc::mkdtemp(template_bytes.as_mut_ptr().cast()) }.is_null() {
        return Err(std::io::Error::last_os_error())
            .context("could not create private bwrap placeholder directory");
    }
    template_bytes.pop();
    let directory = PathBuf::from(std::ffi::OsString::from_vec(template_bytes));

    let mut permissions = std::fs::metadata(&directory)
        .with_context(|| {
            format!(
                "could not inspect private bwrap placeholder directory {}",
                directory.display()
            )
        })?
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&directory, permissions).with_context(|| {
        format!(
            "could not secure private bwrap placeholder directory {}",
            directory.display()
        )
    })?;
    if !is_private_bwrap_placeholder_directory(&directory) {
        anyhow::bail!(
            "bwrap placeholder directory {} failed validation",
            directory.display()
        );
    }

    Ok(directory)
}

#[cfg(all(feature = "enforce", target_os = "linux"))]
fn is_private_bwrap_placeholder_directory(directory: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Ok(metadata) = std::fs::symlink_metadata(directory) else {
        return false;
    };
    metadata.file_type().is_dir()
        && !metadata.file_type().is_symlink()
        && metadata.permissions().mode() & 0o077 == 0
}

#[cfg(all(feature = "enforce", target_os = "linux"))]
fn bwrap_blocked_placeholder_in(
    directory: &Path,
    kind: BwrapPlaceholderKind,
) -> anyhow::Result<PathBuf> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    if !is_private_bwrap_placeholder_directory(directory) {
        anyhow::bail!(
            "bwrap placeholder directory {} is not private",
            directory.display()
        );
    }

    let path = directory.join(kind.name());
    match kind {
        BwrapPlaceholderKind::File => {
            OpenOptions::new()
                .create_new(true)
                .custom_flags(libc::O_NOFOLLOW)
                .write(true)
                .open(&path)
                .with_context(|| {
                    format!("could not create bwrap placeholder {}", path.display())
                })?;
        }
        BwrapPlaceholderKind::Directory => {
            std::fs::create_dir(&path).with_context(|| {
                format!("could not create bwrap placeholder {}", path.display())
            })?;
        }
    }
    chmod_000(&path)?;
    if !is_valid_bwrap_placeholder(&path, kind) {
        anyhow::bail!("bwrap placeholder {} failed validation", path.display());
    }
    Ok(path)
}

#[cfg(all(feature = "enforce", target_os = "linux"))]
fn is_valid_bwrap_placeholder(path: &Path, kind: BwrapPlaceholderKind) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    !metadata.file_type().is_symlink()
        && kind.is_expected_type(metadata.file_type())
        && metadata.permissions().mode() & 0o777 == 0
}

/// Whether kernel read-deny enforcement is required. Configuration errors are
/// returned rather than being converted into an unsandboxed fallback.
#[cfg(all(feature = "enforce", unix))]
pub fn requires_read_deny(profile: &ProfileName, workspace: &Path) -> anyhow::Result<bool> {
    match profile {
        ProfileName::Custom(name) => {
            let config = profiles::load_sandbox_config(workspace)?;
            Ok(config
                .profiles
                .get(name)
                .is_some_and(|profile| !profile.deny.is_empty()))
        }
        _ => Ok(false),
    }
}

/// Stub when `enforce` is unavailable — nothing is kernel-enforced.
#[cfg(not(all(feature = "enforce", unix)))]
pub fn requires_read_deny(_profile: &ProfileName, _workspace: &Path) -> anyhow::Result<bool> {
    Ok(false)
}

/// A profile's resolved bwrap deny plan.
#[cfg(target_os = "linux")]
struct BwrapDenyPlan {
    deny_write: Vec<String>,
    deny_read: Vec<String>,
    has_globs: bool,
}

/// Resolve the bwrap deny plan in one checked config read.
#[cfg(all(feature = "enforce", target_os = "linux"))]
fn bwrap_deny_plan(profile: &ProfileName, workspace: &Path) -> anyhow::Result<BwrapDenyPlan> {
    let config = profiles::load_sandbox_config(workspace)?;
    let deny_write: Vec<String> = if is_devbox_based(profile, &config) {
        vec!["/data".to_string()]
    } else {
        Vec::new()
    };
    let entries = if *profile == ProfileName::Off {
        Vec::new()
    } else {
        profile.resolve_profile(workspace, &config)?.deny
    };
    let (exact, globs) = deny::partition_deny_entries(&entries);
    let mut deny_read = deny::exact_deny_path_strings(workspace, &exact);
    let has_globs = !globs.is_empty();
    if has_globs {
        tracing::warn!(
            count = globs.len(),
            "sandbox deny globs are enforced best-effort on Linux (expanded at launch); \
             files matching them that are created later are NOT covered"
        );
        let expanded = deny::expand_deny_globs(
            workspace,
            &globs,
            deny::DENY_GLOB_MAX_DEPTH,
            deny::DENY_GLOB_MAX_MATCHES,
            deny::DENY_GLOB_MAX_ENTRIES,
        )
        .ok_or_else(|| anyhow::anyhow!("sandbox deny glob expansion exceeded safety limits"))?;
        deny_read.extend(expanded);
    }
    Ok(BwrapDenyPlan {
        deny_write,
        deny_read,
        has_globs,
    })
}

/// Without kernel enforcement, preserve the devbox `/data` write-deny mount.
#[cfg(all(not(feature = "enforce"), target_os = "linux"))]
fn bwrap_deny_plan(profile: &ProfileName, workspace: &Path) -> anyhow::Result<BwrapDenyPlan> {
    let config = profiles::load_sandbox_config(workspace)?;
    let deny_write = if is_devbox_based(profile, &config) {
        vec!["/data".to_string()]
    } else {
        Vec::new()
    };
    Ok(BwrapDenyPlan {
        deny_write,
        deny_read: Vec::new(),
        has_globs: false,
    })
}

#[cfg(target_os = "linux")]
fn is_devbox_based(profile: &ProfileName, config: &SandboxConfig) -> bool {
    match profile {
        ProfileName::Devbox => true,
        ProfileName::Custom(name) => {
            config.profiles.get(name).and_then(|p| p.extends.as_deref()) == Some("devbox")
        }
        _ => false,
    }
}

/// Build the bwrap re-exec command needed on Linux. It returns `Ok(None)` only
/// if no mount-namespace enforcement is needed or we are already inside bwrap;
/// configuration, glob-expansion, and placeholder failures are errors.
#[cfg(target_os = "linux")]
pub fn bwrap_reexec_for_profile(
    profile: &ProfileName,
    workspace: &Path,
) -> anyhow::Result<Option<std::process::Command>> {
    bwrap_reexec_for_profile_with_state(profile, workspace, is_inside_bwrap())
}

#[cfg(target_os = "linux")]
fn bwrap_reexec_for_profile_with_state(
    profile: &ProfileName,
    workspace: &Path,
    inside_bwrap: bool,
) -> anyhow::Result<Option<std::process::Command>> {
    let BwrapDenyPlan {
        deny_write,
        deny_read,
        has_globs,
    } = bwrap_deny_plan(profile, workspace)?;
    if deny_write.is_empty() && deny_read.is_empty() && !has_globs {
        return Ok(None);
    }
    let write_refs: Vec<&str> = deny_write.iter().map(String::as_str).collect();
    let read_refs: Vec<&str> = deny_read.iter().map(String::as_str).collect();
    bwrap_reexec_command_with_state(&write_refs, &read_refs, inside_bwrap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    #[cfg(all(feature = "enforce", unix))]
    use std::path::PathBuf;

    #[test]
    fn bwrap_reexec_returns_none_inside_bwrap() {
        let result = bwrap_reexec_command_with_state(&["/data"], &[], /*inside_bwrap*/ true)
            .expect("build bwrap command");
        assert!(result.is_none());
    }

    #[test]
    fn bwrap_reexec_returns_some_outside_bwrap() {
        let cmd = bwrap_reexec_command_with_state(&["/tmp"], &[], /*inside_bwrap*/ false)
            .expect("build bwrap command")
            .expect("bwrap command outside bwrap");
        assert_eq!(cmd.get_program(), "bwrap");
    }

    #[test]
    fn bwrap_reexec_writes_the_exact_marker_value() {
        let cmd = bwrap_reexec_command_with_state(&[], &[], /*inside_bwrap*/ false)
            .expect("build bwrap command")
            .expect("bwrap command outside bwrap");
        let marker = cmd.get_envs().find_map(|(name, value)| {
            if name == std::ffi::OsStr::new(BWRAP_ENV_VAR) {
                value
            } else {
                None
            }
        });

        assert_eq!(marker, Some(std::ffi::OsStr::new(BWRAP_MARKER_VALUE)));
    }

    #[test]
    fn bwrap_marker_requires_the_exact_value_without_mutating_environment() {
        assert!(!has_exact_bwrap_marker(/*marker*/ None));
        assert!(!has_exact_bwrap_marker(Some(std::ffi::OsStr::new(""))));
        assert!(!has_exact_bwrap_marker(Some(std::ffi::OsStr::new("0"))));
        assert!(!has_exact_bwrap_marker(Some(std::ffi::OsStr::new("wrong"))));
        assert!(has_exact_bwrap_marker(Some(std::ffi::OsStr::new(
            BWRAP_MARKER_VALUE
        ))));
    }

    #[test]
    fn bwrap_marker_requires_affirmative_mount_namespace_isolation() {
        let marker = Some(std::ffi::OsStr::new(BWRAP_MARKER_VALUE));
        assert!(!has_bwrap_marker_and_mount_namespace(
            marker,
            MountNamespaceIsolation::Unknown
        ));
        assert!(!has_bwrap_marker_and_mount_namespace(
            marker,
            MountNamespaceIsolation::SharedWithPidOne
        ));
        assert!(has_bwrap_marker_and_mount_namespace(
            marker,
            MountNamespaceIsolation::Isolated
        ));
        assert!(!has_bwrap_marker_and_mount_namespace(
            Some(std::ffi::OsStr::new("wrong")),
            MountNamespaceIsolation::Isolated
        ));
    }

    #[test]
    fn bwrap_reexec_skips_nonexistent_paths() {
        let cmd = bwrap_reexec_command_with_state(
            &["/nonexistent-test-path-xyz-12345"],
            &[],
            /*inside_bwrap*/ false,
        )
        .expect("build bwrap command")
        .expect("bwrap command outside bwrap");
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert!(
            !args
                .iter()
                .any(|arg| arg == "/nonexistent-test-path-xyz-12345")
        );
    }

    #[test]
    fn bwrap_reexec_mounts_existing_paths_read_only() {
        let cmd = bwrap_reexec_command_with_state(&["/tmp"], &[], /*inside_bwrap*/ false)
            .expect("build bwrap command")
            .expect("bwrap command outside bwrap");
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert!(
            args.windows(3)
                .any(|window| window == ["--ro-bind", "/tmp", "/tmp"])
        );
    }

    #[test]
    fn bwrap_reexec_uses_dev_bind() {
        let cmd = bwrap_reexec_command_with_state(&[], &[], /*inside_bwrap*/ false)
            .expect("build bwrap command")
            .expect("bwrap command outside bwrap");
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert!(
            args.windows(3)
                .any(|window| window == ["--dev-bind", "/dev", "/dev"])
        );
    }

    #[cfg(all(feature = "enforce", unix))]
    fn temp_workspace_with_sandbox_toml(tag: &str, toml_body: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let workspace =
            std::env::temp_dir().join(format!("devo-{tag}-{}-{nanos}", std::process::id()));
        let devo = workspace.join(".devo");
        std::fs::create_dir_all(&devo).expect("create sandbox config directory");
        std::fs::write(devo.join("sandbox.toml"), toml_body).expect("write sandbox config");
        workspace
    }

    #[cfg(all(feature = "enforce", unix))]
    fn temp_workspace_with_deny(tag: &str, deny_toml: &str) -> PathBuf {
        temp_workspace_with_sandbox_toml(
            tag,
            &format!("[profiles.denytest]\nextends = \"workspace\"\ndeny = [{deny_toml}]\n"),
        )
    }

    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn requires_read_deny_only_for_custom_profile_with_deny() {
        let workspace = temp_workspace_with_deny("requires-deny", "\".env\"");
        assert!(
            requires_read_deny(&ProfileName::Custom("denytest".to_string()), &workspace)
                .expect("load sandbox config")
        );
        assert!(
            !requires_read_deny(&ProfileName::Custom("undefined".to_string()), &workspace)
                .expect("load sandbox config")
        );
        assert!(!requires_read_deny(&ProfileName::Workspace, &workspace).expect("load config"));
        assert!(!requires_read_deny(&ProfileName::Strict, &workspace).expect("load config"));
        assert!(!requires_read_deny(&ProfileName::Devbox, &workspace).expect("load config"));
        assert!(!requires_read_deny(&ProfileName::Off, &workspace).expect("load config"));
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "linux"))]
    fn bwrap_reexec_binds_nonexistent_deny_read_paths() {
        let missing = "/nonexistent-deny-read-path-xyz-12345";
        let cmd = bwrap_reexec_command_with_state(&[], &[missing], /*inside_bwrap*/ false)
            .expect("build bwrap command")
            .expect("bwrap command outside bwrap");
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert!(
            args.windows(3)
                .any(|window| window[0] == "--ro-bind" && window[2] == missing)
        );
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "linux"))]
    fn bwrap_placeholder_is_exclusive_and_validated() {
        let directory = private_placeholder_test_directory("valid");
        let placeholder = bwrap_blocked_placeholder_in(&directory, BwrapPlaceholderKind::File)
            .expect("create placeholder exclusively");
        assert!(is_valid_bwrap_placeholder(
            &placeholder,
            BwrapPlaceholderKind::File
        ));
        assert!(bwrap_blocked_placeholder_in(&directory, BwrapPlaceholderKind::File).is_err());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "linux"))]
    fn bwrap_placeholder_rejects_a_preexisting_symlink() {
        use std::os::unix::fs::symlink;

        let directory = private_placeholder_test_directory("symlink");
        let target = directory.join("outside-target");
        std::fs::write(&target, "must remain unchanged").expect("write symlink target");
        let placeholder = directory.join(BwrapPlaceholderKind::File.name());
        symlink(&target, &placeholder).expect("create preexisting symlink");
        assert!(bwrap_blocked_placeholder_in(&directory, BwrapPlaceholderKind::File).is_err());
        assert_eq!(
            std::fs::read_to_string(&target).expect("read symlink target"),
            "must remain unchanged"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[cfg(all(feature = "enforce", target_os = "linux"))]
    fn private_placeholder_test_directory(tag: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "devo-bwrap-placeholder-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("create placeholder test directory");
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .expect("make placeholder test directory private");
        directory
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "linux"))]
    fn bwrap_reexec_for_profile_devbox_extends_composes_data_and_read_deny() {
        let workspace = temp_workspace_with_sandbox_toml(
            "devbox-compose",
            "[profiles.devcustom]\nextends = \"devbox\"\ndeny = [\"secret.pem\"]\n",
        );
        let cmd = bwrap_reexec_for_profile_with_state(
            &ProfileName::Custom("devcustom".to_string()),
            &workspace,
            /*inside_bwrap*/ false,
        )
        .expect("load sandbox config")
        .expect("build bwrap re-exec command");
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        let deny_path = workspace.join("secret.pem").to_string_lossy().to_string();
        assert!(
            args.windows(3)
                .any(|window| window[0] == "--ro-bind" && window[2] == deny_path)
        );
        if Path::new("/data").exists() {
            assert!(
                args.windows(3)
                    .any(|window| window == ["--ro-bind", "/data", "/data"])
            );
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }
}
