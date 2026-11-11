use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use devo_core::tools::AgentToolCoordinator;
use devo_protocol::{
    ModelRequest, ModelResponse, PendingInputItem, PendingInputKind, ResponseContent,
    ResponseMetadata, SpawnAgentParams, StopReason, StreamEvent, Usage,
};
use devo_provider::ModelProviderSDK;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::sync::Semaphore;

use crate::{ClientTransportKind, test_support::TestRuntime};

struct GatedChildProvider {
    requests: Mutex<Vec<ModelRequest>>,
    started: Semaphore,
    release: Semaphore,
}

#[async_trait]
impl ModelProviderSDK for GatedChildProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title".into(),
            content: vec![ResponseContent::Text("Child task".into())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        let first = {
            let mut requests = self.requests.lock().expect("requests");
            requests.push(request);
            requests.len() == 1
        };
        if first {
            self.started.add_permits(1);
            self.release.acquire().await?.forget();
        }
        Ok(Box::pin(futures::stream::iter([Ok(
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "child-reply".into(),
                    content: vec![ResponseContent::Text("Child complete".into())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            },
        )])))
    }

    fn name(&self) -> &str {
        "gated-child"
    }
}

#[tokio::test]
async fn invalid_queued_child_binding_does_not_strand_next_valid_input() -> Result<()> {
    let home = tempfile::TempDir::new()?;
    let provider = Arc::new(GatedChildProvider {
        requests: Mutex::new(Vec::new()),
        started: Semaphore::new(/*permits*/ 0),
        release: Semaphore::new(/*permits*/ 0),
    });
    let runtime = TestRuntime::new(provider.clone())
        .with_test_model()
        .runtime(home.path());
    let (tx, mut notifications) = crate::test_outbound_channel(128);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, tx)
        .await;
    let initialized = runtime.handle_incoming(connection, json!({
        "id": 1, "method": "initialize", "params": {
            "protocolVersion": 1, "clientCapabilities": {}, "_meta": {"devo": {"protocol": "native"}},
            "clientInfo": {"name": "queue-child-test", "title": "test", "version": "1"}
        }
    })).await.context("initialize")?;
    assert!(initialized.get("error").is_none(), "{initialized}");
    let created = runtime.handle_incoming(connection, json!({
        "id": 2, "method": "session/new", "params": {"cwd": home.path(), "idempotencyKey": "queue-parent"}
    })).await.context("create parent")?;
    let root = devo_protocol::SessionId::from(
        created["result"]["session"]["id"]
            .as_str()
            .context("parent id")?,
    );
    let child = runtime
        .clone()
        .spawn_agent(serde_json::from_value::<SpawnAgentParams>(json!({
            "session_id": root, "message": "Keep initial child turn active", "fork_turns": "none"
        }))?)
        .await?;
    tokio::time::timeout(Duration::from_secs(/*secs*/ 15), provider.started.acquire())
        .await??
        .forget();

    let subscribed = runtime
        .handle_incoming(
            connection,
            json!({
                "id": 3, "method": "subscription/create", "params": {
                    "selectors": [{"kind": "session", "sessionId": child.child_session_id}],
                    "includeSnapshot": false
                }
            }),
        )
        .await
        .context("subscribe to child queue changes")?;
    assert!(subscribed.get("error").is_none(), "{subscribed}");

    // Native queue payloads no longer expose model overrides. Seed the same
    // durable actor queue metadata used by restored/legacy queued inputs.
    let handle = runtime
        .session(child.child_session_id)
        .await
        .context("child actor")?;
    let invalid = PendingInputItem::new(
        PendingInputKind::UserText {
            text: "invalid queued input".into(),
        },
        Some(json!({"model": "missing/model"})),
        Utc::now(),
    );
    let invalid_id = invalid.id;
    runtime.register_background_work(
        root,
        format!("agent:{}:queued:{invalid_id}", child.child_session_id),
    );
    let valid = PendingInputItem::new(
        PendingInputKind::UserText {
            text: "valid queued input".into(),
        },
        /*metadata*/ None,
        Utc::now(),
    );
    let valid_id = valid.id;
    handle.enqueue_pending_turn_input(invalid).await;
    handle.enqueue_pending_turn_input(valid).await;
    // Snapshot is a mailbox barrier ensuring both enqueues precede completion.
    let snapshot = handle
        .turn_reservation_snapshot()
        .await
        .context("queued snapshot")?;
    assert_eq!(snapshot.pending_turn_queue.lock().expect("queue").len(), 2);
    provider.release.add_permits(1);

    let (removed, drained) = tokio::time::timeout(Duration::from_secs(/*secs*/ 15), async {
        let mut completed = 0;
        let mut removed = false;
        let mut drained = false;
        while let Some(event) = notifications.recv().await {
            if event["method"] == "queue/updated" {
                removed |= event["params"]["change"] == "removed"
                    && event["params"]["queueItemId"] == json!(invalid_id);
                drained |= event["params"]["change"] == "drained"
                    && event["params"]["queueItemId"] == json!(valid_id);
            }
            if event["method"] == "turn/completed"
                && event["params"]["turn"]["sessionId"] == json!(child.child_session_id)
            {
                completed += 1;
                if completed == 2 {
                    return Ok::<_, anyhow::Error>((removed, drained));
                }
            }
        }
        anyhow::bail!("notification stream closed before valid queued turn completed")
    })
    .await
    .context("queue did not advance past rejected binding")??;
    assert_eq!((removed, drained), (true, true));
    let snapshot = handle
        .turn_reservation_snapshot()
        .await
        .context("drained snapshot")?;
    assert!(
        snapshot
            .pending_turn_queue
            .lock()
            .expect("queue")
            .is_empty()
    );
    // The rejected child input is actively delivered to the parent without a
    // goal or a polling tool. Wait for the actual parent provider request.
    tokio::time::timeout(Duration::from_secs(/*secs*/ 15), async {
        loop {
            let delivered = provider
                .requests
                .lock()
                .expect("requests")
                .iter()
                .any(|request| {
                    let text = serde_json::to_string(request).expect("request JSON");
                    text.contains(
                        "Queued child turn could not start: child provider is not configured",
                    ) && text.contains(&format!(
                        "agent:{}:queued:{invalid_id}",
                        child.child_session_id
                    ))
                });
            if delivered {
                break;
            }
            tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;
        }
    })
    .await
    .with_context(|| {
        format!(
            "parent did not receive queued child failure: {:?}",
            provider
                .requests
                .lock()
                .unwrap()
                .iter()
                .map(|r| serde_json::to_value(r).unwrap()["messages"].clone())
                .collect::<Vec<_>>()
        )
    })?;
    let requests = provider.requests.lock().expect("requests");
    assert!(requests.iter().any(|request| {
        let text = serde_json::to_string(request).expect("request JSON");
        text.contains("valid queued input") && !text.contains("invalid queued input")
    }));
    Ok(())
}
