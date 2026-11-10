use super::*;
use crate::runtime::kernel_host::dispatch_host_request_with_bridge;
use pretty_assertions::assert_eq;

const RUN: &str = "0123456789abcdef0123456789abcdef";

fn intent(stopping: bool) -> Value {
    json!({"format_version":1,"spec":{"version":2,"nodes":[{"id":"work","prompt":"task"}],"max_parallel":1,"failure_policy":"fail_fast"},
        "run":{"id":RUN,"status":"running","stop_requested":stopping,"failed":false,
            "nodes":{"work":{"id":"work","name":"factory-0123456789abcdef-work","status":"admitting","child_id":null,"output":null,"error":null,"cleanup_error":null,"needs_cleanup":true}}}})
}

#[tokio::test]
async fn stale_and_duplicate_effects_do_not_pass_the_ownership_fence() {
    for action in ["factory.spawn", "factory.cancel"] {
        let home = tempfile::tempdir().unwrap();
        let mut bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
        bridge.session_dir = Some(home.path().to_path_buf());
        let snapshot = intent(action == "factory.cancel");
        for revision in 0..2 {
            let saved = dispatch_host_request_with_bridge(&bridge, json!({
                "type":"factory.save","run_id":RUN,"expected_revision":revision,"snapshot":snapshot
            })).await;
            assert_eq!(saved["status"], "ok");
        }
        let mut request =
            json!({"type":action,"run_id":RUN,"expected_revision":1,"node_id":"work"});
        if action == "factory.spawn" {
            request["prompt"] = json!("task");
        }
        assert_eq!(
            dispatch_host_request_with_bridge(&bridge, request.clone()).await,
            json!({"status":"error","error":"Factory revision conflict before effect"})
        );
        request["expected_revision"] = json!(2);
        // The missing runtime simulates an effect failure after reservation. It
        // must remain an error and must consume its revision, never retry blind.
        let failed = dispatch_host_request_with_bridge(&bridge, request.clone()).await;
        assert_eq!(failed["status"], "error");
        assert!(
            failed["error"]
                .as_str()
                .unwrap()
                .contains("runtime not available")
        );
        assert_eq!(
            dispatch_host_request_with_bridge(&bridge, request).await,
            json!({"status":"error","error":"Factory revision conflict before effect"})
        );
        assert_eq!(
            dispatch_host_request_with_bridge(&bridge, json!({"type":"factory.load","run_id":RUN}))
                .await,
            json!({"status":"ok","result":{"run_id":RUN,"revision":3,"snapshot":snapshot}})
        );
    }
}

#[tokio::test]
async fn effects_require_valid_saved_identity_and_stop_intent() {
    let home = tempfile::tempdir().unwrap();
    let mut bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
    bridge.session_dir = Some(home.path().to_path_buf());
    dispatch_host_request_with_bridge(
        &bridge,
        json!({"type":"factory.save","run_id":RUN,"expected_revision":0,"snapshot":intent(false)}),
    )
    .await;
    for (action, node, expected) in [
        (
            "factory.cancel",
            "work",
            "Factory cleanup requires stop intent",
        ),
        (
            "factory.spawn",
            "missing",
            "Factory node has no unresolved admission",
        ),
    ] {
        let mut request = json!({"type":action,"run_id":RUN,"expected_revision":1,"node_id":node});
        if action == "factory.spawn" {
            request["prompt"] = json!("task");
        }
        assert_eq!(
            dispatch_host_request_with_bridge(&bridge, request).await,
            json!({"status":"error","error":expected})
        );
    }
    bridge.collaboration_mode = devo_protocol::CollaborationMode::Plan;
    for action in ["factory.spawn", "factory.cancel"] {
        assert_eq!(
            dispatch_host_request_with_bridge(&bridge, json!({"type":action})).await,
            json!({"status":"error","error":format!("denied: {action} blocked in Plan mode")})
        );
    }
}

#[test]
fn effect_transaction_excludes_claim_until_lifecycle_completion() {
    use super::files::FileMode;
    let home = tempfile::tempdir().unwrap();
    let owner = devo_protocol::native::ids::SessionId::new();
    let open = || {
        files::Directory::open(home.path())
            .unwrap()
            .create_child("factory")
            .unwrap()
    };
    store::save(&open(), owner, RUN.to_owned(), 0, intent(false)).unwrap();
    let transaction = store::EffectTransaction::open(open()).unwrap();
    let run = transaction.load(owner, RUN).unwrap();
    assert_eq!(transaction.reserve(owner, run).unwrap()["revision"], 2);
    // Even after the effect revision is persisted, a claim cannot acquire the
    // stable gate until admission/close completes. No timing/sleep assumptions.
    let contender = open().open_file("store.lock", FileMode::Lock).unwrap();
    assert!(fs2::FileExt::try_lock_exclusive(&contender).is_err());
    drop(transaction);
    fs2::FileExt::try_lock_exclusive(&contender).unwrap();
    fs2::FileExt::unlock(&contender).unwrap();
    assert_eq!(
        store::save(&open(), owner, RUN.to_owned(), 2, intent(false)).unwrap()["revision"],
        3
    );
    assert!(store::save(&open(), owner, RUN.to_owned(), 2, intent(false)).is_err());
}

