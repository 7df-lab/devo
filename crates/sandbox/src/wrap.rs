//! Command-wrapping sandbox API: decide whether a child process is launched
//! directly or through an OS sandbox launcher (Linux `bwrap`; macOS
//! `sandbox-exec`).
//!
//! Two composition modes:
//!
//! - [`WrapMode::PipeComposed`]: pipe spawns already apply the profile via
//!   `pre_exec` Landlock/Seatbelt, so the wrapper only adds what those kernel
//!   primitives cannot express (Linux read-deny bind-overs and network
//!   restriction).
//! - [`WrapMode::PtyOnly`]: PTY spawns have no `pre_exec` hook, so the
//!   wrapper carries the full profile policy.
//!
//! The API never fails closed (user decision): a missing launcher or a wrap
//! construction failure logs a warning and yields [`SandboxWrap::None`]. Only
//! profile resolution errors are returned as `Err`.
//!
//! Every decision that resolves a non-`off` profile is also recorded as a
//! [`SandboxEvent`] on the caller-provided [`SandboxLogger`] (spawn-time
//! logging — a `pre_exec` child cannot log, so the parent side is the correct
//! layer): a successful wrap logs `profile_applied`, a warn-and-release logs
//! `apply_failed` (construction/validation failed) or `not_enforced` (the
//! environment cannot provide enforcement). [`wrap_command_for_profile`]
//! flushes the events to disk before returning.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use crate::logging::SandboxLogger;
use crate::profiles::{ProfileName, SandboxConfig, SandboxProfile, load_sandbox_config};
use crate::types::SandboxEvent;

/// Name prefix of the per-launch bwrap placeholder directory under
/// `devo_home` (see [`crate::bwrap_placeholder`]).
pub(crate) const PLACEHOLDER_DIR_PREFIX: &str = "bwrap-placeholder.";

/// How long after a successful spawn a bwrap placeholder directory must
/// survive: mounts are not yet up when `spawn` returns, but are long up
/// after this delay.
pub const PLACEHOLDER_CLEANUP_DELAY: Duration = Duration::from_secs(60);

/// Placeholder directories older than this are removed by the janitor.
const PLACEHOLDER_STALE_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Environment override for launcher detection, for tests and diagnostics:
/// `auto` (default), `none` (pretend no launcher exists), `bwrap`, or
/// `sandbox-exec`.
const LAUNCHER_OVERRIDE_ENV: &str = "DEVO_SANDBOX_LAUNCHER";

/// How a wrapped command composes with the other sandbox layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapMode {
    /// PTY spawns have no `pre_exec` hook: the wrapper carries the full policy.
    PtyOnly,
    /// Pipe spawns already apply Landlock/Seatbelt via `pre_exec`; the wrapper
    /// only adds Linux deny bind-overs and network restriction.
    PipeComposed,
}

/// Sandbox decision for a command about to be spawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxWrap {
    /// Run the command as-is (no wrapper needed, or none available — see the
    /// warning logged in that case).
    None,
    /// Replace program/args with a sandbox launcher invocation.
    Wrapped(WrappedCommand),
}

/// A launcher invocation that sandboxes the original command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedCommand {
    /// Launcher executable (`bwrap`, `devo-linux-sandbox`, or
    /// `/usr/bin/sandbox-exec` on macOS).
    pub program: String,
    /// Arguments up to and including the `--` separator; the original program
    /// and its arguments are appended after them.
    pub prefix_args: Vec<String>,
    /// bwrap read-deny placeholder directory. The spawner removes it with
    /// [`remove_placeholder_dir`] [`PLACEHOLDER_CLEANUP_DELAY`] after a
    /// successful spawn (mounts are not up when `spawn` returns).
    pub placeholder_dir: Option<PathBuf>,
    /// When true, the Linux helper applies Landlock/seccomp inside the wrap;
    /// the parent must not also apply a `pre_exec` plan onto the helper.
    pub helper_enforces: bool,
}

