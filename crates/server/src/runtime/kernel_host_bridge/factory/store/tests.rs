use super::*;
use pretty_assertions::assert_eq;

const RUN: &str = "0123456789abcdef0123456789abcdef";

fn owner() -> SessionId {
    SessionId::from_string("ses_00000000-0000-0000-0000-000000000001".into())
}

pub(in crate::runtime::kernel_host_bridge::factory) fn snapshot(id: &str) -> Value {
    json!({"format_version":1,"spec":{"version":1,"nodes":[{"id":"work","prompt":"task"}],"max_parallel":1,"failure_policy":"fail_fast"},
        "run":{"id":id,"status":"ready","stop_requested":false,"failed":false,
            "nodes":{"work":{"id":"work","name":"reserved-name","status":"pending","child_id":null,"output":null,"error":null,"cleanup_error":null,"needs_cleanup":false}}}})
}

#[test]
fn roundtrip_cas_owner_and_corruption_fail_closed() {
    let home = tempfile::tempdir().unwrap();
    let directory = Directory::open(home.path()).unwrap();
    let first = save(&directory, owner(), RUN.into(), 0, snapshot(RUN)).unwrap();
    assert_eq!(
        first,
        json!({"run_id":RUN,"revision":1,"snapshot":snapshot(RUN)})
    );
    assert_eq!(load(&directory, owner(), RUN).unwrap().response(), first);
    assert_eq!(
        list(&directory, owner()).unwrap(),
        json!({"runs":[{"run_id":RUN,"revision":1,"status":"ready"}]})
    );
    let before = std::fs::read(home.path().join(format!("{RUN}.json"))).unwrap();
    assert!(
        save(&directory, owner(), RUN.into(), 0, snapshot(RUN))
            .unwrap_err()
            .to_string()
            .contains("revision conflict")
    );
    assert_eq!(
        std::fs::read(home.path().join(format!("{RUN}.json"))).unwrap(),
        before
    );
    assert!(load(&directory, SessionId::new(), RUN).is_err());
    std::fs::write(home.path().join(format!("{RUN}.json")), "broken").unwrap();
    assert!(
        save(&directory, owner(), RUN.into(), 1, snapshot(RUN))
            .unwrap_err()
            .to_string()
            .contains("corrupt")
    );
    assert!(list(&directory, owner()).is_err());
    assert_eq!(
        std::fs::read(home.path().join(format!("{RUN}.json"))).unwrap(),
        b"broken"
    );
}

#[test]
fn invalid_ids_and_schema_fail_before_writes() {
    let home = tempfile::tempdir().unwrap();
    let directory = Directory::open(home.path()).unwrap();
    for id in [
        "",
        "../escape",
        "ABCDEF0123456789abcdef0123456789ab",
        "0000000000000000000000000000000g",
    ] {
        assert!(save(&directory, owner(), id.into(), 0, snapshot(id)).is_err());
    }
    let mut malformed = snapshot(RUN);
    malformed["run"]["id"] = json!("wrong");
    assert!(save(&directory, owner(), RUN.into(), 0, malformed).is_err());
    let mut oversized = snapshot(RUN);
    oversized["run"]["nodes"]["work"]["output"] = json!("x".repeat(MAX_SNAPSHOT_BYTES));
    assert!(save(&directory, owner(), RUN.into(), 0, oversized).is_err());
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

#[test]
fn concurrent_same_revision_has_exactly_one_winner() {
    let home = tempfile::tempdir().unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let results = std::thread::scope(|scope| {
        let workers = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                let path = home.path();
                scope.spawn(move || {
                    let directory = Directory::open(path).unwrap();
                    barrier.wait();
                    save(&directory, owner(), RUN.into(), 0, snapshot(RUN)).is_ok()
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        results.into_iter().filter(|succeeded| *succeeded).count(),
        1
    );
    let directory = Directory::open(home.path()).unwrap();
    assert_eq!(load(&directory, owner(), RUN).unwrap().revision, 1);
    assert_eq!(
        list(&directory, owner()).unwrap()["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn retained_run_limit_is_transactional() {
    let home = tempfile::tempdir().unwrap();
    let directory = Directory::open(home.path()).unwrap();
    for index in 0..MAX_RUNS {
        let id = format!("{index:032x}");
        save(&directory, owner(), id.clone(), 0, snapshot(&id)).unwrap();
    }
    assert!(
        save(&directory, owner(), RUN.into(), 0, snapshot(RUN))
            .unwrap_err()
            .to_string()
            .contains("run limit")
    );
    let existing = format!("{:032x}", 0);
    assert_eq!(
        save(
            &directory,
            owner(),
            existing.clone(),
            1,
            snapshot(&existing)
        )
        .unwrap()["revision"],
        2
    );
}

#[cfg(unix)]
#[test]
fn private_atomic_files_and_symlink_rejection() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let home = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let directory = Directory::open(home.path()).unwrap();
    let target = home.path().join(format!("{RUN}.json"));
    save(&directory, owner(), RUN.into(), 0, snapshot(RUN)).unwrap();
    for name in [format!("{RUN}.json"), "store.lock".into()] {
        assert_eq!(
            std::fs::metadata(home.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    std::fs::remove_file(&target).unwrap();
    let outside = other.path().join("untouched");
    std::fs::write(&outside, "original").unwrap();
    symlink(&outside, &target).unwrap();
    assert!(load(&directory, owner(), RUN).is_err());
    assert!(save(&directory, owner(), RUN.into(), 0, snapshot(RUN)).is_err());
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "original");
    std::fs::remove_file(home.path().join("store.lock")).unwrap();
    symlink(
        other.path().join("new-lock"),
        home.path().join("store.lock"),
    )
    .unwrap();
    assert!(save(&directory, owner(), RUN.into(), 0, snapshot(RUN)).is_err());
    assert!(!other.path().join("new-lock").exists());
}

#[test]
fn checkpoints_cannot_rewrite_definition_identity_or_admission_history() {
    let home = tempfile::tempdir().unwrap();
    let directory = Directory::open(home.path()).unwrap();
    let mut original = snapshot(RUN);
    original["run"]["nodes"]["work"]["status"] = json!("admitting");
    save(&directory, owner(), RUN.into(), 0, original.clone()).unwrap();
    let mut admitted = original.clone();
    admitted["run"]["nodes"]["work"]["child_id"] = json!("ses_host_owned_child");
    admitted["run"]["nodes"]["work"]["status"] = json!("running");
    save(&directory, owner(), RUN.into(), 1, admitted.clone()).unwrap();
    let before = load(&directory, owner(), RUN).unwrap();
    for (path, replacement) in [
        (vec!["spec", "version"], json!(2)),
        (vec!["run", "nodes", "work", "name"], json!("another-child")),
        (vec!["run", "nodes", "work", "child_id"], Value::Null),
        (
            vec!["run", "nodes", "work", "child_id"],
            json!("different-child"),
        ),
        (vec!["run", "nodes", "work", "status"], json!("pending")),
    ] {
        let mut changed = admitted.clone();
        let mut target = &mut changed;
        for key in path {
            target = &mut target[key];
        }
        *target = replacement;
        assert!(save(&directory, owner(), RUN.into(), 2, changed).is_err());
        assert_eq!(load(&directory, owner(), RUN).unwrap(), before);
    }
    admitted["run"]["nodes"]["work"]["status"] = json!("recovery_required");
    assert_eq!(
        save(&directory, owner(), RUN.into(), 2, admitted).unwrap()["revision"],
        3
    );
}
