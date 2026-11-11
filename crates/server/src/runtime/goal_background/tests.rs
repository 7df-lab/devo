use super::*;
use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::{ModelRequest, ModelResponse, StreamEvent};
use devo_provider::ModelProviderSDK;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

struct InboxProvider {
    requests: Mutex<Vec<ModelRequest>>,
    started: Semaphore,
    release: Semaphore,
}

#[async_trait]
impl ModelProviderSDK for InboxProvider {
    fn name(&self) -> &str {
        "completion-inbox"
    }
    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("no auxiliary completion")
    }
    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().expect("requests").push(request);
        self.started.add_permits(1);
        self.release.acquire().await?.forget();
        Ok(Box::pin(futures::stream::iter([Ok(
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "inbox-response".into(),
                    content: vec![devo_protocol::ResponseContent::Text("done".into())],
                    stop_reason: Some(devo_protocol::StopReason::EndTurn),
                    usage: Default::default(),
                    metadata: Default::default(),
                },
            },
        )])))
    }
}

#[tokio::test]
async fn child_terminal_wakes_its_owner_without_goal_or_polling() -> Result<()> {
    let home = tempfile::TempDir::new()?;
    let provider = Arc::new(InboxProvider {
        requests: Mutex::new(Vec::new()),
        started: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let runtime = crate::test_support::TestRuntime::new(provider.clone())
        .with_test_model()
        .runtime(home.path());
    let (tx, _notifications) = crate::test_outbound_channel(512);
    let connection = runtime
        .register_connection(crate::ClientTransportKind::Stdio, tx)
        .await;
    runtime
        .handle_incoming(
            connection,
            json!({"id":1,"method":"initialize","params":{
                "protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}},
                "clientInfo":{"name":"completion-test","title":"test","version":"1"}
            }}),
        )
        .await
        .context("initialize")?;
    let created = runtime
        .handle_incoming(
            connection,
            json!({"id":2,"method":"session/new",
                "params":{"cwd":home.path(),"idempotencyKey":"owner"}
            }),
        )
        .await
        .context("create")?;
    let owner = SessionId::from(
        created["result"]["session"]["id"]
            .as_str()
            .context("owner id")?,
    );
    let child = SessionId::new();
    let turn = TurnId::new();
    runtime.register_background_work(owner, format!("agent:{child}:{turn}"));
    runtime
        .record_subagent_status_event_with_text(
            owner,
            child,
            crate::subagent::SubagentStatus::Completed,
            turn,
            Some("unique child result".into()),
        )
        .await;
    tokio::time::timeout(Duration::from_secs(15), provider.started.acquire())
        .await??
        .forget();
    tokio::time::timeout(Duration::from_secs(5), async {
        while runtime.has_pending_background_work(owner) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("completion was accepted but barrier remained")?;
    {
        let requests = provider.requests.lock().expect("requests");
        assert_eq!(requests.len(), 1);
        let serialized = serde_json::to_string(&requests[0])?;
        assert!(serialized.contains("unique child result"), "{serialized}");
    }
    assert_eq!(runtime.take_background_work_results(owner), None);
    runtime
        .record_subagent_status_event_with_text(
            owner,
            child,
            crate::subagent::SubagentStatus::Completed,
            turn,
            Some("duplicate result".into()),
        )
        .await;
    assert_eq!(
        runtime
            .deps
            .db
            .list_pending(&owner, crate::db::QueueType::Turn)?
            .len(),
        0
    );
    // An exec completion arriving while the owner is blocked in a model
    // request must be admitted immediately, without a goal or keypress.
    let work_id = "exec:owned-task".to_string();
    runtime.register_background_work(owner, work_id.clone());
    devo_core::tools::AgentToolCoordinator::background_work_completed(
        runtime.as_ref(),
        owner,
        work_id,
        "unique shell result".into(),
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while runtime.has_pending_background_work(owner) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("busy completion was not admitted")?;
    let queued = runtime
        .deps
        .db
        .list_pending(&owner, crate::db::QueueType::Turn)?;
    assert_eq!(queued.len(), 1);
    assert_eq!(
        queued[0].host_message_origin().map(|origin| origin.source),
        Some(devo_protocol::native::host_message::HostMessageSource::BackgroundTask)
    );
    assert_eq!(provider.requests.lock().expect("requests").len(), 1);
    provider.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(15), provider.started.acquire())
        .await??
        .forget();
    {
        let requests = provider.requests.lock().expect("requests");
        assert_eq!(requests.len(), 2);
        assert!(serde_json::to_string(&requests[1])?.contains("unique shell result"));
    }
    let failed_child = SessionId::new();
    runtime.register_background_work(owner, format!("agent:{failed_child}:startup"));
    runtime
        .fail_child_agent_startup(owner, failed_child, "test startup failure".into())
        .await;
    provider.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(15), provider.started.acquire())
        .await??
        .forget();
    let requests = provider.requests.lock().expect("requests");
    assert_eq!(requests.len(), 3);
    assert!(serde_json::to_string(&requests[2])?.contains("test startup failure"));
    drop(requests);
    provider.release.add_permits(1);
    Ok(())
}

#[tokio::test]
async fn spawned_child_completion_automatically_starts_parent_request() -> Result<()> {
    use devo_core::tools::AgentToolCoordinator;
    let home = tempfile::TempDir::new()?;
    let provider = Arc::new(InboxProvider {
        requests: Mutex::new(Vec::new()),
        started: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let runtime = crate::test_support::TestRuntime::new(provider.clone())
        .with_test_model()
        .runtime(home.path());
    let (tx, _notifications) = crate::test_outbound_channel(512);
    let connection = runtime
        .register_connection(crate::ClientTransportKind::Stdio, tx)
        .await;
    runtime
        .handle_incoming(
            connection,
            json!({"id":1,"method":"initialize","params":{
                "protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}},
                "clientInfo":{"name":"spawn-completion-test","title":"test","version":"1"}
            }}),
        )
        .await
        .context("initialize")?;
    let created = runtime
        .handle_incoming(
            connection,
            json!({"id":2,"method":"session/new",
                "params":{"cwd":home.path(),"idempotencyKey":"owner"}
            }),
        )
        .await
        .context("create")?;
    let owner = SessionId::from(
        created["result"]["session"]["id"]
            .as_str()
            .context("owner id")?,
    );
    let spawned = Arc::clone(&runtime)
        .spawn_agent(devo_protocol::SpawnAgentParams {
            session_id: owner,
            message: "child task".into(),
            model: None,
            thinking: None,
            fork_turns: Some("none".into()),
            max_turns: Some(1),
            tool_policy: Default::default(),
            ephemeral: false,
            nickname: Some("worker".into()),
        })
        .await?;
    tokio::time::timeout(Duration::from_secs(15), provider.started.acquire())
        .await??
        .forget();
    assert_eq!(provider.requests.lock().expect("requests").len(), 1);
    provider.release.add_permits(1);
    // No wait_agent/collect RPC is made. A terminal child turn itself must
    // admit a result for its direct parent and wake the idle parent.
    tokio::time::timeout(Duration::from_secs(20), provider.started.acquire())
        .await??
        .forget();
    {
        let requests = provider.requests.lock().expect("requests");
        assert_eq!(requests.len(), 2);
        let input = serde_json::to_string(&requests[1])?;
        assert!(input.contains("done"), "{input}");
        assert!(input.contains(spawned.child_session_id.as_str()), "{input}");
    }
    assert!(runtime.active_turn_id_for_session(owner).await.is_some());
    provider.release.add_permits(1);
    Ok(())
}
