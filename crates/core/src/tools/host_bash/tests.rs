use super::*;
use pretty_assertions::assert_eq;

#[test]
fn output_is_bounded_on_utf8_boundary() {
    let mut entry = Entry {
        session_id: "bounds".into(),
        mode: BashMode::Pipe,
        io: None,
        command: String::new(),
        turn_id: None,
        cancelled: false,
        cwd: std::path::PathBuf::new(),
        created_at: chrono::Utc::now(),
        cancel: CancellationToken::new(),
        expiry_cancel: CancellationToken::new(),
        started: Instant::now(),
        finished: None,
        output: "x".repeat(MAX_OUTPUT_BYTES - 1),
        tail: OutputTail::default(),
        output_truncated: false,
        exit_code: None,
        error: None,
    };
    entry.append("é");
    assert_eq!(entry.output.len(), MAX_OUTPUT_BYTES);
    assert!(entry.output.ends_with("[host bash output truncated]"));
    assert!(entry.output_truncated);
    let output = entry.output.clone();
    entry.append("abc");
    assert_eq!(entry.output, output);
}

#[cfg(unix)]
#[tokio::test]
async fn origin_cancellation_finishes_started_handle() {
    use crate::contracts::ToolBudgets;
    use crate::invocation::ToolCallId;
    use devo_protocol::{CollaborationMode, SessionId};
    use devo_tools::ToolAgentScope;

    let cancel = CancellationToken::new();
    let session = Uuid::new_v4().to_string();
    let ctx = ToolContext {
        output_store: None,
        tool_call_id: ToolCallId("host-bash-cancel".into()),
        session_id: SessionId::from(session.clone()),
        turn_id: None,
        workspace_root: std::env::temp_dir(),
        budgets: ToolBudgets {
            output_limit_bytes: 32 * 1024,
            wall_time_limit_ms: Some(10_000),
        },
        cancel_token: cancel.clone(),
        agent_scope: ToolAgentScope::Parent,
        collaboration_mode: CollaborationMode::Build,
        agent_coordinator: None,
        client_filesystem: None,
        file_read_ledger: None,
        network_proxy: None,
        network_no_proxy: None,
        sandbox_profile: Some("off".into()),
        sandbox_permission_overlay: None,
        kernel: None,
        python_cell_first_wait_ms: None,
        python_cell_watch: None,
        python_cell_completion: None,
        session_dir: None,
    };
    let started = start(
        ctx,
        json!({
            "command": "printf streaming; sleep 30",
            "workdir": std::fs::canonicalize(std::env::temp_dir())
                .expect("canonical temp dir")
                .to_string_lossy()
        }),
        SandboxRetry::Never,
        None,
    );
    let id = started["result"]["handle_id"]
        .as_str()
        .expect("handle id")
        .to_string();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = control(&session, "bash.poll", &json!({"handle_id": id}));
            if snapshot["result"]["output"]
                .as_str()
                .is_some_and(|output| output.contains("streaming"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("streaming output");
    cancel.cancel();
    let done = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = control(&session, "bash.poll", &json!({"handle_id": id}));
            if snapshot["result"]["running"] == false {
                break snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancel completes");
    assert!(
        done["result"]["output"]
            .as_str()
            .is_some_and(|output| output.contains("cancelled")),
        "cancelled result: {done}"
    );
    let activity = session_activities(&session);
    assert_eq!(activity.len(), 1);
    assert_eq!((activity[0].running, activity[0].cancelled), (false, true));
    cancel_session(&session);
}

#[test]
fn cancel_session_cancels_owned_token_and_removes_handle() {
    let session = Uuid::new_v4().to_string();
    let id = Uuid::new_v4().to_string();
    let cancel = CancellationToken::new();
    HANDLES.lock().unwrap().insert(
        id.clone(),
        Arc::new(Mutex::new(Entry {
            session_id: session.clone(),
            mode: BashMode::Pipe,
            io: None,
            command: String::new(),
            turn_id: None,
            cancelled: false,
            cwd: std::path::PathBuf::new(),
            created_at: chrono::Utc::now(),
            cancel: cancel.clone(),
            expiry_cancel: CancellationToken::new(),
            started: Instant::now(),
            finished: None,
            output: String::new(),
            tail: OutputTail::default(),
            output_truncated: false,
            exit_code: None,
            error: None,
        })),
    );
    cancel_session(&session);
    assert_eq!(
        (
            cancel.is_cancelled(),
            control(&session, "bash.poll", &json!({"handle_id":id}))
        ),
        (
            true,
            json!({"status":"error", "error":"unknown bash handle"})
        )
    );
}
