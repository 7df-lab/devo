use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::{ModelRequest, ModelResponse, StreamEvent};
use devo_provider::ModelProviderSDK;
use devo_server::test_support::TestRuntime;
use devo_server::{ClientTransportKind, ServerRuntime};
use futures::Stream;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::sync::mpsc;

struct BlockingProvider(mpsc::UnboundedSender<()>);

#[async_trait]
impl ModelProviderSDK for BlockingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        let _ = self.0.send(());
        std::future::pending().await
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let _ = self.0.send(());
        std::future::pending().await
    }

    fn name(&self) -> &str {
        "blocking-subscription-provider"
    }
}

async fn request(
    runtime: &Arc<ServerRuntime>,
    connection: u64,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let response = tokio::time::timeout(
        Duration::from_secs(8),
        Box::pin(runtime.handle_incoming(
            connection,
            json!({
                "id": id, "method": method, "params": params,
            }),
        )),
    )
    .await
    .with_context(|| format!("{method} timed out"))?
    .context("missing RPC response")?;
    anyhow::ensure!(response.get("error").is_none(), "{method}: {response}");
    Ok(response["result"].clone())
}

#[tokio::test]
async fn snapshot_only_skips_replay_and_folder_snapshots_do_not_parse_transcripts() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let runtime = TestRuntime::new(Arc::new(devo_server::test_support::NoopProvider::default()))
        .with_test_model()
        .runtime(directory.path());
    let (outbound, mut notifications) = devo_server::test_outbound_channel(4096);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    request(&runtime, connection, 1, "initialize", json!({
        "protocolVersion": 1, "clientCapabilities": {}, "_meta": { "devo": { "protocol": "native" } },
    })).await?;
    let mut ids = Vec::new();
    for index in 0..20 {
        let session = request(
            &runtime,
            connection,
            2 + index,
            "session/new",
            json!({
                "cwd": directory.path(), "idempotencyKey": format!("roster-{index}"),
            }),
        )
        .await?;
        ids.push(session["session"]["id"].clone());
    }
    let selected = ids[0].clone();
    request(
        &runtime,
        connection,
        30,
        "session/metadata/update",
        json!({
            "sessionId": selected, "expectedVersion": 0, "title": "Indexed title",
        }),
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = notifications.recv().await {
            if event["method"] == "session/metadataUpdated" {
                return Ok::<_, anyhow::Error>(());
            }
        }
        anyhow::bail!("metadata event missing")
    })
    .await??;
    let selector = json!([{ "kind": "session", "sessionId": selected }]);
    let replay = request(
        &runtime,
        connection,
        31,
        "subscription/create",
        json!({
            "selectors": selector, "includeSnapshot": true, "after": [],
        }),
    )
    .await?;
    anyhow::ensure!(
        !replay["replay"]
            .as_array()
            .context("replay array")?
            .is_empty(),
        "default replay must be preserved"
    );
    let snapshot = request(
        &runtime,
        connection,
        32,
        "subscription/create",
        json!({
            "selectors": selector, "includeSnapshot": true, "replay": "snapshotOnly", "after": [],
        }),
    )
    .await?;
    assert_eq!(
        snapshot.get("replay").cloned().unwrap_or_else(|| json!([])),
        json!([])
    );
    assert_eq!(snapshot["cursors"], replay["cursors"]);
    assert_eq!(snapshot["snapshots"], replay["snapshots"]);
    let invalid = runtime
        .handle_incoming(
            connection,
            json!({
                "id": 33, "method": "subscription/create", "params": {
                    "selectors": selector, "includeSnapshot": false, "replay": "snapshotOnly",
                },
            }),
        )
        .await
        .context("invalid response")?;
    assert_eq!(
        invalid["error"]["message"],
        json!("snapshotOnly replay requires includeSnapshot")
    );
    // A damaged unrelated transcript must not block an indexed folder roster.
    let damaged = directory
        .path()
        .join("sessions")
        .join(format!("{}.jsonl", ids[19].as_str().context("session id")?));
    anyhow::ensure!(
        damaged.exists(),
        "fixture rollout must exist: {}",
        damaged.display()
    );
    std::fs::write(damaged, "broken rollout for metadata-only test\n")?;
    let roster = request(
        &runtime,
        connection,
        34,
        "subscription/create",
        json!({
            "selectors": [{ "kind": "sessionsByCwd", "cwd": directory.path() }],
            "includeSnapshot": true, "replay": "snapshotOnly",
        }),
    )
    .await?;
    let sessions = roster["snapshots"][0]["data"]["sessions"]
        .as_array()
        .context("roster")?;
    let mut actual: Vec<_> = sessions
        .iter()
        .map(|session| session["id"].clone())
        .collect();
    actual.sort_by_key(Value::to_string);
    ids.sort_by_key(Value::to_string);
    assert_eq!(actual, ids);
    assert_eq!(
        roster.get("replay").cloned().unwrap_or_else(|| json!([])),
        json!([])
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn snapshot_bootstrap_keeps_mid_turn_reads_queue_steer_interrupt_and_resume_responsive()
-> Result<()> {
    let directory = tempfile::tempdir()?;
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let runtime = TestRuntime::new(Arc::new(BlockingProvider(started_tx)))
        .with_test_model()
        .runtime(directory.path());
    let (outbound, mut notifications) = devo_server::test_outbound_channel(4096);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    request(&runtime, connection, 1, "initialize", json!({
        "protocolVersion": 1, "clientCapabilities": {}, "_meta": { "devo": { "protocol": "native" } },
    })).await?;
    request(
        &runtime,
        connection,
        2,
        "subscription/create",
        json!({
            "selectors": [{ "kind": "sessionsByCwd", "cwd": directory.path() }],
            "includeSnapshot": true, "replay": "snapshotOnly",
        }),
    )
    .await?;
    let session = request(
        &runtime,
        connection,
        3,
        "session/new",
        json!({
            "cwd": directory.path(), "idempotencyKey": "working-roster",
        }),
    )
    .await?["session"]["id"]
        .clone();
    let turn = request(
        &runtime,
        connection,
        4,
        "turn/start",
        json!({
            "sessionId": session, "idempotencyKey": "working-turn",
            "input": [{ "type": "text", "text": "Wait for interruption" }],
        }),
    )
    .await?["turn"]["id"]
        .clone();
    tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
        .await?
        .context("provider starts")?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = notifications.recv().await {
            if event["method"] == "session/statusChanged" && event["params"]["status"] == "active" {
                assert_eq!(event["params"]["sessionId"], session);
                return Ok::<_, anyhow::Error>(());
            }
        }
        anyhow::bail!("folder selector must receive live active status")
    })
    .await??;
    tokio::time::timeout(Duration::from_secs(5), async {
        let snapshot = request(
            &runtime,
            connection,
            5,
            "subscription/create",
            json!({
                "selectors": [{ "kind": "session", "sessionId": session }],
                "includeSnapshot": true, "replay": "snapshotOnly",
            }),
        )
        .await?;
        assert_eq!(
            snapshot.get("replay").cloned().unwrap_or_else(|| json!([])),
            json!([])
        );
        assert_eq!(snapshot["snapshots"][0]["data"]["activeTurn"]["id"], turn);
        let reads = futures::future::join_all([
            request(&runtime, connection, 6, "runtime/ping", json!({})),
            request(
                &runtime,
                connection,
                7,
                "session/list",
                json!({ "cwds": [directory.path()], "limit": 5 }),
            ),
            request(
                &runtime,
                connection,
                8,
                "session/items/list",
                json!({ "sessionId": session, "cursor": "tail", "limit": 2 }),
            ),
            request(
                &runtime,
                connection,
                9,
                "workspace/changes/read",
                json!({ "sessionId": session, "scopes": ["uncommitted"] }),
            ),
        ])
        .await;
        for read in reads {
            read?;
        }
        request(
            &runtime,
            connection,
            10,
            "session/queue/push",
            json!({
                "sessionId": session, "idempotencyKey": "queued-work",
                "input": [{ "type": "text", "text": "Queued follow-up" }],
            }),
        )
        .await?;
        request(
            &runtime,
            connection,
            11,
            "turn/steer",
            json!({
                "sessionId": session, "expectedTurnId": turn, "idempotencyKey": "steer-work",
                "input": [{ "type": "text", "text": "Steering follow-up" }],
            }),
        )
        .await?;
        let queue = request(
            &runtime,
            connection,
            12,
            "session/queue/list",
            json!({ "sessionId": session }),
        )
        .await?;
        anyhow::ensure!(
            !queue["entries"]
                .as_array()
                .context("queue entries")?
                .is_empty(),
            "queue persists mid-turn"
        );
        request(
            &runtime,
            connection,
            13,
            "session/interrupt",
            json!({ "scope": { "scope": "session", "sessionId": session } }),
        )
        .await?;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("snapshot/bootstrap must not deadlock the working session")??;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = notifications.recv().await {
            if event["method"] == "turn/completed" {
                return Ok::<_, anyhow::Error>(());
            }
        }
        anyhow::bail!("interrupt completion missing")
    })
    .await??;
    request(
        &runtime,
        connection,
        14,
        "session/resume",
        json!({ "sessionId": session }),
    )
    .await?;
    request(
        &runtime,
        connection,
        15,
        "session/delete",
        json!({ "sessionId": session }),
    )
    .await?;
    Ok(())
}
