use super::*;
use pretty_assertions::assert_eq;

#[cfg(windows)]
#[test]
fn session_fence_does_not_grant_shared_temp_roots() {
    let workspace = tempfile::tempdir().expect("workspace");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let fence = resolve_kernel_fence(
        workspace.path(),
        Some(artifacts.path()),
        devo_protocol::CollaborationMode::Build,
    )
    .expect("fence");
    assert_eq!(
        fence.writable_roots,
        vec![
            workspace.path().to_path_buf(),
            artifacts.path().to_path_buf(),
            global_harness_state_dir(),
        ]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn owned_temp_does_not_hide_base_profile_plan_overlap() {
    let workspace = tempfile::tempdir().expect("workspace under shared scratch");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let error = match ensure_kernel_with_restore_state(
        &None,
        workspace.path(),
        Some(artifacts.path()),
        devo_protocol::CollaborationMode::Plan,
    )
    .await
    {
        Ok(_) => panic!("Plan must reject the writable scratch ancestor in the Unix base profile"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("overlaps workspace"), "{error}");
}

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