#[tokio::test]
async fn dropping_kernel_reply_does_not_abandon_reserved_host_work() {
    use std::future::Future;
    use std::task::Poll;
    let home = tempfile::tempdir().unwrap();
    let mut bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
    bridge.session_dir = Some(home.path().to_path_buf());
    let directory = || {
        files::Directory::open(home.path())
            .unwrap()
            .child("factory")
            .unwrap()
    };
    dispatch_host_request_with_bridge(
        &bridge,
        json!({"type":"factory.save","run_id":RUN,"expected_revision":0,"snapshot":intent(false)}),
    )
    .await;
    let gate = store::EffectTransaction::open(directory()).unwrap();
    let params = serde_json::from_value(
        json!({"run_id":RUN,"expected_revision":1,"node_id":"work","prompt":"task"}),
    )
    .unwrap();
    {
        let mut reply = std::pin::pin!(effects::execute(
            &bridge,
            directory(),
            "factory.spawn",
            params
        ));
        // Poll once to launch the owned host task. It cannot reserve while the
        // gate is held; dropping only its response future models kernel SIGINT.
        std::future::poll_fn(|cx| {
            assert!(reply.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    drop(gate);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let loaded = dispatch_host_request_with_bridge(
                &bridge,
                json!({"type":"factory.load","run_id":RUN}),
            )
            .await;
            if loaded["result"]["revision"] == 2 {
                assert_eq!(loaded["result"]["snapshot"], intent(false));
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned effect must consume revision after caller disappears");
}

#[tokio::test]
async fn claims_wait_for_spawn_and_cancel_even_after_losing_the_reply() {
    use effects::Kind;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for kind in [Kind::Spawn, Kind::Cancel] {
        let home = tempfile::tempdir().unwrap();
        let owner = devo_protocol::native::ids::SessionId::new();
        let open = || {
            files::Directory::open(home.path())
                .unwrap()
                .create_child("factory")
                .unwrap()
        };
        let snapshot = intent(kind == Kind::Cancel);
        store::save(&open(), owner, RUN.to_owned(), 0, snapshot.clone()).unwrap();
        let params = serde_json::from_value(json!({"run_id":RUN,"expected_revision":1,"node_id":"work","prompt":if kind == Kind::Spawn { Some("task") } else { None }})).unwrap();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let mutations = Arc::new(AtomicUsize::new(0));
        let observed = mutations.clone();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let task = effects::run_fenced_effect(
            open(),
            owner,
            kind,
            params,
            cancel_token.clone(),
            move |target, kwargs, prompt| async move {
                assert_eq!(target, "factory-0123456789abcdef-work");
                assert_eq!(kwargs, json!({"name":"factory-0123456789abcdef-work"}));
                assert_eq!(
                    prompt,
                    if kind == Kind::Spawn {
                        Some("task".to_string())
                    } else {
                        None
                    }
                );
                entered_tx.send(()).unwrap();
                release_rx.await.unwrap();
                observed.fetch_add(1, Ordering::SeqCst);
                json!({"status":"ok","result":{"rlm_child_id":"child"}})
            },
        );
        entered_rx.await.unwrap();
        assert_eq!(store::load(&open(), owner, RUN).unwrap().revision, 2);
        let lock = open()
            .open_file("store.lock", files::FileMode::Lock)
            .unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&lock).is_err());
        // Simulate a lost response while the actual lifecycle callback is paused.
        cancel_token.cancel();
        drop(task);
        let directory = open();
        let check_mutations = mutations.clone();
        let claim = tokio::task::spawn_blocking(move || {
            let claimed = store::save(&directory, owner, RUN.to_owned(), 2, snapshot).unwrap();
            assert_eq!(check_mutations.load(Ordering::SeqCst), 1);
            claimed
        });
        release_tx.send(()).unwrap();
        let claimed = tokio::time::timeout(std::time::Duration::from_secs(5), claim)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed["revision"], 3);
        // A stale retry cannot execute either effect once the restore claim wins.
        let params = serde_json::from_value(
            json!({"run_id":RUN,"expected_revision":2,"node_id":"work","prompt":"task"}),
        )
        .unwrap();
        let stale = effects::run_fenced_effect(
            open(),
            owner,
            kind,
            params,
            tokio_util::sync::CancellationToken::new(),
            |_, _, _| async { panic!("stale owner executed lifecycle") },
        )
        .await
        .unwrap();
        assert_eq!(
            stale,
            json!({"status":"error","error":"Factory revision conflict before effect"})
        );
        assert_eq!(mutations.load(Ordering::SeqCst), 1);
    }
}
