//! The kernel Landlock profile must not expose the spawning server's procfs.
#![cfg(all(target_os = "linux", feature = "enforce"))]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

const PROBE_ENV: &str = "DEVO_SANDBOX_PROC_PROBE";

#[test]
fn kernel_cannot_read_parent_proc_environ() {
    let support = nono::Sandbox::support_info();
    if !support.is_supported {
        eprintln!("skipping: Landlock unavailable: {}", support.details);
        return;
    }

    // Isolate the test parent's environment from the test harness so the
    // child has a known secret to look for in /proc/<getppid()>/environ.
    let output = Command::new(std::env::current_exe().unwrap())
        .env(PROBE_ENV, "parent")
        .env("DEVO_SANDBOX_PROC_SECRET", "server-credential-marker")
        .args(["--ignored", "--exact", "--nocapture", "proc_parent_entry"])
        .output()
        .expect("spawn parent process");
    assert!(
        output.status.success(),
        "proc fence regression: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "invoked by kernel_cannot_read_parent_proc_environ"]
fn proc_parent_entry() {
    if std::env::var(PROBE_ENV).as_deref() != Ok("parent") {
        return;
    }
    let self_environ = fs::read("/proc/self/environ").expect("parent procfs precondition");
    assert!(
        self_environ
            .windows(b"server-credential-marker".len())
            .any(|w| w == b"server-credential-marker")
    );

    let baseline = Command::new(std::env::current_exe().unwrap())
        .env(PROBE_ENV, "baseline")
        .args(["--ignored", "--exact", "--nocapture", "proc_kernel_entry"])
        .output()
        .expect("spawn unsandboxed baseline");
    assert!(
        baseline.status.success(),
        "unsandboxed same-UID child could not read parent environ (invalid test): {}",
        String::from_utf8_lossy(&baseline.stderr)
    );

    let workspace = std::env::temp_dir();
    let exe = std::env::current_exe().unwrap();
    // The test binary lives in target/debug/deps, outside the real kernel's
    // Python runtime grant. Add its directory for this test invocation only.
    let overlay = devo_sandbox::SandboxPermissionOverlay {
        read_paths: vec![exe.parent().unwrap().to_path_buf()],
        ..Default::default()
    };
    let plan = devo_sandbox::resolve_enforcement_plan_with_overlay(
        Some("rlm-kernel"),
        Path::new(&workspace),
        Some(&overlay),
    )
    .expect("resolve kernel profile")
    .expect("active kernel profile");
    let output = unsafe {
        Command::new(std::env::current_exe().unwrap())
            .env(PROBE_ENV, "kernel")
            .args(["--ignored", "--exact", "--nocapture", "proc_kernel_entry"])
            .pre_exec(move || {
                devo_sandbox::apply_resolved_enforcement_in_child(Some(&plan))
                    .map_err(|e| std::io::Error::other(format!("sandbox apply: {e:#}")))
            })
            .output()
    }
    .expect("apply kernel sandbox and run child");
    assert!(
        output.status.success(),
        "sandboxed kernel could access parent procfs or lost required proc metadata: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "invoked by proc_parent_entry"]
fn proc_kernel_entry() {
    let mode = std::env::var(PROBE_ENV);
    if !matches!(mode.as_deref(), Ok("kernel" | "baseline")) {
        return;
    }
    let parent = unsafe { libc::getppid() };
    let environ = format!("/proc/{parent}/environ");
    if mode.as_deref() == Ok("baseline") {
        let exposed = fs::read(&environ).expect("same-UID baseline must read parent environ");
        assert!(
            exposed
                .windows(b"server-credential-marker".len())
                .any(|w| w == b"server-credential-marker"),
            "parent secret absent from unsandboxed baseline"
        );
        return;
    }
    assert!(
        fs::read(&environ).is_err(),
        "sandboxed kernel read parent's /proc environ"
    );
    let fds = format!("/proc/{parent}/fd");
    assert!(
        fs::read_dir(&fds).is_err(),
        "sandboxed kernel listed parent FDs"
    );
    for path in ["/proc/cpuinfo", "/proc/meminfo"] {
        if Path::new(path).is_file() {
            assert!(
                fs::read(path).is_ok(),
                "safe system metadata blocked: {path}"
            );
        }
    }
}
