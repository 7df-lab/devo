use super::*;
use crate::contracts::ToolBudgets;
use crate::invocation::ToolCallId;
use base64::Engine;
use devo_protocol::{CollaborationMode, SessionId};
use devo_tools::ToolAgentScope;
use pretty_assertions::assert_eq;

fn context(session: &str, cancel: CancellationToken) -> ToolContext {
    ToolContext {
        output_store: None,
        tool_call_id: ToolCallId("host-interactive".into()),
        session_id: SessionId::from(session.to_owned()),
        turn_id: None,
        workspace_root: std::env::temp_dir(),
        budgets: ToolBudgets {
            output_limit_bytes: 32768,
            wall_time_limit_ms: None,
        },
        cancel_token: cancel,
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
    }
}

struct Cleanup(String);
impl Drop for Cleanup {
    fn drop(&mut self) {
        cancel_session(&self.0);
    }
}

async fn wait_ready(session: &str, id: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let reply = control(session, "bash.poll", &json!({"handle_id":id}));
            assert_eq!(reply["status"], "ok", "{reply}");
            if reply["result"]["ready"] == true {
                break;
            }
            assert_eq!(reply["result"]["running"], true, "{reply}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("spawn ready");
}

async fn wait_done(session: &str, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let reply = control(session, "bash.poll", &json!({"handle_id":id}));
            if reply["result"]["running"] == false {
                return reply;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("process exits")
}

#[tokio::test]
async fn interactive_survives_turn_and_kernel_then_accepts_multiple_inputs() {
    let session = Uuid::new_v4().to_string();
    let _cleanup = Cleanup(session.clone());
    let turn = CancellationToken::new();
    let kernel = CancellationToken::new();
    let started = start_with_session_lifetime(
        context(&session, turn.clone()),
        json!({"command":"cat", "mode":"interactive", "timeout_ms":10000}),
        SandboxRetry::Never,
        Some(kernel.clone()),
        Some(CancellationToken::new()),
    );
    let id = started["result"]["handle_id"].as_str().unwrap();
    wait_ready(&session, id).await;
    turn.cancel();
    kernel.cancel();
    assert_eq!(
        control("other", "bash.attach", &json!({"handle_id":id})),
        json!({"status":"error","error":"unknown bash handle"})
    );
    assert_eq!(
        control("other", "bash.list", &json!({}))["result"]["handles"],
        json!([])
    );
    let attach = control(&session, "bash.attach", &json!({"handle_id":id}));
    assert_eq!(attach["result"]["mode"], "interactive");
    for text in ["one\n", "two\n"] {
        let reply = control_async(
            &session,
            "bash.write",
            &json!({"handle_id":id,
            "data":base64::engine::general_purpose::STANDARD.encode(text)}),
        )
        .await;
        assert_eq!(
            reply,
            json!({"status":"ok","result":{"written":text.len()}})
        );
    }
    let denied = control_async(
        "other",
        "bash.write",
        &json!({"handle_id":id,"data":"eA=="}),
    )
    .await;
    assert_eq!(
        denied,
        json!({"status":"error","error":"unknown bash handle"})
    );
    let mut bytes = Vec::new();
    let mut cursor = 0;
    while bytes.len() < 8 {
        let reply = control_async(
            &session,
            "bash.read",
            &json!({"handle_id":id,"cursor":cursor,"wait_ms":1000}),
        )
        .await;
        assert_eq!(reply["status"], "ok");
        bytes.extend(
            base64::engine::general_purpose::STANDARD
                .decode(reply["result"]["data"].as_str().unwrap())
                .unwrap(),
        );
        cursor = reply["result"]["next_cursor"].as_u64().unwrap();
        assert!(!reply["result"]["eof"].as_bool().unwrap() || bytes.len() == 8);
    }
    assert_eq!(bytes, b"one\ntwo\n");
    assert_eq!(
        control_async(&session, "bash.close_stdin", &json!({"handle_id":id})).await["status"],
        "ok"
    );
    let done = wait_done(&session, id).await;
    assert_eq!(done["result"]["exit_code"], 0);
    assert_eq!(
        control(&session, "bash.attach", &json!({"handle_id":id}))["status"],
        "ok"
    );
}

#[tokio::test]
async fn interactive_idle_timeout_is_explicit_and_session_delete_cleans() {
    let session = Uuid::new_v4().to_string();
    let _cleanup = Cleanup(session.clone());
    let started = start(
        context(&session, CancellationToken::new()),
        json!({"command":"cat", "mode":"interactive", "idle_timeout_ms":100}),
        SandboxRetry::Never,
        None,
    );
    let id = started["result"]["handle_id"].as_str().unwrap();
    let done = wait_done(&session, id).await;
    assert_eq!(done["result"]["error"], "interactive bash idle timeout");
    let started = start(
        context(&session, CancellationToken::new()),
        json!({"command":"cat", "mode":"interactive"}),
        SandboxRetry::Never,
        None,
    );
    let id = started["result"]["handle_id"].as_str().unwrap();
    wait_ready(&session, id).await;
    let io = HANDLES
        .lock()
        .unwrap()
        .get(id)
        .unwrap()
        .lock()
        .unwrap()
        .io
        .clone()
        .unwrap();
    cancel_session(&session);
    let read = io.read(0, 1024, 10000).await;
    assert_eq!(read["eof"], true);
}

#[tokio::test]
async fn interactive_pty_write_resize_and_no_fake_eof() {
    let session = Uuid::new_v4().to_string();
    let _cleanup = Cleanup(session.clone());
    let started = start(
        context(&session, CancellationToken::new()),
        json!({"command":"read value; printf 'ACK:%s' \"$value\"", "mode":"pty", "yield_time_ms":10,"timeout_ms":10000}),
        SandboxRetry::Never,
        None,
    );
    let id = started["result"]["handle_id"].as_str().unwrap();
    wait_ready(&session, id).await;
    assert_eq!(
        control_async(
            &session,
            "bash.resize",
            &json!({"handle_id":id,"cols":100,"rows":30})
        )
        .await["status"],
        "ok"
    );
    assert_eq!(
        control_async(&session, "bash.close_stdin", &json!({"handle_id":id})).await["status"],
        "error"
    );
    let reply = control_async(
        &session,
        "bash.write",
        &json!({"handle_id":id,"data":"aGVsbG8K"}),
    )
    .await;
    assert_eq!(reply["status"], "ok");
    let done = wait_done(&session, id).await;
    assert_eq!(done["result"]["exit_code"], 0);
    let read = control_async(&session, "bash.read", &json!({"handle_id":id})).await;
    let output = base64::engine::general_purpose::STANDARD
        .decode(read["result"]["data"].as_str().unwrap())
        .unwrap();
    assert!(String::from_utf8_lossy(&output).contains("ACK:hello"));
}

#[tokio::test]
async fn interactive_session_lifetime_covers_admission_and_running_process() {
    let session = Uuid::new_v4().to_string();
    let _cleanup = Cleanup(session.clone());
    let closed = CancellationToken::new();
    closed.cancel();
    assert_eq!(
        start_with_session_lifetime(
            context(&session, CancellationToken::new()),
            json!({"command":"cat","mode":"interactive"}),
            SandboxRetry::Never,
            None,
            Some(closed)
        ),
        json!({"status":"error","error":"originating session stopped"})
    );
    let lifetime = CancellationToken::new();
    let started = start_with_session_lifetime(
        context(&session, CancellationToken::new()),
        json!({"command":"cat","mode":"interactive"}),
        SandboxRetry::Never,
        None,
        Some(lifetime.clone()),
    );
    let id = started["result"]["handle_id"].as_str().unwrap();
    wait_ready(&session, id).await;
    lifetime.cancel();
    let done = wait_done(&session, id).await;
    assert!(done["result"]["error"].is_string());
}