/// Decide whether `profile` requires the spawned command to be wrapped in a
/// sandbox launcher, and build that invocation.
///
/// `None`/`"off"`/Windows → [`SandboxWrap::None`]. A missing launcher or a
/// construction failure logs a warning and returns `Ok(SandboxWrap::None)`
/// (never fails closed); only profile resolution errors are `Err`.
///
/// `logger` receives one event per resolved non-`off` decision (see the
/// module docs) and is flushed to disk before this function returns. There is
/// no global logger: callers either hold a [`SandboxLogger`] or pass a fresh
/// `&SandboxLogger::new()` per spawn.
pub fn wrap_command_for_profile(
    profile: Option<&str>,
    workspace: &Path,
    mode: WrapMode,
    logger: &SandboxLogger,
) -> anyhow::Result<SandboxWrap> {
    static JANITOR: std::sync::Once = std::sync::Once::new();
    JANITOR.call_once(cleanup_stale_placeholder_dirs);

    if cfg!(windows) {
        let active = profile.is_some_and(|p| {
            let p = p.trim();
            !p.is_empty() && p != "off" && p != "none"
        });
        if active {
            tracing::info!(
                profile = profile.expect("checked above"),
                mode = ?mode,
                "Windows sandbox profile selected; wrap_command returns None because \
                 enforcement is applied by devo-windows-sandbox in shell_exec"
            );
            if let Err(error) = logger.flush_to_disk() {
                tracing::warn!(error = %error, "failed to flush sandbox events to disk");
            }
        }
        return Ok(SandboxWrap::None);
    }
    let Some(profile) = profile else {
        return Ok(SandboxWrap::None);
    };
    let profile_name = profile
        .parse::<ProfileName>()
        .map_err(|error| anyhow::anyhow!("invalid sandbox profile '{profile}': {error}"))?;
    if profile_name == ProfileName::Off {
        return Ok(SandboxWrap::None);
    }
    let config = load_sandbox_config(workspace)?;
    let resolved = profile_name.resolve_profile(workspace, &config)?;
    let wrap = wrap_for_platform(
        &profile_name,
        &config,
        &resolved,
        workspace,
        mode,
        launcher_availability(),
        logger,
    );
    // Events were recorded at the decision sites; persist them (best-effort).
    if let Err(error) = logger.flush_to_disk() {
        tracing::warn!(error = %error, "failed to flush sandbox events to disk");
    }
    wrap
}

/// Tag a wrap-path sandbox event with the wrap mode and launcher, then log it.
#[allow(unused)]
fn log_wrap_event(logger: &SandboxLogger, mut event: SandboxEvent, mode: WrapMode, launcher: &str) {
    event.mode = Some(format!("{mode:?}"));
    event.launcher = Some(launcher.to_string());
    logger.log(event);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LauncherOverride {
    Auto,
    None,
    Bwrap,
    SandboxExec,
}

fn launcher_override(value: Option<&str>) -> LauncherOverride {
    match value {
        Some("none") => LauncherOverride::None,
        Some("bwrap") => LauncherOverride::Bwrap,
        Some("sandbox-exec") => LauncherOverride::SandboxExec,
        _ => LauncherOverride::Auto,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LauncherAvailability {
    sandbox_exec: bool,
    bwrap: bool,
}

fn launcher_availability() -> LauncherAvailability {
    match launcher_override(std::env::var(LAUNCHER_OVERRIDE_ENV).ok().as_deref()) {
        LauncherOverride::None => LauncherAvailability {
            sandbox_exec: false,
            bwrap: false,
        },
        LauncherOverride::Bwrap => LauncherAvailability {
            sandbox_exec: false,
            bwrap: true,
        },
        LauncherOverride::SandboxExec => LauncherAvailability {
            sandbox_exec: true,
            bwrap: false,
        },
        LauncherOverride::Auto => LauncherAvailability {
            sandbox_exec: sandbox_exec_available(),
            bwrap: bwrap_available(),
        },
    }
}

fn sandbox_exec_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| Path::new("/usr/bin/sandbox-exec").is_file())
}

fn bwrap_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let Ok(output) = std::process::Command::new("bwrap").arg("--help").output() else {
            return false;
        };
        if !output.status.success() {
            // Older bwrap may exit non-zero on --help; fall back to --version.
            return std::process::Command::new("bwrap")
                .arg("--version")
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let help = format!("{stdout}{stderr}");
        // Deny-read masks require `--perms` when the host bwrap supports it. Devo still uses
        // host placeholders today, but warn when the binary is too old so we
        // know advanced deny mounts are unavailable.
        if !(help.contains("--perms")) {
            tracing::warn!(
                "system bwrap does not advertise --perms; in-namespace \
                 deny-read masks are unavailable (placeholder binds still work)"
            );
        }
        true
    })
}

