use super::*;
use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_core::tools::AgentToolCoordinator;
use devo_protocol::{ModelRequest, ModelResponse, SpawnAgentParams, StreamEvent};
use devo_provider::ModelProviderSDK;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

struct Lane {
    requests: Mutex<Vec<ModelRequest>>,
    started: Semaphore,
    release: Semaphore,
}
impl Default for Lane {
    fn default() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            started: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }
}
#[derive(Default)]
struct NestedProvider {
    root: Lane,
    child: Lane,
    grandchild: Lane,
}
#[async_trait]
impl ModelProviderSDK for NestedProvider {
    fn name(&self) -> &str {
        "nested-inbox"
    }
    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("no auxiliary completion")
    }
    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        let encoded = serde_json::to_string(&request.messages)?;
        let (lane, result) = if encoded.contains("grandchild-input-marker") {
            (&self.grandchild, "grandchild-result-marker")
        } else if encoded.contains("child-input-marker") {
            (&self.child, "child-result-marker")
        } else {
            (&self.root, "root-result-marker")
        };
        lane.requests.lock().expect("requests").push(request);
        lane.started.add_permits(1);
        lane.release.acquire().await?.forget();
        Ok(Box::pin(futures::stream::iter([
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: result.into(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: result.into(),
                    content: vec![devo_protocol::ResponseContent::Text(result.into())],
                    stop_reason: Some(devo_protocol::StopReason::EndTurn),
                    usage: Default::default(),
                    metadata: Default::default(),
                },
            }),
        ])))
    }
}

#[tokio::test]
async fn nested_completion_targets_busy_and_idle_direct_parent_only() -> Result<()> {
    let home = tempfile::TempDir::new()?;
    let provider = Arc::new(NestedProvider::default());
    let runtime = crate::test_support::TestRuntime::new(provider.clone())
        .with_test_model()
        .runtime(home.path());
    let (tx, _notifications) = crate::test_outbound_channel(2048);
    let connection = runtime
        .register_connection(crate::ClientTransportKind::Stdio, tx)
        .await;
    runtime
        .handle_incoming(
            connection,
            json!({"id":1,"method":"initialize","params":{
                "protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}},
                "clientInfo":{"name":"nested-test","title":"test","version":"1"}
            }}),
        )
        .await
        .context("initialize")?;
    let created = runtime
        .handle_incoming(
            connection,
            json!({"id":2,"method":"session/new",
                "params":{"cwd":home.path(),"idempotencyKey":"root"}
            }),
        )
        .await
        .context("create root")?;
    let root = SessionId::from(
        created["result"]["session"]["id"]
            .as_str()
            .context("root id")?,
    );
    let child = runtime
        .clone()
        .spawn_agent(SpawnAgentParams {
            session_id: root,
            message: "child-input-marker".into(),
            model: None,
            thinking: None,
            fork_turns: Some("none".into()),
            max_turns: None,
            tool_policy: Default::default(),
            ephemeral: false,
            nickname: Some("child".into()),
        })
        .await?
        .child_session_id;
    tokio::time::timeout(Duration::from_secs(20), provider.child.started.acquire())
        .await??
        .forget();
    let grandchild = runtime
        .clone()
        .spawn_agent(SpawnAgentParams {
            session_id: child,
            message: "grandchild-input-marker".into(),
            model: None,
            thinking: None,
            fork_turns: Some("none".into()),
            max_turns: Some(1),
            tool_policy: Default::default(),
            ephemeral: false,
            nickname: Some("grandchild".into()),
        })
        .await?
        .child_session_id;
    tokio::time::timeout(
        Duration::from_secs(20),
        provider.grandchild.started.acquire(),
    )
    .await??
    .forget();
    let original_child_turn = runtime
        .active_turn_id_for_session(child)
        .await
        .context("active child")?;
    provider.grandchild.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let queue = runtime
                .deps
                .db
                .list_pending(&child, crate::db::QueueType::Turn)
                .expect("child inbox");
            if !queue.is_empty() {
                assert_eq!(queue.len(), 1);
                assert_eq!(
                    queue[0].host_message_origin().map(|origin| origin.sender),
                    Some(grandchild.to_string())
                );
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("grandchild result did not enter busy child inbox")?;
    assert!(
        runtime
            .deps
            .db
            .list_pending(&root, crate::db::QueueType::Turn)?
            .is_empty()
    );
    assert!(
        provider
            .root
            .requests
            .lock()
            .expect("root requests")
            .is_empty()
    );
    assert_eq!(
        provider
            .child
            .requests
            .lock()
            .expect("child requests")
            .len(),
        1
    );
    provider.child.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(20), provider.child.started.acquire())
        .await??
        .forget();
    let second =
        serde_json::to_string(&provider.child.requests.lock().expect("child requests")[1])?;
    assert!(second.contains("grandchild-result-marker"), "{second}");
    assert_eq!(
        runtime.active_turn_id_for_session(child).await,
        Some(original_child_turn)
    );
    assert!(
        provider
            .root
            .requests
            .lock()
            .expect("root requests")
            .is_empty()
    );
    // The busy child consumes the result in its existing turn, then reports
    // completion to the idle root only after the second request completes.
    provider.child.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(20), provider.root.started.acquire())
        .await??
        .forget();
    let root_input =
        serde_json::to_string(&provider.root.requests.lock().expect("root requests")[0])?;
    assert!(
        !root_input.contains(grandchild.as_str()),
        "grandchild sent directly to root: {root_input}"
    );
    tokio::time::timeout(Duration::from_secs(20), async {
        while runtime.active_turn_id_for_session(child).await.is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("child did not become idle")?;
    let grandchild2 = runtime
        .clone()
        .spawn_agent(SpawnAgentParams {
            session_id: child,
            message: "grandchild-input-marker".into(),
            model: None,
            thinking: None,
            fork_turns: Some("none".into()),
            max_turns: Some(1),
            tool_policy: Default::default(),
            ephemeral: false,
            nickname: Some("grandchild2".into()),
        })
        .await?
        .child_session_id;
    tokio::time::timeout(
        Duration::from_secs(20),
        provider.grandchild.started.acquire(),
    )
    .await??
    .forget();
    provider.grandchild.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(20), provider.child.started.acquire())
        .await??
        .forget();
    assert_eq!(
        provider
            .child
            .requests
            .lock()
            .expect("child requests")
            .len(),
        3
    );
    let third = serde_json::to_string(&provider.child.requests.lock().expect("child requests")[2])?;
    assert!(third.contains(grandchild2.as_str()), "{third}");
    for pending in runtime
        .deps
        .db
        .list_pending(&root, crate::db::QueueType::Turn)?
    {
        assert_eq!(
            pending.host_message_origin().map(|origin| origin.sender),
            Some(child.to_string())
        );
    }
    provider.child.release.add_permits(1);
    provider.root.release.add_permits(8);
    Ok(())
}

#[path = "parent_report_tests.rs"]
mod parent_report_tests;
