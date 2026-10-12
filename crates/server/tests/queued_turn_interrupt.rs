use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::{ModelRequest, ModelResponse, StreamEvent};
use devo_provider::ModelProviderSDK;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Semaphore;
use tokio::time::timeout;

#[path = "support/subagent_lifecycle.rs"]
mod support;

#[derive(Debug, Clone, Copy)]
enum QueuedStart {
    AfterFirstTurn,
    IdlePush,
}

/// Hold the first response until a follow-up is queued, then keep the queued
/// response pending so interruption must cancel actual in-flight work.
struct GatedQueuedProvider {
    scripted: support::ScriptedProvider,
    requests: AtomicUsize,
    first_started: Semaphore,
    finish_first: Semaphore,
    queued_started: Semaphore,
    queued_start: QueuedStart,
}

#[async_trait]
impl ModelProviderSDK for GatedQueuedProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.scripted.completion(request).await
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        match (
            self.queued_start,
            self.requests.fetch_add(1, Ordering::SeqCst),
        ) {
            (QueuedStart::AfterFirstTurn, 0) => {
                self.first_started.add_permits(1);
                self.finish_first.acquire().await?.forget();
                self.scripted.completion_stream(request).await
            }
            (QueuedStart::AfterFirstTurn, 1) | (QueuedStart::IdlePush, 0) => {
                self.queued_started.add_permits(1);
                Ok(Box::pin(futures::stream::pending()))
            }
            request => anyhow::bail!("unexpected stream request {request:?}"),
        }
    }

    fn name(&self) -> &str {
        "gated-queued-interrupt"
    }
}

async fn assert_queued_turn_interrupt(queued_start: QueuedStart) -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(GatedQueuedProvider {
        scripted: support::ScriptedProvider::new([support::ScriptedProvider::completed(
            "first turn finished",
        )]),
        requests: AtomicUsize::new(0),
        first_started: Semaphore::new(0),
        finish_first: Semaphore::new(0),
        queued_started: Semaphore::new(0),
        queued_start,
    });
    let runtime = support::build_runtime(data_root.path(), Arc::clone(&provider) as _)?;
    let (connection_id, mut notifications) = support::initialize_connection(&runtime).await?;
    let session_id =
        support::start_parent_session(&runtime, connection_id, data_root.path()).await?;
    let subscribed = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 3,
                "method": "subscription/create",
                "params": {
                    "selectors": [{"kind": "session", "sessionId": session_id}],
                    "includeSnapshot": false
                }
            }),
        )
        .await
        .context("subscription response")?;
    assert_eq!(subscribed.get("error"), None, "subscribe: {subscribed}");
    if matches!(queued_start, QueuedStart::AfterFirstTurn) {
        support::start_turn_with_approval_policy(
            &runtime,
            connection_id,
            session_id,
            "first gated turn",
            Some("never"),
        )
        .await?;
        timeout(Duration::from_secs(15), provider.first_started.acquire())
            .await
            .context("first turn did not start")??
            .forget();
    } else {
        let permissions = runtime
            .handle_incoming(
                connection_id,
                json!({
                    "id": 8, "method": "session/metadata/update",
                    "params": {"sessionId": session_id, "expectedVersion": 0,
                        "settings": {"permissionProfile": "fullAccess"}}
                }),
            )
            .await
            .context("idle session permissions")?;
        assert_eq!(permissions.get("error"), None, "permissions: {permissions}");
    }
    let queued = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 20,
                "method": "session/queue/push",
                "params": {
                    "sessionId": session_id,
                    "input": [{"type": "text", "text": "queued turn to interrupt"}],
                    "idempotencyKey": "queued-turn-interrupt"
                }
            }),
        )
        .await
        .context("queue response")?;
    assert_eq!(queued.get("error"), None, "queue push: {queued}");
    if matches!(queued_start, QueuedStart::AfterFirstTurn) {
        provider.finish_first.add_permits(1);
    }
    timeout(Duration::from_secs(15), provider.queued_started.acquire())
        .await
        .context("queued turn did not start")??
        .forget();
    let queued_turn_id = timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications.recv().await {
            match queued_start {
                QueuedStart::AfterFirstTurn
                    if value["method"] == "queue/updated"
                        && value["params"]["change"] == "drained" =>
                {
                    return Ok(value["params"]["startedTurnId"].clone());
                }
                QueuedStart::IdlePush if value["method"] == "turn/started" => {
                    return Ok(value["params"]["turn"]["id"].clone());
                }
                QueuedStart::AfterFirstTurn | QueuedStart::IdlePush => {}
            }
        }
        anyhow::bail!("no queue drain notification")
    })
    .await
    .context("queued turn did not publish its drain notification")??;
    assert!(
        queued_turn_id.is_string(),
        "drain must identify the queued turn"
    );

    let read = timeout(
        Duration::from_secs(2),
        runtime.handle_incoming(
            connection_id,
            json!({
                "id": 19, "method": "session/read", "params": {"sessionId": session_id}
            }),
        ),
    )
    .await
    .context("queued turn blocked a session read")?
    .context("session read response")?;
    assert_eq!(read.get("error"), None, "session read: {read}");

    let interrupted = runtime
        .handle_incoming(
            connection_id,
            json!({
                "id": 21,
                "method": "session/interrupt",
                "params": {"scope": {"scope": "session", "sessionId": session_id}}
            }),
        )
        .await
        .context("interrupt response")?;
    assert_eq!(interrupted.get("error"), None, "interrupt: {interrupted}");
    assert_eq!(interrupted["result"], json!({"interrupted": true}));
    assert_eq!(
        json!({"id": read["result"]["session"]["id"],
            "status": read["result"]["session"]["status"]}),
        json!({"id": session_id, "status": "active"}),
    );
    let terminal = timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications.recv().await {
            if value["method"] == "turn/completed"
                && value["params"]["turn"]["id"] == queued_turn_id
            {
                return Ok(value["params"]["turn"].clone());
            }
        }
        anyhow::bail!("queued turn never terminalized")
    })
    .await
    .context("interrupted queued turn did not terminalize")??;
    assert_eq!(terminal["status"], json!("interrupted"));
    let expected_requests = match queued_start {
        QueuedStart::AfterFirstTurn => 2,
        QueuedStart::IdlePush => 1,
    };
    assert_eq!(provider.requests.load(Ordering::SeqCst), expected_requests);
    Ok(())
}

#[tokio::test]
async fn session_interrupt_cancels_a_drained_followup_turn() -> Result<()> {
    assert_queued_turn_interrupt(QueuedStart::AfterFirstTurn).await
}

#[tokio::test]
async fn session_interrupt_preserves_idle_queue_push_cancellation() -> Result<()> {
    assert_queued_turn_interrupt(QueuedStart::IdlePush).await
}
