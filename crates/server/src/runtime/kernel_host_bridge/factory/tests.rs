use super::*;
use crate::runtime::kernel_host::dispatch_host_request_with_bridge;
use devo_core::tools::PermissionChecker;
use devo_protocol::CollaborationMode;
use pretty_assertions::assert_eq;

const RUN: &str = "0123456789abcdef0123456789abcdef";

fn snapshot() -> Value {
    json!({"format_version":1,"spec":{"version":2,"nodes":[{"id":"work","prompt":"task"}],"max_parallel":1,"failure_policy":"fail_fast"},
        "run":{"id":RUN,"status":"admitting","stop_requested":false,"failed":false,
            "nodes":{"work":{"id":"work","name":"reserved-name","status":"admitting","child_id":null,"output":null,"error":null,"cleanup_error":null,"needs_cleanup":true}}}})
}

#[tokio::test]
async fn dispatch_roundtrip_and_session_isolation() {
    let home = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let mut bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
    bridge.session_dir = Some(home.path().to_path_buf());
    let saved = dispatch_host_request_with_bridge(
        &bridge,
        json!({"type":"factory.save","run_id":RUN,"expected_revision":0,"snapshot":snapshot()}),
    )
    .await;
    assert_eq!(
        saved,
        json!({"status":"ok","result":{"run_id":RUN,"revision":1,"snapshot":snapshot()}})
    );
    assert_eq!(
        dispatch_host_request_with_bridge(&bridge, json!({"type":"factory.load","run_id":RUN}))
            .await,
        saved
    );
    assert_eq!(
        dispatch_host_request_with_bridge(&bridge, json!({"type":"factory.list"})).await,
        json!({"status":"ok","result":{"runs":[{"run_id":RUN,"revision":1,"status":"admitting"}]}})
    );
    bridge.session_dir = Some(other.path().to_path_buf());
    assert_eq!(
        dispatch_host_request_with_bridge(&bridge, json!({"type":"factory.load","run_id":RUN}))
            .await["status"],
        "error"
    );
    assert_eq!(std::fs::read_dir(other.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn plan_reads_and_denied_save_create_nothing() {
    let home = tempfile::tempdir().unwrap();
    let mut bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
    bridge.session_dir = Some(home.path().to_path_buf());
    bridge.collaboration_mode = CollaborationMode::Plan;
    assert_eq!(
        dispatch_host_request_with_bridge(
            &bridge,
            json!({"type":"factory.save","run_id":RUN,"expected_revision":0,"snapshot":snapshot()})
        )
        .await,
        json!({"status":"error","error":"denied: factory.save blocked in Plan mode"})
    );
    assert_eq!(
        dispatch_host_request_with_bridge(&bridge, json!({"type":"factory.list"})).await,
        json!({"status":"ok","result":{"runs":[]}})
    );
    assert_eq!(
        dispatch_host_request_with_bridge(&bridge, json!({"type":"factory.load","run_id":RUN}))
            .await["status"],
        "error"
    );
    bridge.collaboration_mode = CollaborationMode::Build;
    bridge.permission = PermissionChecker::new(|_| Box::pin(async { Err("test denial".into()) }));
    assert_eq!(
        dispatch_host_request_with_bridge(
            &bridge,
            json!({"type":"factory.save","run_id":RUN,"expected_revision":0,"snapshot":snapshot()})
        )
        .await,
        json!({"status":"error","error":"test denial"})
    );
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn rejects_unknown_routing_fields_and_bad_revision_without_writes() {
    let home = tempfile::tempdir().unwrap();
    let mut bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
    bridge.session_dir = Some(home.path().to_path_buf());
    for request in [
        json!({"type":"factory.list","scope":"global"}),
        json!({"type":"factory.load","run_id":RUN,"session_id":"other"}),
        json!({"type":"factory.save","run_id":RUN,"expected_revision":true,"snapshot":snapshot()}),
        json!({"type":"factory.save","run_id":"../escape","expected_revision":0,"snapshot":snapshot()}),
        json!({"type":"factory.save","run_id":RUN,"expected_revision":0,"snapshot":snapshot(),"path":"elsewhere"}),
    ] {
        assert_eq!(
            dispatch_host_request_with_bridge(&bridge, request).await["status"],
            "error"
        );
    }
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn approval_path_swap_cannot_redirect_checkpoint_or_lock() {
    use std::os::unix::fs::symlink;
    let home = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let original = home.path().join("session");
    let moved = home.path().join("moved");
    std::fs::create_dir(&original).unwrap();
    let mut bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
    bridge.session_dir = Some(original.clone());
    let outside = other.path().to_path_buf();
    bridge.permission = PermissionChecker::new(move |request| {
        assert_eq!(request.path, Some(original.join("factory")));
        std::fs::rename(&original, &moved).unwrap();
        symlink(&outside, &original).unwrap();
        Box::pin(async { Ok(Default::default()) })
    });
    let response = dispatch_host_request_with_bridge(
        &bridge,
        json!({"type":"factory.save","run_id":RUN,"expected_revision":0,"snapshot":snapshot()}),
    )
    .await;
    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(std::fs::read_dir(other.path()).unwrap().count(), 0);
    assert!(
        home.path()
            .join("moved/factory")
            .join(format!("{RUN}.json"))
            .exists()
    );
}

#[tokio::test]
async fn live_kernel_checkpoint_survives_kernel_replacement() -> anyhow::Result<()> {
    use devo_kernel::{KernelSession, KernelSessionConfig};
    use std::sync::Arc;

    let home = tempfile::tempdir()?;
    let runtime_path =
        devo_kernel::default_runtime_pythonpath(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
            .expect("repository includes the Python runtime");
    let config = KernelSessionConfig {
        cwd: home.path().to_path_buf(),
        python_path_entries: vec![runtime_path],
        ..Default::default()
    };
    let mut bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
    bridge.session_dir = Some(home.path().to_path_buf());
    let bridge = Arc::new(bridge);
    let kernel = KernelSession::spawn(config.clone()).await?;
    kernel
        .set_host_handler(Some(super::super::host_handler_for_bridge(bridge.clone())))
        .await;
    let request =
        json!({"type":"factory.save","run_id":RUN,"expected_revision":0,"snapshot":snapshot()})
            .to_string();
    let code = format!(
        "import json\nfrom rlm.repl import host_request\nreply = await host_request(json.loads({request:?}))\nassert reply['status'] == 'ok', reply\nassert reply['result']['revision'] == 1\nprint('checkpoint-saved')"
    );
    let result = kernel.execute(&code).await?;
    assert_eq!(result.status, "ok", "{result:?}");
    assert!(result.stdout.contains("checkpoint-saved"));
    kernel.shutdown().await?;

    let restored = KernelSession::spawn(config).await?;
    restored
        .set_host_handler(Some(super::super::host_handler_for_bridge(bridge)))
        .await;
    let code = format!(
        "from rlm.repl import host_request\nreply = await host_request({{'type':'factory.load','run_id':'{RUN}'}})\nassert reply['status'] == 'ok', reply\nassert reply['result']['revision'] == 1\nassert reply['result']['snapshot']['run']['nodes']['work']['status'] == 'admitting'\nprint('checkpoint-restored')"
    );
    let result = restored.execute(&code).await?;
    assert_eq!(result.status, "ok", "{result:?}");
    assert!(result.stdout.contains("checkpoint-restored"));
    restored.shutdown().await?;
    Ok(())
}
