use super::*;
use std::fs;
use std::sync::Arc;

use crate::test_support::TestRuntime;
use pretty_assertions::assert_eq;
use serde_json::json;

#[tokio::test]
async fn terminal_snapshots_wait_for_retained_output_and_do_not_duplicate_live_entries() {
    let manager = CommandExecManager::new();
    let session_id = SessionId::from_string("ses_exec_output_completion".to_string());
    let key = CommandExecKey {
        connection_id: 1,
        session_id: Some(session_id),
        process_id: "process-output-completion".to_string(),
    };
    let command = "interactive fixture".to_string();
    manager.sessions.lock().await.insert(
        key.clone(),
        CommandExecSession {
            store_process_id: 1,
            command: command.clone(),
        },
    );
    manager
        .tails
        .lock()
        .await
        .insert(key.clone(), b"ready\n".to_vec());

    // An exited/absent OS process is insufficient: the output pump has not yet
    // committed its terminal record. Reads must keep the task running.
    assert_eq!(
        manager.task_snapshot(&key.process_id).await,
        Some(TaskProcessSnapshot {
            process_id: key.process_id.clone(),
            session_id: Some(session_id),
            command: command.clone(),
            is_running: true,
            exit_code: None,
            tail: b"ready\n".to_vec(),
        })
    );

    let tail = b"ready\nreceived:hello\ndone\n".to_vec();
    manager
        .completed
        .lock()
        .await
        .push_back(CompletedTaskRecord {
            session_id: Some(session_id),
            process_id: key.process_id.clone(),
            command: command.clone(),
            exit_code: Some(0),
            tail: tail.clone(),
        });
    let expected = TaskProcessSnapshot {
        process_id: key.process_id.clone(),
        session_id: Some(session_id),
        command,
        is_running: false,
        exit_code: Some(0),
        tail,
    };

    // The live entry and old tail can remain during persistence. Both APIs
    // must prefer the complete retained record and list it only once.
    assert_eq!(
        manager.task_snapshot(&key.process_id).await,
        Some(expected.clone())
    );
    assert_eq!(
        manager.task_snapshots_for_session(session_id).await,
        vec![expected]
    );
}

async fn create_session(runtime: &Arc<ServerRuntime>, cwd: &std::path::Path) -> SessionId {
    let response = runtime
        .handle_native_session_new(
            1,
            json!(1),
            json!({
                "cwd": cwd,
                "idempotencyKey": format!("exec-context-{}", cwd.display()),
            }),
        )
        .await;
    let native = serde_json::from_value::<
        crate::SuccessResponse<devo_protocol::native::rpc_session::SessionNewResult>,
    >(response.clone())
    .unwrap_or_else(|error| panic!("session/new result: {error}; response={response}"));
    SessionId::from(native.result.session.id.as_str())
}

#[tokio::test]
async fn unknown_explicit_session_is_not_found_even_with_cwd() {
    let home = tempfile::tempdir().expect("tempdir");
    let runtime = TestRuntime::noop().runtime(home.path());
    let execution = home.path().join("execution");
    fs::create_dir_all(&execution).expect("create execution cwd");

    let response = runtime
        .handle_command_exec_translated(
            1,
            json!(1),
            CommandExecParams {
                session_id: Some(SessionId::from_string("ses_missing".to_string())),
                process_id: "unknown-session".to_string(),
                cwd: Some(execution),
                program: CommandExecProgram::OneShot {
                    command: "echo must-not-run".to_string(),
                },
                size: None,
            },
        )
        .await;

    assert_eq!(response["error"]["code"], json!("SessionNotFound"));
}

#[tokio::test]
async fn sessionless_command_exec_requires_explicit_cwd() {
    let home = tempfile::tempdir().expect("tempdir");
    let runtime = TestRuntime::noop().runtime(home.path());

    let error = runtime
        .command_exec_context(None, None)
        .await
        .expect_err("sessionless exec without cwd must fail");

    assert_eq!(error.0, ProtocolErrorCode::InvalidParams);
}

#[tokio::test]
async fn sessionless_command_exec_keeps_explicit_cwd_without_sandbox_workspace() {
    let home = tempfile::tempdir().expect("tempdir");
    let runtime = TestRuntime::noop().runtime(home.path());
    let execution = home.path().join("execution");
    fs::create_dir_all(&execution).expect("create execution cwd");

    let context = runtime
        .command_exec_context(None, Some(execution.clone()))
        .await
        .expect("sessionless context");

    assert_eq!(
        context,
        CommandExecContext {
            cwd: execution,
            sandbox_workspace: None,
            sandbox_profile: None,
        }
    );
}

#[tokio::test]
async fn explicit_session_resolves_sandbox_workspace_from_session() {
    let home = tempfile::tempdir().expect("tempdir");
    let workspace = home.path().join("workspace");
    let execution = home.path().join("execution");
    fs::create_dir_all(&workspace).expect("create session workspace");
    fs::create_dir_all(&execution).expect("create execution cwd");
    let runtime = TestRuntime::noop().runtime(home.path());
    let session_id = create_session(&runtime, &workspace).await;

    let context = runtime
        .command_exec_context(Some(session_id), Some(execution.clone()))
        .await
        .expect("session context");

    // Execution may use the client cwd, but sandbox roots remain tied to the
    // trusted session workspace.
    assert_eq!(context.cwd, execution);
    assert_eq!(context.sandbox_workspace, Some(workspace));
}