fn wrap_for_platform(
    profile_name: &ProfileName,
    config: &SandboxConfig,
    resolved: &SandboxProfile,
    workspace: &Path,
    mode: WrapMode,
    launchers: LauncherAvailability,
    logger: &SandboxLogger,
) -> anyhow::Result<SandboxWrap> {
    #[cfg(target_os = "linux")]
    {
        linux_wrap(
            profile_name,
            config,
            resolved,
            workspace,
            mode,
            launchers,
            logger,
        )
    }
    #[cfg(target_os = "macos")]
    {
        // `config` and bwrap availability are Linux-only inputs.
        let _ = (config, launchers.bwrap);
        match mode {
            // Pipe children get full Seatbelt enforcement via pre_exec; a
            // wrapper would add nothing on macOS.
            WrapMode::PipeComposed => Ok(SandboxWrap::None),
            // PTY spawns have no pre_exec hook: `sandbox-exec -p <sbpl>`
            // carries the full profile.
            WrapMode::PtyOnly => macos_pty_wrap(
                profile_name,
                resolved,
                workspace,
                launchers.sandbox_exec,
                mode,
                logger,
            ),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (
            profile_name,
            config,
            resolved,
            workspace,
            mode,
            launchers.sandbox_exec,
            launchers.bwrap,
            logger,
        );
        Ok(SandboxWrap::None)
    }
}

/// macOS PTY wrap: `sandbox-exec -p <sbpl>` carrying the full profile. Never
/// fails closed — a missing launcher, a build failure, or a failed precheck
/// warns, records an event, and runs unwrapped (user decision).
#[cfg(all(feature = "enforce", target_os = "macos"))]
fn macos_pty_wrap(
    profile_name: &ProfileName,
    resolved: &SandboxProfile,
    workspace: &Path,
    sandbox_exec_available: bool,
    mode: WrapMode,
    logger: &SandboxLogger,
) -> anyhow::Result<SandboxWrap> {
    if !sandbox_exec_available {
        tracing::warn!(
            profile = %profile_name,
            "sandbox-exec is not available; PTY child runs WITHOUT sandbox enforcement \
             (deny paths, filesystem policy, and network restriction are NOT enforced)"
        );
        log_wrap_event(
            logger,
            SandboxEvent::not_enforced(
                &profile_name.to_string(),
                workspace,
                "sandbox-exec is not available",
            ),
            mode,
            "sandbox-exec",
        );
        return Ok(SandboxWrap::None);
    }
    let sbpl = match crate::seatbelt::seatbelt_profile_for(workspace, resolved) {
        Ok(sbpl) => sbpl,
        Err(error) => {
            tracing::warn!(
                profile = %profile_name,
                error = %error,
                "could not build the Seatbelt profile; PTY child runs WITHOUT \
                 sandbox enforcement"
            );
            log_wrap_event(
                logger,
                SandboxEvent::apply_failed(&profile_name.to_string(), workspace, &error),
                mode,
                "sandbox-exec",
            );
            return Ok(SandboxWrap::None);
        }
    };
    if !crate::seatbelt::sandbox_exec_accepts_profile(&sbpl) {
        tracing::warn!(
            profile = %profile_name,
            "sandbox-exec rejected the generated Seatbelt profile; PTY child runs \
             WITHOUT sandbox enforcement"
        );
        log_wrap_event(
            logger,
            SandboxEvent::apply_failed(
                &profile_name.to_string(),
                workspace,
                &"sandbox-exec rejected the generated Seatbelt profile",
            ),
            mode,
            "sandbox-exec",
        );
        return Ok(SandboxWrap::None);
    }
    tracing::info!(
        profile = %profile_name,
        "spawning PTY command inside sandbox-exec (Seatbelt)"
    );
    log_wrap_event(
        logger,
        SandboxEvent::profile_applied(&profile_name.to_string(), workspace, resolved),
        mode,
        "/usr/bin/sandbox-exec",
    );
    Ok(SandboxWrap::Wrapped(WrappedCommand {
        program: "/usr/bin/sandbox-exec".to_string(),
        prefix_args: vec!["-p".to_string(), sbpl],
        placeholder_dir: None,
        helper_enforces: false,
    }))
}

/// Without the `enforce` feature there is no sbpl emitter; warn, record, and
/// run unwrapped (never fail closed).
#[cfg(all(not(feature = "enforce"), target_os = "macos"))]
fn macos_pty_wrap(
    profile_name: &ProfileName,
    _resolved: &SandboxProfile,
    workspace: &Path,
    _sandbox_exec_available: bool,
    mode: WrapMode,
    logger: &SandboxLogger,
) -> anyhow::Result<SandboxWrap> {
    tracing::warn!(
        profile = %profile_name,
        "built without the 'enforce' feature; PTY child runs WITHOUT sandbox enforcement"
    );
    log_wrap_event(
        logger,
        SandboxEvent::not_enforced(
            &profile_name.to_string(),
            workspace,
            "built without the 'enforce' feature",
        ),
        mode,
        "sandbox-exec",
    );
    Ok(SandboxWrap::None)
}

#[cfg(target_os = "linux")]
fn linux_wrap(
    profile_name: &ProfileName,
    config: &SandboxConfig,
    resolved: &SandboxProfile,
    workspace: &Path,
    mode: WrapMode,
    launchers: LauncherAvailability,
    logger: &SandboxLogger,
) -> anyhow::Result<SandboxWrap> {
    // Prefer the Linux helper path (bwrap → apply-seccomp-then-exec) when
    // available: parent only serializes the profile; the helper enforces.
    // Skip when DEVO_SANDBOX_LAUNCHER forces a direct launcher (helper outer
    // sets `bwrap` to avoid recursive helper wraps).
    if matches!(
        launcher_override(std::env::var(LAUNCHER_OVERRIDE_ENV).ok().as_deref()),
        LauncherOverride::Auto
    ) && linux_wrap_adds_enforcement(resolved, mode)
        && let Some(helper) = crate::linux_helper::find_linux_sandbox_helper()
    {
        let mut permission_profile =
            crate::LinuxSandboxPermissionProfile::new(profile_name.to_string(), workspace);
        if resolved.restrict_network {
            // Prefer the in-process managed-proxy port store; fall back to a
            // parent-process HTTP(S)_PROXY when the user already has one.
            permission_profile =
                permission_profile.with_proxy_network(crate::sandbox_proxy_available());
        }
        match crate::linux_helper::create_linux_sandbox_command_args(
            &[],
            workspace,
            &permission_profile,
            workspace,
        ) {
            Ok(prefix_args) => {
                tracing::info!(
                    profile = %profile_name,
                    mode = ?mode,
                    helper = %helper.display(),
                    "spawning command via devo-linux-sandbox helper"
                );
                log_wrap_event(
                    logger,
                    SandboxEvent::profile_applied(&profile_name.to_string(), workspace, resolved),
                    mode,
                    crate::DEVO_LINUX_SANDBOX_ARG0,
                );
                return Ok(SandboxWrap::Wrapped(WrappedCommand {
                    program: helper.to_string_lossy().into_owned(),
                    prefix_args,
                    placeholder_dir: None,
                    helper_enforces: true,
                }));
            }
            Err(error) => {
                tracing::warn!(
                    profile = %profile_name,
                    error = %error,
                    "could not build linux-sandbox helper args; falling back to direct bwrap"
                );
            }
        }
    }

    if !linux_wrap_adds_enforcement(resolved, mode) {
        return Ok(SandboxWrap::None);
    }
    if !launchers.bwrap {
        // Pipe spawns keep their pre_exec Landlock enforcement (including the
        // nono network block); PTY spawns get nothing at all. Either way the
        // deny bind-overs are lost, so name the paths that go unenforced.
        tracing::warn!(
            profile = %profile_name,
            mode = ?mode,
            deny = ?resolved.deny,
            restrict_network = resolved.restrict_network,
            "bwrap is not available; spawning WITHOUT the sandbox wrapper — \
             the listed deny paths are NOT enforced (PTY spawns also lose \
             network restriction and all filesystem policy)"
        );
        log_wrap_event(
            logger,
            SandboxEvent::not_enforced(
                &profile_name.to_string(),
                workspace,
                "bwrap is not available; deny paths are not enforced",
            ),
            mode,
            "bwrap",
        );
        return Ok(SandboxWrap::None);
    }
    let devbox_based = crate::bwrap::is_devbox_based(profile_name, config);
    match crate::bwrap::bwrap_wrap_argv(workspace, resolved, devbox_based, mode) {
        Ok((prefix_args, placeholder_dir)) => {
            tracing::info!(
                profile = %profile_name,
                mode = ?mode,
                "spawning command inside bwrap sandbox"
            );
            log_wrap_event(
                logger,
                SandboxEvent::profile_applied(&profile_name.to_string(), workspace, resolved),
                mode,
                "bwrap",
            );
            Ok(SandboxWrap::Wrapped(WrappedCommand {
                program: "bwrap".to_string(),
                prefix_args,
                placeholder_dir,
                helper_enforces: false,
            }))
        }
        Err(error) => {
            tracing::warn!(
                profile = %profile_name,
                mode = ?mode,
                error = %error,
                "could not build the bwrap sandbox wrapper; spawning unwrapped \
                 (deny paths are NOT enforced)"
            );
            log_wrap_event(
                logger,
                SandboxEvent::apply_failed(&profile_name.to_string(), workspace, &error),
                mode,
                "bwrap",
            );
            Ok(SandboxWrap::None)
        }
    }
}

/// Whether a Linux bwrap wrap enforces anything beyond the pre_exec sandbox.
#[cfg(any(target_os = "linux", test))]
fn linux_wrap_adds_enforcement(resolved: &SandboxProfile, mode: WrapMode) -> bool {
    match mode {
        // PTY has no pre_exec: the wrapper carries the entire policy.
        WrapMode::PtyOnly => true,
        // Pipe children already get Landlock via pre_exec; bwrap only adds
        // read-deny bind-overs and network restriction.
        WrapMode::PipeComposed => !resolved.deny.is_empty() || resolved.restrict_network,
    }
}

/// Best-effort removal of a per-launch bwrap placeholder directory. Spawners
/// call this [`PLACEHOLDER_CLEANUP_DELAY`] after a successful spawn; it is
/// also safe to call once the wrapped process has exited. Refuses to touch
/// anything not named like a placeholder directory.
pub fn remove_placeholder_dir(directory: &Path) {
    if !is_placeholder_dir_name(directory) {
        tracing::warn!(
            path = %directory.display(),
            "refusing to remove a directory that is not a bwrap placeholder directory"
        );
        return;
    }
    if let Err(error) = std::fs::remove_dir_all(directory)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            path = %directory.display(),
            error = %error,
            "could not remove bwrap placeholder directory"
        );
    }
}

