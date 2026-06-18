use std::collections::BTreeSet;
use std::path::PathBuf;

use devo_safety::{
    ApprovalCache, ApprovalScope, EffectiveSandboxPolicy, FileSystemPolicyRecord, NetworkPolicy,
    PermissionDecision, PermissionPolicy, PermissionRequest, PolicyModelSelection, PolicySnapshot,
    ResourceKind, SafetyPolicyMode, SandboxMode, SandboxPolicyRecord, StaticPermissionPolicy,
};
use pretty_assertions::assert_eq;

fn abs_path(parts: &[&str]) -> PathBuf {
    #[cfg(windows)]
    let mut path = PathBuf::from(r"C:\");
    #[cfg(unix)]
    let mut path = PathBuf::from("/");

    for part in parts {
        path.push(part);
    }
    path
}

fn snapshot(
    readable_roots: BTreeSet<PathBuf>,
    writable_roots: BTreeSet<PathBuf>,
    denied_roots: BTreeSet<PathBuf>,
) -> PolicySnapshot {
    PolicySnapshot {
        mode: SafetyPolicyMode::StaticPolicy,
        policy_model: PolicyModelSelection::UseTurnModel,
        sandbox_policy: SandboxPolicyRecord {
            mode: SandboxMode::Restricted,
            workspace_write: true,
        },
        file_system_policy: FileSystemPolicyRecord::default(),
        network_policy: NetworkPolicy::DenyAll,
        approval_cache: ApprovalCache::default(),
        effective_policy: EffectiveSandboxPolicy {
            mode: SandboxMode::Restricted,
            readable_roots,
            writable_roots,
            denied_roots,
            network: NetworkPolicy::DenyAll,
        },
        explicit_denials: Vec::new(),
    }
}

fn file_request(tool_name: &str, resource: ResourceKind, path: PathBuf) -> PermissionRequest {
    PermissionRequest {
        tool_name: tool_name.into(),
        resource,
        action_summary: "touch file".into(),
        justification: "test".into(),
        path: Some(path),
        host: None,
        target: None,
    }
}

#[tokio::test]
async fn static_policy_asks_when_write_path_escapes_writable_root() {
    let workspace = abs_path(&["workspace"]);
    let outside = abs_path(&["outside.txt"]);
    let snapshot = snapshot(
        BTreeSet::new(),
        BTreeSet::from([workspace.clone()]),
        BTreeSet::new(),
    );
    let request_path = workspace
        .join("sub")
        .join("..")
        .join("..")
        .join("outside.txt");

    let decision = StaticPermissionPolicy
        .decide(
            &snapshot,
            &file_request("write", ResourceKind::FileWrite, request_path),
        )
        .await
        .expect("decision");

    assert_eq!(
        decision,
        PermissionDecision::Ask {
            approval_id: "approval-write".into(),
            message: format!("write needs write access to {}", outside.display()),
            available_scopes: vec![
                ApprovalScope::Once,
                ApprovalScope::Turn,
                ApprovalScope::Session,
                ApprovalScope::PathPrefix { path: outside },
            ],
        }
    );
}

#[tokio::test]
async fn static_policy_does_not_deny_path_that_normalizes_outside_denied_root() {
    let workspace = abs_path(&["workspace"]);
    let denied = workspace.join("secrets");
    let snapshot = snapshot(
        BTreeSet::new(),
        BTreeSet::from([workspace.clone()]),
        BTreeSet::from([denied]),
    );
    let request_path = workspace.join("secrets").join("..").join("allowed.txt");

    let decision = StaticPermissionPolicy
        .decide(
            &snapshot,
            &file_request("write", ResourceKind::FileWrite, request_path),
        )
        .await
        .expect("decision");

    assert_eq!(decision, PermissionDecision::Allow);
}

#[tokio::test]
async fn static_policy_asks_when_read_path_escapes_readable_root() {
    let workspace = abs_path(&["workspace"]);
    let outside = abs_path(&["outside.txt"]);
    let snapshot = snapshot(
        BTreeSet::from([workspace.clone()]),
        BTreeSet::new(),
        BTreeSet::new(),
    );
    let request_path = workspace
        .join("sub")
        .join("..")
        .join("..")
        .join("outside.txt");

    let decision = StaticPermissionPolicy
        .decide(
            &snapshot,
            &file_request("read", ResourceKind::FileRead, request_path),
        )
        .await
        .expect("decision");

    assert_eq!(
        decision,
        PermissionDecision::Ask {
            approval_id: "approval-read".into(),
            message: format!("read needs read access to {}", outside.display()),
            available_scopes: vec![
                ApprovalScope::Once,
                ApprovalScope::Turn,
                ApprovalScope::Session,
                ApprovalScope::PathPrefix { path: outside },
            ],
        }
    );
}
