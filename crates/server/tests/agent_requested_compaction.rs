use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::{ModelRequest, ModelResponse, StreamEvent};
use devo_provider::ModelProviderSDK;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::Semaphore;
use tokio::time::timeout;

#[path = "support/subagent_lifecycle.rs"]
mod support;

/// Hold the parent response after compact.run so a follow-up is certainly queued
/// before its turn ends. Semaphore permits avoid timing-dependent sleeps.
struct QueuedCompactProvider {
    scripted: support::ScriptedProvider,
    reached_boundary: Semaphore,
    finish_turn: Semaphore,
}

#[async_trait]
impl ModelProviderSDK for QueuedCompactProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.scripted.completion(request).await
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        if self.scripted.stream_calls() == 1 {
            self.reached_boundary.add_permits(1);
            self.finish_turn.acquire().await?.forget();
        }
        self.scripted.completion_stream(request).await
    }

    fn name(&self) -> &str {
        "queued-agent-compaction"
    }
}

#[tokio::test]
async fn agent_requested_compaction_survives_a_queued_followup() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(QueuedCompactProvider {
        scripted: support::ScriptedProvider::new([
            support::ScriptedProvider::ipython_code_tool_call(
                "import compact\nprint(await compact.run('retain the queued follow-up'))",
            ),
            support::ScriptedProvider::completed("compaction scheduled"),
            support::ScriptedProvider::completed("queued follow-up finished"),
        ]),
        reached_boundary: Semaphore::new(0),
        finish_turn: Semaphore::new(0),
    });
    let runtime = support::build_runtime(data_root.path(), Arc::clone(&provider) as _)?;
    let (connection_id, mut notifications) = support::initialize_connection(&runtime).await?;
    let session_id =
        support::start_parent_session(&runtime, connection_id, data_root.path()).await?;
    support::start_turn_with_approval_policy(
        &runtime,
        connection_id,
        session_id,
        "schedule compaction before a queued follow-up",
        Some("never"),
    )
    .await?;
    timeout(Duration::from_secs(30), provider.reached_boundary.acquire())
        .await
        .context("parent did not reach the turn boundary")??
        .forget();

    let queued = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 20,
                "method": "session/queue/push",
                "params": {
                    "sessionId": session_id,
                    "input": [{"type": "text", "text": "queued follow-up"}],
                    "idempotencyKey": "queued-agent-compaction"
                }
            }),
        )
        .await
        .context("queue response")?;
    assert_eq!(queued.get("error"), None, "queue push: {queued}");
    provider.finish_turn.add_permits(1);

    support::wait_for_session_notification(
        &mut notifications,
        "context/compactionCompleted",
        session_id,
    )
    .await?;
    assert_eq!(provider.scripted.stream_calls(), 3);
    let rollout = std::fs::read_to_string(
        data_root
            .path()
            .join("sessions")
            .join(format!("{session_id}.jsonl")),
    )?;
    let records = rollout
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        records
            .iter()
            .filter(|record| record["kind"] == "compactionSnapshot")
            .count(),
        1,
        "compaction must replace history and persist exactly one snapshot"
    );
    Ok(())
}
