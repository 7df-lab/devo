use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::native::ids::SessionId;
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
        "blocking-native-delete-provider"
    }
}

async fn request(
    runtime: &Arc<ServerRuntime>,
    connection: u64,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let response = Box::pin(runtime.handle_incoming(
        connection,
        json!({
            "id": id, "method": method, "params": params,
        }),
    ))
    .await
    .context("missing RPC response")?;
    anyhow::ensure!(response.get("error").is_none(), "{method}: {response}");
    Ok(response["result"].clone())
}

#[cfg(unix)]
#[tokio::test]
async fn deletion_stops_idle_session_commands_owned_by_another_connection() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let runtime = TestRuntime::new(Arc::new(devo_server::test_support::NoopProvider::default()))
        .with_test_model()
        .runtime(directory.path());
    let mut connections = Vec::new();
    let mut notifications = Vec::new();
    for index in 0..2 {
        let (outbound, incoming) = devo_server::test_outbound_channel(4096);
        let connection = runtime
            .register_connection(ClientTransportKind::Stdio, outbound)
            .await;
        request(
            &runtime,
            connection,
            index + 1,
            "initialize",
            json!({
                "protocolVersion": 1, "clientCapabilities": {},
                "_meta": { "devo": { "protocol": "native" } },
            }),
        )
        .await?;
        connections.push(connection);
        notifications.push(incoming);
    }
    let session = request(
        &runtime,
        connections[0],
        3,
        "session/new",
        json!({
            "cwd": directory.path(), "idempotencyKey": "idle-session",
        }),
    )
    .await?["session"]["id"]
        .clone();
    request(
        &runtime,
        connections[0],
        4,
        "command/exec",
        json!({
            "session_id": session, "process_id": "delete-idle-command",
            "cwd": directory.path(), "size": null,
            "program": { "type": "one_shot", "command": "sleep 300" },
        }),
    )
    .await?;
    request(
        &runtime,
        connections[1],
        5,
        "session/delete",
        json!({
            "sessionId": session,
        }),
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = notifications[0].recv().await {
            if event["method"] == "command/exec/exited" {
                assert_eq!(event["params"]["processId"], json!("delete-idle-command"));
                return Ok::<_, anyhow::Error>(());
            }
        }
        anyhow::bail!("command exit notification missing")
    })
    .await
    .context("deleted session command must stop promptly")??;
    Ok(())
}

#[tokio::test]
async fn deleting_concurrent_native_sessions_discards_queues_and_preserves_other_folders()
-> Result<()> {
    let directory = tempfile::tempdir()?;
    let project = directory.path().join("remove-project");
    let retained_project = directory.path().join("keep-project");
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(&retained_project)?;
    let marker = project.join("keep.txt");
    std::fs::write(&marker, "folder removal keeps workspace files")?;
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let runtime = TestRuntime::new(Arc::new(BlockingProvider(started_tx)))
        .with_test_model()
        .runtime(directory.path());
    let (outbound, mut notifications) = devo_server::test_outbound_channel(4096);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    request(
        &runtime,
        connection,
        1,
        "initialize",
        json!({
            "protocolVersion": 1, "clientCapabilities": {},
            "_meta": { "devo": { "protocol": "native" } },
        }),
    )
    .await?;
    let retained = request(
        &runtime,
        connection,
        2,
        "session/new",
        json!({
            "cwd": retained_project, "idempotencyKey": "keep-project",
        }),
    )
    .await?["session"]["id"]
        .clone();
    let mut sessions = Vec::new();
    for index in 0..12 {
        let id = 10 + index * 10;
        let session = request(
            &runtime,
            connection,
            id,
            "session/new",
            json!({
                "cwd": project, "idempotencyKey": format!("new-{index}"),
            }),
        )
        .await?;
        let session_id: SessionId = serde_json::from_value(session["session"]["id"].clone())?;
        request(
            &runtime,
            connection,
            id + 4,
            "subscription/create",
            json!({
                "selectors": [{ "kind": "session", "sessionId": session_id }],
                "includeSnapshot": false,
            }),
        )
        .await?;
        let turn = request(
            &runtime,
            connection,
            id + 1,
            "turn/start",
            json!({
                "sessionId": session_id, "idempotencyKey": format!("turn-{index}"),
                "input": [{ "type": "text", "text": "Keep working until interrupted" }],
            }),
        )
        .await?;
        tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
            .await?
            .context("provider did not start")?;
        request(
            &runtime,
            connection,
            id + 2,
            "session/queue/push",
            json!({
                "sessionId": session_id, "idempotencyKey": format!("queue-{index}"),
                "input": [{ "type": "text", "text": "This queued work must never start" }],
            }),
        )
        .await?;
        request(
            &runtime,
            connection,
            id + 3,
            "turn/steer",
            json!({
                "sessionId": session_id, "expectedTurnId": turn["turn"]["id"],
                "idempotencyKey": format!("steer-{index}"),
                "input": [{ "type": "text", "text": "Discard this steering input on deletion" }],
            }),
        )
        .await?;
        sessions.push(session_id);
    }
    let results = tokio::time::timeout(
        Duration::from_secs(3),
        futures::future::join_all(sessions.iter().enumerate().map(|(index, session_id)| {
            request(
                &runtime,
                connection,
                200 + index as u64,
                "session/delete",
                json!({
                    "sessionId": session_id,
                }),
            )
        })),
    )
    .await
    .context("concurrent folder deletion was too slow")?;
    assert_eq!(
        results.into_iter().collect::<Result<Vec<_>>>()?,
        vec![json!({}); 12]
    );
    let mut deleted_ids = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), async {
        while deleted_ids.len() < sessions.len() {
            let event = notifications
                .recv()
                .await
                .context("notification channel closed")?;
            if event["method"] == "session/deleted" {
                let id: SessionId = serde_json::from_value(event["params"]["sessionId"].clone())?;
                assert_eq!(event["params"]["deletedSessionIds"], json!([id]));
                deleted_ids.push(id);
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("all deleted sessions must be broadcast")??;
    deleted_ids.sort();
    let mut expected_ids = sessions.clone();
    expected_ids.sort();
    assert_eq!(deleted_ids, expected_ids);
    let listed = request(&runtime, connection, 300, "session/list", json!({})).await?;
    let listed_ids: Vec<_> = listed["data"]
        .as_array()
        .context("session list")?
        .iter()
        .map(|session| session["id"].clone())
        .collect();
    assert_eq!(listed_ids, vec![retained]);
    assert_eq!(
        std::fs::read_to_string(marker)?,
        "folder removal keeps workspace files"
    );
    let db = devo_server::db::Database::open(directory.path().join("test.db"))?;
    let queues = sessions
        .iter()
        .map(|session_id| {
            Ok((
                db.get_session(session_id)?,
                serde_json::to_value(
                    db.list_pending(session_id, devo_server::db::QueueType::Turn)?,
                )?,
                serde_json::to_value(
                    db.list_pending(session_id, devo_server::db::QueueType::Steer)?,
                )?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(queues, vec![(None, json!([]), json!([])); 12]);
    assert!(
        started_rx.try_recv().is_err(),
        "queued turns must not restart after deletion"
    );
    Ok(())
}
