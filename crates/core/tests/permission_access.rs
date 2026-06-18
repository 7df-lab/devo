use std::path::Path;

use devo_core::{
    AccessMode, FsPolicyEntry, NetworkPolicy, PermissionProfile, can_read, can_write,
    resolve_access,
};
use pretty_assertions::assert_eq;

fn write_profile(root: &Path) -> PermissionProfile {
    PermissionProfile {
        filesystem_policy: vec![FsPolicyEntry {
            path: root.to_path_buf(),
            access: AccessMode::Write,
            is_explicit: true,
        }],
        network_policy: NetworkPolicy::default(),
    }
}

#[test]
fn missing_dotdot_path_outside_root_is_not_authorized() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(workspace.join("sub")).expect("create workspace");
    let workspace = workspace.canonicalize().expect("canonicalize workspace");
    let profile = write_profile(&workspace);

    let outside_missing = workspace
        .join("sub")
        .join("..")
        .join("..")
        .join("outside.txt");

    assert!(!outside_missing.exists());
    assert_eq!(resolve_access(&outside_missing, &profile), AccessMode::None);
    assert!(!can_write(&outside_missing, &profile));
}

#[test]
fn missing_dotdot_path_inside_root_remains_authorized() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(workspace.join("sub")).expect("create workspace");
    let workspace = workspace.canonicalize().expect("canonicalize workspace");
    let profile = write_profile(&workspace);

    let inside_missing = workspace.join("sub").join("..").join("inside.txt");

    assert!(!inside_missing.exists());
    assert_eq!(resolve_access(&inside_missing, &profile), AccessMode::Write);
    assert!(can_write(&inside_missing, &profile));
}

#[cfg(unix)]
#[test]
fn missing_path_under_symlink_outside_root_is_not_authorized() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let workspace = temp.path().join("workspace");
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&workspace).expect("create workspace");
    std::fs::create_dir_all(&outside).expect("create outside dir");
    std::os::unix::fs::symlink(&outside, workspace.join("link")).expect("create symlink");
    let workspace = workspace.canonicalize().expect("canonicalize workspace");
    let profile = write_profile(&workspace);

    let outside_missing = workspace.join("link").join("missing.txt");

    assert!(!outside_missing.exists());
    assert_eq!(resolve_access(&outside_missing, &profile), AccessMode::None);
    assert!(!can_write(&outside_missing, &profile));
}

/// Trace: L2-DES-SAFETY-001
/// Verifies: same-path deny entries take precedence over write entries.
#[test]
fn equal_specificity_deny_overrides_write() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("create workspace");
    let workspace = workspace.canonicalize().expect("canonicalize workspace");
    let profile = PermissionProfile {
        filesystem_policy: vec![
            FsPolicyEntry {
                path: workspace.clone(),
                access: AccessMode::Write,
                is_explicit: true,
            },
            FsPolicyEntry {
                path: workspace.clone(),
                access: AccessMode::None,
                is_explicit: true,
            },
        ],
        network_policy: NetworkPolicy::default(),
    };
    let path = workspace.join("blocked.txt");

    assert_eq!(resolve_access(&path, &profile), AccessMode::None);
    assert!(!can_read(&path, &profile));
    assert!(!can_write(&path, &profile));
}
