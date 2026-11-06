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

#[tokio::test]
async fn skill_refresh_does_not_wait_for_a_parked_cell() {
    let workspace = tempfile::tempdir().expect("workspace");
    let package = workspace.path().join("src/parked_skill");
    std::fs::create_dir_all(&package).expect("package");
    std::fs::write(package.join("__init__.py"), "value = 123\n").expect("module");
    let skills = vec![crate::PythonSkill {
        skill_path: workspace.path().join("SKILL.md"),
        module: "parked_skill".into(),
        package_dir: package,
    }];
    let (kernel, _, _) = super::ensure_kernel_with_skills(
        &None,
        workspace.path(),
        None,
        devo_protocol::CollaborationMode::Build,
        &skills,
    )
    .await
    .expect("kernel");
    let cell = kernel
        .begin_execute("import asyncio\nawait asyncio.sleep(1)\nparked_value = parked_skill.value")
        .await
        .expect("cell");
    let (same, restored, imports) = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        super::ensure_kernel_with_skills(
            &Some(Arc::clone(&kernel)),
            workspace.path(),
            None,
            devo_protocol::CollaborationMode::Build,
            &[],
        ),
    )
    .await
    .expect("startup must not wait for a parked cell")
    .expect("reuse");
    assert_eq!(
        (Arc::ptr_eq(&kernel, &same), restored, imports),
        (true, None, vec![])
    );
    assert_eq!(cell.wait_until_done().await.expect("done").status, "ok");
    let (_, _, imports) = super::ensure_kernel_with_skills(
        &Some(Arc::clone(&kernel)),
        workspace.path(),
        None,
        devo_protocol::CollaborationMode::Build,
        &[],
    )
    .await
    .expect("idle refresh");
    assert_eq!(imports, Vec::<String>::new());
    assert_eq!(
        kernel
            .execute("assert 'parked_skill' not in globals()\nprint(parked_value)")
            .await
            .expect("check")
            .stdout,
        "123\n"
    );
    kernel.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn selected_skill_aliases_survive_namespace_restore() {
    let workspace = tempfile::tempdir().expect("workspace");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let package = workspace.path().join("src/restore_skill");
    std::fs::create_dir_all(&package).expect("package");
    std::fs::write(package.join("__init__.py"), "def read(): return 42\n").expect("module");
    let skills = vec![crate::PythonSkill {
        skill_path: workspace.path().join("SKILL.md"),
        module: "restore_skill".into(),
        package_dir: package,
    }];
    let (kernel, _, imports) = super::ensure_kernel_with_skills(
        &None,
        workspace.path(),
        Some(artifacts.path()),
        devo_protocol::CollaborationMode::Build,
        &skills,
    )
    .await
    .expect("kernel");
    assert_eq!(imports, vec!["restore_skill"]);
    let output = kernel
        .execute(
            "saved_module = restore_skill\nsaved_function = restore_skill.read\nuser_answer = 42",
        )
        .await
        .expect("bind aliases");
    assert_eq!(output.status, "ok");
    let snapshot = kernel
        .snapshot(
            &kernel_namespace_snapshot_path(artifacts.path()),
            &kernel_namespace_manifest_path(artifacts.path()),
        )
        .await
        .expect("snapshot");
    assert_eq!(snapshot.status, "ok", "{snapshot:?}");
    kernel.shutdown().await.expect("shutdown");
    let (kernel, restored, imports) = super::ensure_kernel_with_skills(
        &None,
        workspace.path(),
        Some(artifacts.path()),
        devo_protocol::CollaborationMode::Build,
        &skills,
    )
    .await
    .expect("restored kernel");
    assert_eq!(
        (restored, imports),
        (
            Some(KernelNamespaceRestoreState::Restored),
            vec!["restore_skill".into()]
        )
    );
    let output = kernel
        .execute("print(saved_module.read(), saved_function(), user_answer)")
        .await
        .expect("restored aliases");
    assert_eq!(
        (output.status.as_str(), output.stdout.as_str()),
        ("ok", "42 42 42\n")
    );
    kernel.shutdown().await.expect("shutdown");
}
