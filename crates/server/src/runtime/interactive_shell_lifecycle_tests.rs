//! Cross-runtime regression coverage for session-owned Python shell tasks.
use super::*;
use crate::runtime::kernel_host_bridge::{HostBridge, handle_bash_start};
use crate::test_support::TestRuntime;
use base64::Engine;
use devo_core::tools::host_bash;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::time::Duration;

struct ShellCleanup(SessionId);
impl Drop for ShellCleanup {
    fn drop(&mut self) {
        host_bash::cancel_session(self.0.as_str());
    }
}

async fn create_session(runtime: &Arc<ServerRuntime>, cwd: &std::path::Path) -> SessionId {
    let response = runtime
        .handle_native_session_new(
            1,
            json!(1),
            json!({"cwd":cwd,"idempotencyKey":uuid::Uuid::new_v4().to_string()}),
        )
        .await;
    assert!(response.get("error").is_none(), "{response}");
    SessionId::from(response["result"]["session"]["id"].as_str().unwrap())
}

async fn start_interactive(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    cwd: &std::path::Path,
) -> String {
    let mut bridge = HostBridge::for_tests(session_id);
    bridge.runtime = Arc::downgrade(runtime);
    bridge.cwd = cwd.to_path_buf();
    bridge.sandbox_profile = Some("off".into());
    #[cfg(unix)]
    let command = "while IFS= read -r line; do printf 'reply:%s\\n' \"$line\"; done";
    #[cfg(windows)]
    let command = "while ($null -ne ($line = [Console]::ReadLine())) { [Console]::WriteLine('reply:' + $line) }";
    let response =
        handle_bash_start(&bridge, &json!({"command":command,"mode":"interactive"})).await;
    assert_eq!(response["status"], "ok", "{response}");
    response["result"]["handle_id"].as_str().unwrap().to_owned()
}

async fn exchange(session: SessionId, id: &str, marker: &str) {
    let payload = base64::engine::general_purpose::STANDARD.encode(format!("{marker}\n"));
    let write = host_bash::control_async(
        session.as_str(),
        "bash.write",
        &json!({"handle_id":id,"data":payload}),
    )
    .await;
    assert_eq!(write["status"], "ok", "{write}");
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut cursor = 0;
        let mut output = Vec::new();
        loop {
            let read = host_bash::control_async(
                session.as_str(),
                "bash.read",
                &json!({"handle_id":id,"cursor":cursor,"max_bytes":65536,"wait_ms":100}),
            )
            .await;
            assert_eq!(read["status"], "ok", "{read}");
            cursor = read["result"]["next_cursor"].as_u64().unwrap();
            output.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(read["result"]["data"].as_str().unwrap())
                    .unwrap(),
            );
            if String::from_utf8_lossy(&output).contains(&format!("reply:{marker}")) {
                break;
            }
        }
    })
    .await
    .expect("shell reply");
}

#[tokio::test]
async fn interactive_shell_survives_disconnect_and_is_pinned_until_shutdown() {
    let home = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::noop().runtime(home.path());
    let owner = create_session(&runtime, home.path()).await;
    let _cleanup = ShellCleanup(owner);
    let (tx, _rx) = crate::test_outbound_channel(32);
    let connection = runtime
        .register_connection(crate::ClientTransportKind::Stdio, tx)
        .await;
    let id = start_interactive(&runtime, owner, home.path()).await;
    exchange(owner, &id, "before-disconnect").await;
    runtime.unregister_connection(connection).await;
    assert!(runtime.is_parent_session_pinned(owner).await);
    let attached =
        host_bash::control_async(owner.as_str(), "bash.attach", &json!({"handle_id":id})).await;
    assert_eq!(attached["status"], "ok", "{attached}");
    exchange(owner, &id, "after-disconnect").await;
    let handle = runtime.session(owner).await.unwrap();
    let lifetime = handle.lifetime_token();
    handle.shutdown().await;
    assert!(lifetime.is_cancelled());
    assert_eq!(
        host_bash::control(owner.as_str(), "bash.poll", &json!({"handle_id":id}))["status"],
        "error"
    );
}