fn is_placeholder_dir_name(directory: &Path) -> bool {
    directory
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(PLACEHOLDER_DIR_PREFIX))
}

/// Startup janitor: remove per-launch placeholder directories older than 24h
/// that crashed processes left behind under `devo_home`. Best-effort.
pub fn cleanup_stale_placeholder_dirs() {
    let Ok(devo_home) = crate::paths::devo_home() else {
        return;
    };
    cleanup_stale_placeholder_dirs_in(&devo_home, SystemTime::now());
}

fn cleanup_stale_placeholder_dirs_in(root: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_placeholder_dir_name(&path) || !path.is_dir() {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|mtime| now.duration_since(mtime).ok())
            .is_some_and(|age| age >= PLACEHOLDER_STALE_AGE);
        if !stale {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                tracing::info!(path = %path.display(), "removed stale bwrap placeholder directory")
            }
            Err(error) => tracing::warn!(
                path = %path.display(),
                error = %error,
                "could not remove stale bwrap placeholder directory"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn temp_workspace(tag: &str, toml_body: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let workspace =
            std::env::temp_dir().join(format!("devo-wrap-{tag}-{}-{nanos}", std::process::id()));
        let devo = workspace.join(".devo");
        std::fs::create_dir_all(&devo).expect("create sandbox config directory");
        std::fs::write(devo.join("sandbox.toml"), toml_body).expect("write sandbox config");
        workspace
    }

    fn resolved_profile(deny: &[&str], restrict_network: bool) -> SandboxProfile {
        SandboxProfile {
            name: "test".to_string(),
            read_only: vec![],
            read_write: vec![],
            deny: deny.iter().map(PathBuf::from).collect(),
            default_read: true,
            restrict_network,
        }
    }

    #[test]
    fn none_and_off_profiles_never_wrap() {
        let workspace = Path::new("/tmp");
        let logger = SandboxLogger::new();
        for profile in [None, Some("off"), Some("none")] {
            for mode in [WrapMode::PtyOnly, WrapMode::PipeComposed] {
                assert_eq!(
                    wrap_command_for_profile(profile, workspace, mode, &logger)
                        .expect("off/None profiles are not errors"),
                    SandboxWrap::None,
                    "profile {profile:?} in mode {mode:?} must not wrap"
                );
            }
        }
        assert!(
            logger.take_events().is_empty(),
            "off/None profiles must not record events"
        );
    }

    #[test]
    fn undefined_custom_profile_is_an_error() {
        let workspace = temp_workspace("missing", "");
        let error = wrap_command_for_profile(
            Some("devo-test-missing-profile-xyz"),
            &workspace,
            WrapMode::PipeComposed,
            &SandboxLogger::new(),
        )
        .expect_err("an unresolvable profile name must fail, not silently unwrap");
        assert!(
            error.to_string().contains("not found"),
            "unexpected error: {error:#}"
        );
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_pipe_never_wraps_and_pty_wraps_via_sandbox_exec() {
        let workspace = temp_workspace(
            "macos",
            "[profiles.wrapdeny]\nextends = \"workspace\"\ndeny = [\"secret.txt\"]\n",
        );
        // Pipe children enforce via pre_exec Seatbelt, so they never wrap.
        assert_eq!(
            wrap_command_for_profile(
                Some("wrapdeny"),
                &workspace,
                WrapMode::PipeComposed,
                &SandboxLogger::new(),
            )
            .expect("valid profile resolves"),
            SandboxWrap::None,
            "macOS PipeComposed must not wrap"
        );
        match wrap_command_for_profile(
            Some("wrapdeny"),
            &workspace,
            WrapMode::PtyOnly,
            &SandboxLogger::new(),
        )
        .expect("valid profile resolves")
        {
            SandboxWrap::Wrapped(wrapped) => {
                assert_eq!(wrapped.program, "/usr/bin/sandbox-exec");
                assert_eq!(wrapped.prefix_args.len(), 2, "{wrapped:?}");
                assert_eq!(wrapped.prefix_args[0], "-p");
                let sbpl = &wrapped.prefix_args[1];
                assert!(sbpl.contains("(deny default)"), "{sbpl}");
                assert!(sbpl.contains("(allow pseudo-tty)"), "{sbpl}");
                assert!(sbpl.contains("(deny file-read*"), "{sbpl}");
                assert_eq!(wrapped.placeholder_dir, None);
            }
            SandboxWrap::None => assert!(
                !Path::new("/usr/bin/sandbox-exec").is_file(),
                "sandbox-exec exists but the PTY wrap was declined"
            ),
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "macos"))]
    fn macos_pty_wrap_without_launcher_records_not_enforced() {
        let logger = SandboxLogger::new();
        let wrap = macos_pty_wrap(
            &ProfileName::Workspace,
            &resolved_profile(&["secret.txt"], false),
            Path::new("/tmp"),
            /*sandbox_exec_available*/ false,
            WrapMode::PtyOnly,
            &logger,
        )
        .expect("a missing launcher is a warn-and-release, not an error");

        assert_eq!(wrap, SandboxWrap::None);
        let events = logger.take_events();
        assert_eq!(events.len(), 1, "expected exactly one event: {events:?}");
        let event = &events[0];
        assert!(matches!(
            event.event_type,
            crate::types::SandboxEventType::NotEnforced
        ));
        assert_eq!(event.profile, "workspace");
        assert_eq!(event.mode.as_deref(), Some("PtyOnly"));
        assert_eq!(event.launcher.as_deref(), Some("sandbox-exec"));
        assert_eq!(event.enforced, Some(false));
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "macos"))]
    fn macos_pty_wrap_success_records_profile_applied() {
        if !Path::new("/usr/bin/sandbox-exec").is_file() {
            eprintln!("skipping: sandbox-exec not available on this machine");
            return;
        }
        let workspace = temp_workspace(
            "macoslog",
            "[profiles.wraplog]\nextends = \"workspace\"\ndeny = [\"secret.txt\"]\n",
        );
        let profile: ProfileName = "wraplog".parse().expect("valid custom profile name");
        let config = load_sandbox_config(&workspace).expect("load sandbox config");
        let resolved = profile
            .resolve_profile(&workspace, &config)
            .expect("custom profile resolves");
        let logger = SandboxLogger::new();

        let wrap = macos_pty_wrap(
            &profile,
            &resolved,
            &workspace,
            /*sandbox_exec_available*/ true,
            WrapMode::PtyOnly,
            &logger,
        )
        .expect("wrap construction must not fail");

        assert!(matches!(wrap, SandboxWrap::Wrapped(_)), "{wrap:?}");
        let events = logger.take_events();
        assert_eq!(events.len(), 1, "expected exactly one event: {events:?}");
        let event = &events[0];
        assert!(matches!(
            event.event_type,
            crate::types::SandboxEventType::ProfileApplied
        ));
        assert_eq!(event.profile, "wraplog");
        assert_eq!(event.mode.as_deref(), Some("PtyOnly"));
        assert_eq!(event.launcher.as_deref(), Some("/usr/bin/sandbox-exec"));
        assert_eq!(event.enforced, Some(true));
        assert_eq!(
            event.deny_paths.as_deref(),
            Some(&["secret.txt".to_string()][..])
        );
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn linux_wrap_without_bwrap_records_not_enforced() {
        let logger = SandboxLogger::new();
        let wrap = linux_wrap(
            &ProfileName::Workspace,
            &SandboxConfig::default(),
            &resolved_profile(&["secret.txt"], false),
            Path::new("/tmp"),
            WrapMode::PipeComposed,
            LauncherAvailability {
                sandbox_exec: false,
                bwrap: false,
            },
            &logger,
        )
        .expect("a missing bwrap is a warn-and-release, not an error");

        assert_eq!(wrap, SandboxWrap::None);
        let events = logger.take_events();
        assert_eq!(events.len(), 1, "expected exactly one event: {events:?}");
        let event = &events[0];
        assert!(matches!(
            event.event_type,
            crate::types::SandboxEventType::NotEnforced
        ));
        assert_eq!(event.profile, "workspace");
        assert_eq!(event.mode.as_deref(), Some("PipeComposed"));
        assert_eq!(event.launcher.as_deref(), Some("bwrap"));
        assert_eq!(event.enforced, Some(false));
    }

    #[test]
    fn launcher_override_values() {
        assert_eq!(launcher_override(None), LauncherOverride::Auto);
        assert_eq!(launcher_override(Some("auto")), LauncherOverride::Auto);
        assert_eq!(launcher_override(Some("none")), LauncherOverride::None);
        assert_eq!(launcher_override(Some("bwrap")), LauncherOverride::Bwrap);
        assert_eq!(
            launcher_override(Some("sandbox-exec")),
            LauncherOverride::SandboxExec
        );
        assert_eq!(launcher_override(Some("garbage")), LauncherOverride::Auto);
    }

    #[test]
    fn linux_wrap_adds_enforcement_only_for_deny_or_network_in_pipe_mode() {
        let deny_profile = resolved_profile(&["secret.txt"], false);
        let net_profile = resolved_profile(&[], true);
        let plain_profile = resolved_profile(&[], false);

        assert!(linux_wrap_adds_enforcement(
            &deny_profile,
            WrapMode::PipeComposed
        ));
        assert!(linux_wrap_adds_enforcement(
            &net_profile,
            WrapMode::PipeComposed
        ));
        assert!(!linux_wrap_adds_enforcement(
            &plain_profile,
            WrapMode::PipeComposed
        ));
        for profile in [&deny_profile, &net_profile, &plain_profile] {
            assert!(
                linux_wrap_adds_enforcement(profile, WrapMode::PtyOnly),
                "PTY wraps always carry the full policy"
            );
        }
    }

    #[test]
    fn placeholder_dir_name_guard_rejects_other_paths() {
        assert!(is_placeholder_dir_name(Path::new(
            "/home/u/.devo/bwrap-placeholder.abc123"
        )));
        assert!(!is_placeholder_dir_name(Path::new("/home/u/.devo")));
        assert!(!is_placeholder_dir_name(Path::new("/")));
        assert!(!is_placeholder_dir_name(Path::new(
            "/home/u/.devo/bwrap-placeholder"
        )));
    }

    #[test]
    fn remove_placeholder_dir_refuses_foreign_directories() {
        let root = std::env::temp_dir().join(format!(
            "devo-wrap-guard-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("system clock after Unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("keep")).expect("create foreign directory");
        remove_placeholder_dir(&root.join("keep"));
        assert!(root.join("keep").is_dir(), "foreign directory must survive");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn janitor_removes_only_stale_placeholder_dirs() {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("devo-janitor-{}-{nanos}", std::process::id()));
        let placeholder = root.join("bwrap-placeholder.test01");
        std::fs::create_dir_all(&placeholder).expect("create placeholder directory");
        std::fs::write(placeholder.join("sandbox-blocked-0"), "x").expect("write placeholder file");
        std::fs::create_dir_all(root.join("keep")).expect("create foreign directory");

        // Young placeholders survive a normal sweep.
        cleanup_stale_placeholder_dirs_in(&root, SystemTime::now());
        assert!(placeholder.is_dir(), "young placeholder must survive");

        // A clock far in the future makes everything look stale: the
        // placeholder goes, the foreign directory stays.
        let far_future = SystemTime::now() + Duration::from_secs(72 * 60 * 60);
        cleanup_stale_placeholder_dirs_in(&root, far_future);
        assert!(!placeholder.exists(), "stale placeholder must be removed");
        assert!(root.join("keep").is_dir(), "foreign directory must survive");
        let _ = std::fs::remove_dir_all(&root);
    }
}
