//! A Linux sandbox must start without a controlling terminal and keep its fence.
#![cfg(all(target_os = "linux", feature = "enforce"))]

use std::os::unix::process::CommandExt;
use std::process::Command;

use pretty_assertions::assert_eq;

#[test]
fn shell_sandbox_starts_without_controlling_terminal() {
    let support = nono::Sandbox::support_info();
    if !support.is_supported {
        eprintln!("skipping: Landlock unavailable: {}", support.details);
        return;
    }
    let output = unsafe {
        Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--ignored",
                "--exact",
                "--nocapture",
                "headless_shell_child",
            ])
            .pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            })
            .output()
    }
    .expect("spawn detached sandbox probe");
    assert!(
        output.status.success(),
        "headless fence failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore = "invoked in a detached process by shell_sandbox_starts_without_controlling_terminal"]
fn headless_shell_child() {
    assert_eq!(
        std::fs::File::open("/dev/tty")
            .expect_err("probe must have no controlling terminal")
            .raw_os_error(),
        Some(libc::ENXIO),
    );
    let plan = devo_sandbox::resolve_enforcement_plan(Some("workspace"), &std::env::temp_dir())
        .expect("resolve shell sandbox")
        .expect("active fence");
    devo_sandbox::apply_resolved_enforcement_in_child(Some(&plan))
        .expect("apply fence without a terminal");
    assert!(std::fs::read("/proc/meminfo").is_ok());
    let scratch = std::env::temp_dir().join(format!("devo-headless-{}", std::process::id()));
    std::fs::write(&scratch, b"fenced scratch").expect("scratch write remains allowed");
    assert_eq!(
        std::fs::read(&scratch).expect("scratch read"),
        b"fenced scratch"
    );
    std::fs::remove_file(scratch).expect("scratch cleanup");
}
