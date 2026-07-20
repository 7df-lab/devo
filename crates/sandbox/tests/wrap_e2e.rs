//! E2E tests for the command-wrapping sandbox API (`wrap_command_for_profile`).
//!
//! Building a wrap is pure argv construction (no kernel state changes in the
//! parent), so the parent test wraps `sh -c 'cat target'` directly and asserts
//! on the child's output: a denied file's MARKER must never appear, a control
//! file must stay readable, and a denied file must stay unwritten. Network
//! restriction is probed by re-invoking this test binary inside the wrap and
//! attempting a TCP connect.
//!
//! When no launcher is available (Linux without bwrap; macOS without
//! `sandbox-exec`) the wrap API returns `SandboxWrap::None` and every test
//! here skips instead of failing.

#![cfg(all(unix, feature = "enforce"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MARKER: &str = "wrap-e2e-marker-7c2b9d";
const SCENARIO_ENV: &str = "WRAP_E2E_SCENARIO";

/// `--bind / / -- true` proves both that bwrap exists and that this kernel
/// allows the namespace creation bwrap needs (some CI containers deny it).
fn bwrap_usable() -> bool {
    Command::new("bwrap")
        .args(["--bind", "/", "/", "--", "true"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn temp_workspace(tag: &str, profile_toml: &str) -> PathBuf {
    let workspace = std::env::temp_dir().join(format!(
        "devo-wrap-e2e-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(workspace.join(".devo")).expect("create .devo config directory");
    fs::write(workspace.join(".devo").join("sandbox.toml"), profile_toml)
        .expect("write sandbox.toml");
    dunce::canonicalize(&workspace).expect("canonicalize temp workspace")
}

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Run `shell_command` (via `sh -c`) inside the wrapped sandbox and return its
/// output, then clean up the launch's placeholder directory (the child has
/// exited, so the mounts are gone).
fn run_wrapped(
    wrapped: &devo_sandbox::WrappedCommand,
    shell_command: &str,
) -> std::process::Output {
    let output = Command::new(&wrapped.program)
        .args(&wrapped.prefix_args)
        .arg("sh")
        .arg("-c")
        .arg(shell_command)
        .output()
        .expect("spawn wrapped sh");
    if let Some(directory) = &wrapped.placeholder_dir {
        devo_sandbox::remove_placeholder_dir(directory);
    }
    output
}

/// Decide the wrap for `profile`, skipping the test (returning `None`) when no
/// launcher is available on this machine.
fn wrap_or_skip(
    profile: &str,
    workspace: &Path,
    mode: devo_sandbox::WrapMode,
) -> Option<devo_sandbox::WrappedCommand> {
    match devo_sandbox::wrap_command_for_profile(
        Some(profile),
        workspace,
        mode,
        &devo_sandbox::SandboxLogger::new(),
    )
    .expect("wrap decision must not fail for a defined profile")
    {
        devo_sandbox::SandboxWrap::Wrapped(wrapped) => Some(wrapped),
        devo_sandbox::SandboxWrap::None => {
            eprintln!("skipping: no sandbox launcher available on this machine");
            None
        }
    }
}

fn assert_deny_enforced(workspace: &Path, wrapped: &devo_sandbox::WrappedCommand, tag: &str) {
    let secret = workspace.join("secret.txt");
    let read = run_wrapped(wrapped, &format!("cat '{}'", secret.display()));
    assert!(
        !String::from_utf8_lossy(&read.stdout).contains(MARKER),
        "[{tag}] wrapped child read a denied path\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&read.stdout),
        String::from_utf8_lossy(&read.stderr)
    );

    let control = workspace.join("control.txt");
    let read = run_wrapped(wrapped, &format!("cat '{}'", control.display()));
    assert!(
        String::from_utf8_lossy(&read.stdout).contains("hello"),
        "[{tag}] wrapped child must still read the control file\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&read.stdout),
        String::from_utf8_lossy(&read.stderr)
    );

    let write = run_wrapped(wrapped, &format!("echo hijacked >> '{}'", secret.display()));
    assert!(
        !write.status.success(),
        "[{tag}] write to a denied path must fail inside the wrap\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&write.stdout),
        String::from_utf8_lossy(&write.stderr)
    );
    assert_eq!(
        fs::read_to_string(&secret).expect("read denied file from parent"),
        format!("SECRET={MARKER}"),
        "[{tag}] denied file content must be unchanged"
    );
}

fn deny_workspace(tag: &str) -> (PathBuf, TempDirGuard) {
    let workspace = temp_workspace(
        tag,
        "[profiles.wrape2edeny]\nextends = \"workspace\"\ndeny = [\"secret.txt\"]\n",
    );
    fs::write(workspace.join("secret.txt"), format!("SECRET={MARKER}")).expect("write denied file");
    fs::write(workspace.join("control.txt"), "hello workspace").expect("write control");
    let guard = TempDirGuard(workspace.clone());
    (workspace, guard)
}

#[test]
fn pty_wrap_enforces_deny_paths() {
    if cfg!(target_os = "linux") && !bwrap_usable() {
        eprintln!("skipping: bwrap unavailable");
        return;
    }
    let (workspace, _guard) = deny_workspace("pty-deny");
    let Some(wrapped) = wrap_or_skip("wrape2edeny", &workspace, devo_sandbox::WrapMode::PtyOnly)
    else {
        return;
    };
    assert_deny_enforced(&workspace, &wrapped, "pty-deny");
}

#[test]
fn pipe_composed_wrap_enforces_deny_paths() {
    if cfg!(target_os = "linux") && !bwrap_usable() {
        eprintln!("skipping: bwrap unavailable");
        return;
    }
    let (workspace, _guard) = deny_workspace("pipe-deny");
    let Some(wrapped) = wrap_or_skip(
        "wrape2edeny",
        &workspace,
        devo_sandbox::WrapMode::PipeComposed,
    ) else {
        return;
    };
    assert_deny_enforced(&workspace, &wrapped, "pipe-deny");
}

#[test]
fn pty_wrap_blocks_network_for_restrict_network_profile() {
    if !cfg!(target_os = "linux") {
        eprintln!("skipping: network wrap assertions are Linux-only for now");
        return;
    }
    if !bwrap_usable() {
        eprintln!("skipping: bwrap unavailable");
        return;
    }
    let workspace = temp_workspace("net", "[profiles.wrape2enet]\nextends = \"read-only\"\n");
    let _guard = TempDirGuard(workspace.clone());
    let Some(wrapped) = wrap_or_skip("wrape2enet", &workspace, devo_sandbox::WrapMode::PtyOnly)
    else {
        return;
    };

    let exe = std::env::current_exe().expect("current_exe");
    let output = Command::new(&wrapped.program)
        .args(&wrapped.prefix_args)
        .arg(exe)
        .env(SCENARIO_ENV, "net_probe")
        .arg("--ignored")
        .arg("--exact")
        .arg("--nocapture")
        .arg("subprocess_entry")
        .output()
        .expect("spawn wrapped network probe");
    if let Some(directory) = &wrapped.placeholder_dir {
        devo_sandbox::remove_placeholder_dir(directory);
    }
    assert!(
        output.status.success(),
        "network must be unreachable inside the wrap\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `#[ignore]`d — only runs when a parent test re-invokes this binary inside
/// the wrap with `WRAP_E2E_SCENARIO` set.
#[test]
#[ignore]
fn subprocess_entry() {
    let Ok(scenario) = std::env::var(SCENARIO_ENV) else {
        return;
    };
    match scenario.as_str() {
        "net_probe" => match std::net::TcpStream::connect(("127.0.0.1", 9_u16)) {
            Err(error) if is_network_unreachable(&error) => {
                eprintln!("OK: network unreachable inside wrap");
                std::process::exit(0);
            }
            Err(error) => {
                eprintln!("FAIL: unexpected connect error (network reachable?): {error}");
                std::process::exit(1);
            }
            Ok(_) => {
                eprintln!("FAIL: TCP connect succeeded despite restrict_network");
                std::process::exit(1);
            }
        },
        other => {
            eprintln!("unknown scenario: {other}");
            std::process::exit(99);
        }
    }
}

/// Inside an unshared network namespace the loopback interface is down, so a
/// connect fails with ENETUNREACH/ENETDOWN/EHOSTUNREACH — never ECONNREFUSED
/// (which would prove a working loopback, i.e. no restriction).
fn is_network_unreachable(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::ENETUNREACH) | Some(libc::ENETDOWN) | Some(libc::EHOSTUNREACH)
    )
}
