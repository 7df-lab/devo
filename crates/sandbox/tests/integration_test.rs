//! Integration tests for devo-sandbox.
//!
//! Note: `Sandbox::apply()` is irreversible and process-wide, so we cannot
//! test actual kernel enforcement in standard `#[test]` functions (they share
//! a process). The end-to-end suite uses subprocesses for enforcement testing.
//! These tests verify the API contracts, config loading, and support detection.

use pretty_assertions::assert_eq;

// `Sandbox::support_info` returns a nono type, so gate this test on the
// platforms where nono compiles with enforcement enabled.
#[test]
#[cfg(all(feature = "enforce", unix))]
fn test_support_info() {
    // Verify that nono can report platform support status without applying
    let support = nono::Sandbox::support_info();
    // On macOS and Linux 5.13+, this should be supported
    // On other platforms, it gracefully reports unsupported
    println!(
        "Sandbox support: supported={}, details={}",
        support.is_supported, support.details
    );
    // We don't assert is_supported because CI may run on any platform
}

// `to_capability_set` is only available with the `enforce` feature.
#[test]
#[cfg(all(feature = "enforce", unix))]
fn test_profile_capability_set_construction() {
    use devo_sandbox::ProfileName;

    // Use CWD as workspace — guaranteed to exist
    let workspace = std::env::current_dir().expect("cwd");

    // All profiles should produce valid CapabilitySets without panicking
    for profile in [
        ProfileName::Workspace,
        ProfileName::ReadOnly,
        ProfileName::Strict,
        ProfileName::Off,
    ] {
        let result = profile.to_capability_set(&workspace);
        assert!(
            result.is_ok(),
            "Profile {:?} failed to build CapabilitySet: {:?}",
            profile,
            result.err()
        );
    }
}

#[test]
fn test_sandbox_manager_lifecycle() {
    use devo_sandbox::{ProfileName, SandboxManager};

    let workspace = std::env::current_dir().expect("cwd");

    // Off profile: apply should succeed without actually sandboxing
    let mut manager = SandboxManager::new(ProfileName::Off);
    assert!(!manager.is_applied());

    let result = manager.apply(&workspace);
    assert!(result.is_ok());
    // Off profile doesn't actually apply
    assert!(!manager.is_applied());
}

#[test]
fn test_sandbox_logger() {
    use devo_sandbox::{SandboxEvent, SandboxLogger};

    let logger = SandboxLogger::new();

    // Log some events (use violation events — profile_applied requires a resolved profile)
    logger.log(SandboxEvent::fs_violation("workspace", "/tmp/test", "read"));
    logger.log(SandboxEvent::fs_violation(
        "workspace",
        "/etc/shadow",
        "write",
    ));
    logger.log(SandboxEvent::net_violation("strict", "evil.com:443"));

    // Check metrics
    assert_eq!(logger.metrics().fs_violation_count(), 2);
    assert_eq!(logger.metrics().net_violation_count(), 1);

    // Take events drains the buffer
    let events = logger.take_events();
    assert_eq!(events.len(), 3);

    // Buffer is now empty
    let events2 = logger.take_events();
    assert!(events2.is_empty());
}
