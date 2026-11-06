use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn session_kernel_uses_owned_temp_and_reuses_namespace() {
    let workspace = tempfile::tempdir().expect("workspace");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let (kernel, state) = ensure_kernel_with_restore_state(
        &None,
        workspace.path(),
        Some(artifacts.path()),
        devo_protocol::CollaborationMode::Build,
    )
    .await
    .expect("kernel");
    assert_eq!(state, Some(KernelNamespaceRestoreState::Fresh));
    let output = kernel
        .execute(
            r#"import os, tempfile, json
from pathlib import Path
expected = Path(os.environ['RLM_SESSION_DIR']) / 'tmp'
with tempfile.TemporaryFile(mode='w+') as probe:
    probe.write('owned temp works')
    probe.seek(0)
    content = probe.read()
startup_value = 42
print(json.dumps({'owned': Path(tempfile.gettempdir()) == expected,
    'env': all(Path(os.environ[k]) == expected for k in ('TEMP', 'TMP', 'TMPDIR')),
    'content': content}))"#,
        )
        .await
        .expect("temp probe");
    assert_eq!(output.status, "ok", "{output:?}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(output.stdout.trim()).expect("probe json"),
        serde_json::json!({"owned": true, "env": true, "content": "owned temp works"})
    );
    let (reused, state) = ensure_kernel_with_restore_state(
        &Some(Arc::clone(&kernel)),
        workspace.path(),
        Some(artifacts.path()),
        devo_protocol::CollaborationMode::Build,
    )
    .await
    .expect("reuse");
    assert_eq!((Arc::ptr_eq(&kernel, &reused), state), (true, None));
    assert_eq!(
        reused
            .execute("print(startup_value)")
            .await
            .expect("namespace")
            .stdout,
        "42\n"
    );
    kernel.shutdown().await.expect("shutdown");
}

/// Plan constrains host actions, not native Python running as the user.
#[tokio::test]
async fn trusted_plan_kernel_writes_reuses_and_restarts_on_mode_or_workspace_change() {
    let workspace = tempfile::tempdir().expect("workspace");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let mut existing = None;
    for (mode, root, reuse) in [
        (
            devo_protocol::CollaborationMode::Plan,
            workspace.path(),
            false,
        ),
        (
            devo_protocol::CollaborationMode::Plan,
            workspace.path(),
            true,
        ),
        (
            devo_protocol::CollaborationMode::Build,
            workspace.path(),
            false,
        ),
        (
            devo_protocol::CollaborationMode::Plan,
            workspace.path(),
            false,
        ),
        (
            devo_protocol::CollaborationMode::Plan,
            artifacts.path(),
            false,
        ),
    ] {
        let (kernel, _) =
            ensure_kernel_with_restore_state(&existing, root, Some(artifacts.path()), mode)
                .await
                .expect("trusted kernel");
        if let Some(previous) = &existing {
            assert_eq!(Arc::ptr_eq(previous, &kernel), reuse);
            if !reuse {
                assert!(previous.execute("print('retired')").await.is_err());
            }
        }
        let output = kernel.execute(
            "from pathlib import Path\nPath('trusted.txt').write_text('user privileges')\nprint(Path('trusted.txt').read_text())"
        ).await.expect("native Python write");
        assert_eq!(
            (output.status.as_str(), output.stdout.as_str()),
            ("ok", "user privileges\n")
        );
        assert_eq!(
            std::fs::read_to_string(root.join("trusted.txt")).expect("file"),
            "user privileges"
        );
        existing = Some(kernel);
    }
    existing
        .expect("kernel")
        .shutdown()
        .await
        .expect("shutdown");
}