#[tokio::test]
async fn interactive_child_work_pins_only_its_root_not_unrelated_roots() {
    let home = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::noop().runtime(home.path());
    let root = create_session(&runtime, home.path()).await;
    let child = create_session(&runtime, home.path()).await;
    let other = create_session(&runtime, home.path()).await;
    let _cleanup = ShellCleanup(child);
    runtime
        .agent_registries
        .lock()
        .await
        .entry(root)
        .or_default()
        .register(
            root,
            child,
            crate::subagent::SubagentMetadata {
                session_id: child,
                parent_session_id: root,
                agent_path: "root/worker".into(),
                nickname: "worker".into(),
                role: "default".into(),
                status: crate::subagent::SubagentStatus::Completed,
                spawned_at: chrono::Utc::now(),
                closed_at: None,
                last_task_message: None,
                close_requested: false,
                session_dir: None,
            },
        );
    assert!(!runtime.is_parent_session_pinned(root).await);
    let id = start_interactive(&runtime, child, home.path()).await;
    exchange(child, &id, "child").await;
    assert!(runtime.is_parent_session_pinned(root).await);
    assert!(!runtime.is_parent_session_pinned(other).await);
    runtime.session(child).await.unwrap().shutdown().await;
}

struct SummaryProvider;
#[async_trait::async_trait]
impl devo_provider::ModelProviderSDK for SummaryProvider {
    fn name(&self) -> &str {
        "shell-lifecycle-summary"
    }
    async fn completion(
        &self,
        _: devo_protocol::ModelRequest,
    ) -> anyhow::Result<devo_protocol::ModelResponse> {
        Ok(devo_protocol::ModelResponse {
            id: "summary".into(),
            content: vec![devo_protocol::ResponseContent::Text(
                "<compaction_summary>summary</compaction_summary>".into(),
            )],
            stop_reason: Some(devo_protocol::StopReason::EndTurn),
            usage: Default::default(),
            metadata: Default::default(),
        })
    }
    async fn completion_stream(
        &self,
        request: devo_protocol::ModelRequest,
    ) -> anyhow::Result<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = anyhow::Result<devo_protocol::StreamEvent>> + Send>,
        >,
    > {
        let response = self.completion(request).await?;
        Ok(Box::pin(futures::stream::iter([Ok(
            devo_protocol::StreamEvent::MessageDone { response },
        )])))
    }
}

#[tokio::test]
async fn interactive_shell_survives_successful_manual_compaction() {
    let home = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::new(Arc::new(SummaryProvider))
        .with_test_model()
        .runtime(home.path());
    let (tx, _rx) = crate::test_outbound_channel(512);
    let connection = runtime
        .register_connection(crate::ClientTransportKind::Stdio, tx)
        .await;
    let init = runtime.handle_incoming(connection, json!({"id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}},"clientInfo":{"name":"shell-lifecycle-test","title":"test","version":"1"}}})).await.unwrap();
    assert!(init.get("error").is_none(), "{init}");
    let owner = create_session(&runtime, home.path()).await;
    let _cleanup = ShellCleanup(owner);
    let id = start_interactive(&runtime, owner, home.path()).await;
    exchange(owner, &id, "before-compact").await;
    for text in ["first history turn", "second history turn"] {
        let result = runtime.handle_incoming(connection, json!({"id":2,"method":"turn/start","params":{"sessionId":owner,"input":[{"type":"text","text":text}],"idempotencyKey":uuid::Uuid::new_v4().to_string()}})).await.unwrap();
        assert!(result.get("error").is_none(), "{result}");
        wait_idle(&runtime, owner).await;
    }
    let compact = runtime
        .handle_native_session_compact_start(json!(3), json!({"sessionId":owner}))
        .await;
    assert!(compact.get("error").is_none(), "{compact}");
    wait_idle(&runtime, owner).await;
    let snapshot = runtime
        .session(owner)
        .await
        .unwrap()
        .export_runtime_session()
        .await
        .unwrap();
    let turn = snapshot.latest_turn.unwrap();
    assert_eq!(
        (turn.native.kind, turn.native.status),
        (
            devo_protocol::native::turn::TurnKind::Compaction,
            devo_protocol::native::turn::TurnStatus::Completed
        )
    );
    assert!(
        snapshot.latest_compaction_snapshot.is_some(),
        "compaction must actually replace history"
    );
    exchange(owner, &id, "after-compact").await;
    runtime.session(owner).await.unwrap().shutdown().await;
}

async fn wait_idle(runtime: &Arc<ServerRuntime>, owner: SessionId) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while runtime.runtime_active_turn_id(owner).await.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("turn completion");
}
