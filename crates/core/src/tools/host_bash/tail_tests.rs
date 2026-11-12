use super::*;
use crate::contracts::ToolBudgets;
use crate::invocation::ToolCallId;
use devo_protocol::{CollaborationMode, SessionId};
use devo_tools::ToolAgentScope;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn host_poll_keeps_recent_output_while_running_and_after_exit() {
    let directory = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let session = Uuid::new_v4().to_string();
    let ctx = ToolContext {
        output_store: None,
        tool_call_id: ToolCallId("host-bash-tail".into()),
        session_id: SessionId::from(session.clone()),
        turn_id: None,
        workspace_root: directory.path().to_owned(),
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
        json!({"command": "head -c 2097152 /dev/zero | tr '\\0' x; printf '\\nLIVE_MARKER\\n'; while [ ! -f finish ]; do sleep 0.02; done; printf 'FINAL_MARKER\\n'", "timeout_ms": 10000}),
        SandboxRetry::Never,
        None,
    );
    let id = started["result"]["handle_id"].as_str().unwrap();
    let params = json!({"handle_id": id});
    let live = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let snapshot = control(&session, "bash.poll", &params);
            if snapshot["result"]["tail"]
                .as_str()
                .is_some_and(|tail| tail.ends_with("LIVE_MARKER\n"))
            {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    // Always release the child, including on failed assertions.
    std::fs::write(directory.path().join("finish"), "done").unwrap();
    let live = live.expect("live marker appears before process completion");
    assert_eq!(live["result"]["running"], true);
    assert!(
        !live["result"]["output"]
            .as_str()
            .unwrap()
            .contains("LIVE_MARKER")
    );
    let done = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let snapshot = control(&session, "bash.poll", &params);
            if snapshot["result"]["running"] == false {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("command exits");
    assert_eq!(done["result"]["exit_code"], 0);
    assert!(
        done["result"]["tail"]
            .as_str()
            .unwrap()
            .ends_with("LIVE_MARKER\nFINAL_MARKER\n")
    );
    assert!(
        session_activities(&session)[0]
            .output
            .ends_with("FINAL_MARKER\n")
    );
    // Completion notices use snapshot(), which deliberately excludes the
    // polling-only tail to avoid doubling transcript payloads.
    let handles = HANDLES.lock().unwrap();
    let entry = handles.get(id).unwrap().lock().unwrap();
    assert!(entry.snapshot(id)["result"].get("tail").is_none());
    drop(entry);
    drop(handles);
    cancel_session(&session);
}
