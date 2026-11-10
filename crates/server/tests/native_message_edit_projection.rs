//! Message edits must project the same branch and envelopes live and after resume.
use std::{path::Path, pin::Pin, sync::Arc};

use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::native::{
    ids::{SessionId, TurnId},
    item::{Item, ItemEnvelope, ItemState},
    page::Page,
    rpc_session::SessionMessageEditResult,
    rpc_turn::TurnStartResult,
};
use devo_protocol::{ModelRequest, ModelResponse, ResponseContent, StreamEvent};
use devo_provider::{ModelProviderSDK, ProviderRoute, ProviderRouter, error::ProviderError};
use devo_server::{ClientTransportKind, ServerRuntime};
use futures::{Stream, StreamExt, stream};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, timeout};

type StreamCall = (ModelRequest, oneshot::Sender<()>);

struct ControlledRouter(mpsc::UnboundedSender<StreamCall>);

#[async_trait]
impl ProviderRouter for ControlledRouter {
    async fn stream(
        &self,
        _route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>, ProviderError> {
        let (finish, finished) = oneshot::channel();
        let _ = self.0.send((request, finish));
        Ok(Box::pin(
            stream::iter([Ok(StreamEvent::TextDelta {
                index: 0,
                text: "answer".into(),
            })])
            .chain(stream::once(async move {
                let _ = finished.await;
                Ok(StreamEvent::MessageDone {
                    response: response(),
                })
            })),
        ))
    }

    async fn complete(
        &self,
        _route: ProviderRoute,
        _request: ModelRequest,
    ) -> Result<ModelResponse, ProviderError> {
        Ok(response())
    }

    fn name(&self) -> &str {
        "edit-projection-router"
    }
}

struct UnusedProvider;

#[async_trait]
impl ModelProviderSDK for UnusedProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("unused provider")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        anyhow::bail!("unused provider")
    }

    fn name(&self) -> &str {
        "unused"
    }
}

fn response() -> ModelResponse {
    ModelResponse {
        id: "edit-answer".into(),
        content: vec![ResponseContent::Text("answer".into())],
        stop_reason: Some(devo_protocol::StopReason::EndTurn),
        usage: Default::default(),
        metadata: Default::default(),
    }
}

fn runtime(root: &Path, calls: mpsc::UnboundedSender<StreamCall>) -> Arc<ServerRuntime> {
    devo_server::test_support::TestRuntime::new(Arc::new(UnusedProvider))
        .router(Arc::new(ControlledRouter(calls)))
        .with_named_model("test-model", "Test Model")
        .db_file("edit-projection.db")
        .runtime(root)
}

async fn rpc(
    runtime: &Arc<ServerRuntime>,
    connection: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let reply = runtime
        .handle_incoming(
            connection,
            json!({ "id": 1, "method": method, "params": params }),
        )
        .await
        .context("RPC response")?;
    anyhow::ensure!(reply.get("error").is_none(), "{method}: {reply}");
    Ok(reply["result"].clone())
}

async fn connect(runtime: &Arc<ServerRuntime>) -> Result<(u64, mpsc::Receiver<Value>)> {
    let (tx, rx) = devo_server::test_outbound_channel(256);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, tx)
        .await;
    rpc(
        runtime,
        connection,
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "_meta": { "devo": { "protocol": "native" } },
            "clientInfo": { "name": "edit-projection-test", "version": "1" }
        }),
    )
    .await?;
    Ok((connection, rx))
}

async fn new_session(
    runtime: &Arc<ServerRuntime>,
    connection: u64,
    cwd: &Path,
) -> Result<SessionId> {
    let result = rpc(
        runtime,
        connection,
        "session/new",
        json!({
            "cwd": cwd, "idempotencyKey": uuid::Uuid::new_v4().to_string()
        }),
    )
    .await?;
    let id: SessionId = serde_json::from_value(result["session"]["id"].clone())?;
    rpc(runtime, connection, "session/metadata/update", json!({
        "sessionId": id, "expectedVersion": 0, "model": { "provider": "", "model": "test-model" }
    })).await?;
    Ok(id)
}

async fn items(
    runtime: &Arc<ServerRuntime>,
    connection: u64,
    session: SessionId,
) -> Result<Vec<ItemEnvelope>> {
    let page: Page<ItemEnvelope> = serde_json::from_value(
        rpc(
            runtime,
            connection,
            "session/items/list",
            json!({ "sessionId": session }),
        )
        .await?,
    )?;
    Ok(page.data)
}

async fn start(
    runtime: &Arc<ServerRuntime>,
    connection: u64,
    session: SessionId,
    text: &str,
) -> Result<TurnId> {
    let result: TurnStartResult = serde_json::from_value(
        rpc(
            runtime,
            connection,
            "turn/start",
            json!({
                "sessionId": session, "input": [{ "type": "text", "text": text }],
                "idempotencyKey": uuid::Uuid::new_v4().to_string()
            }),
        )
        .await?,
    )?;
    Ok(result.turn.id)
}

async fn edit(
    runtime: &Arc<ServerRuntime>,
    connection: u64,
    session: SessionId,
    text: &str,
) -> Result<SessionMessageEditResult> {
    let target = items(runtime, connection, session)
        .await?
        .into_iter()
        .rev()
        .find(|item| matches!(item.item, Item::UserMessage { .. }))
        .context("editable user")?;
    Ok(serde_json::from_value(
        rpc(
            runtime,
            connection,
            "session/message/edit",
            json!({
                "sessionId": session, "itemId": target.id, "expectedRevision": target.revision,
                "content": [{ "type": "text", "text": text }], "workspaceRestore": "skip",
                "idempotencyKey": uuid::Uuid::new_v4().to_string()
            }),
        )
        .await?,
    )?)
}

async fn finish(
    calls: &mut mpsc::UnboundedReceiver<StreamCall>,
    notifications: &mut mpsc::Receiver<Value>,
    turn: TurnId,
) -> Result<Vec<Value>> {
    let (_, done) = timeout(Duration::from_secs(10), calls.recv())
        .await?
        .context("provider call")?;
    let _ = done.send(());
    timeout(Duration::from_secs(10), async {
        let mut events = Vec::new();
        while let Some(event) = notifications.recv().await {
            let terminal = event["method"] == "turn/completed"
                && event["params"]["turn"]["id"] == turn.to_string();
            events.push(event);
            if terminal {
                return Ok(events);
            }
        }
        anyhow::bail!("notifications closed")
    })
    .await?
}

fn transcript(items: &[ItemEnvelope]) -> Vec<Value> {
    items
        .iter()
        .filter_map(|item| match &item.item {
            Item::UserMessage { content, .. } => Some(json!({ "user": content })),
            Item::AssistantMessage { text, .. } => Some(json!({ "assistant": text })),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn repeated_root_edits_keep_superseded_branches_out_of_history_and_cold_resume() -> Result<()>
{
    let root = TempDir::new()?;
    let (tx, mut calls) = mpsc::unbounded_channel();
    let server = runtime(root.path(), tx.clone());
    let (connection, mut notifications) = connect(&server).await?;
    let session = new_session(&server, connection, root.path()).await?;
    let turn = start(&server, connection, session, "original root").await?;
    finish(&mut calls, &mut notifications, turn).await?;
    for text in ["edited root", "edited root again"] {
        let edited = edit(&server, connection, session, text).await?;
        finish(
            &mut calls,
            &mut notifications,
            edited.replacement_turn_id.context("replacement")?,
        )
        .await?;
        let history = items(&server, connection, session).await?;
        assert_eq!(
            transcript(&history),
            vec![
                json!({ "user": [{ "type": "text", "text": text }] }),
                json!({ "assistant": "answer" }),
            ]
        );
        let user = history
            .iter()
            .find(|item| matches!(item.item, Item::UserMessage { .. }))
            .context("edited root")?;
        assert_eq!(user.parent_id, None);
    }
    let expected = items(&server, connection, session).await?;
    let restarted = runtime(root.path(), tx);
    let (connection, _notifications) = connect(&restarted).await?;
    rpc(
        &restarted,
        connection,
        "session/resume",
        json!({ "sessionId": session }),
    )
    .await?;
    assert_eq!(items(&restarted, connection, session).await?, expected);
    Ok(())
}

#[tokio::test]
async fn busy_edit_lifecycle_and_response_match_the_durable_envelope() -> Result<()> {
    let root = TempDir::new()?;
    let (tx, mut calls) = mpsc::unbounded_channel();
    let server = runtime(root.path(), tx);
    let (connection, mut notifications) = connect(&server).await?;
    let session = new_session(&server, connection, root.path()).await?;
    let first = start(&server, connection, session, "retained first").await?;
    finish(&mut calls, &mut notifications, first).await?;
    let before = items(&server, connection, session).await?;
    start(&server, connection, session, "busy original").await?;
    let (_, busy_finish) = timeout(Duration::from_secs(10), calls.recv())
        .await?
        .context("busy call")?;
    let target = items(&server, connection, session)
        .await?
        .into_iter()
        .rev()
        .find(|item| matches!(item.item, Item::UserMessage { .. }))
        .context("busy user")?;
    // Desktop skips revision preconditions; the replacement still increments
    // the target's durable revision, rather than treating zero as its revision.
    let result: SessionMessageEditResult = serde_json::from_value(
        rpc(
            &server,
            connection,
            "session/message/edit",
            json!({
                "sessionId": session, "itemId": target.id, "expectedRevision": 0,
                "content": [{ "type": "text", "text": "busy replacement" }],
                "workspaceRestore": "skip", "idempotencyKey": "busy-edit"
            }),
        )
        .await?,
    )?;
    drop(busy_finish);
    let events = finish(
        &mut calls,
        &mut notifications,
        result.replacement_turn_id.context("replacement")?,
    )
    .await?;
    let after = items(&server, connection, session).await?;
    let durable = after
        .iter()
        .find(|item| item.id == result.item.id)
        .context("durable replacement")?;
    assert_eq!(&result.item, durable);
    assert_eq!(durable.revision, target.revision + 1);
    assert!(durable.seq > before.last().context("retained item")?.seq);
    let lifecycle: Vec<ItemEnvelope> = events
        .iter()
        .filter(|event| {
            matches!(
                event["method"].as_str(),
                Some("item/started" | "item/completed")
            ) && event["params"]["item"]["id"] == result.item.id.to_string()
        })
        .map(|event| serde_json::from_value(event["params"]["item"].clone()))
        .collect::<Result<_, _>>()?;
    let mut started = durable.clone();
    started.state = ItemState::Running;
    assert_eq!(lifecycle, vec![started, durable.clone()]);
    assert_eq!(
        transcript(&after),
        vec![
            json!({ "user": [{ "type": "text", "text": "retained first" }] }),
            json!({ "assistant": "answer" }),
            json!({ "user": [{ "type": "text", "text": "busy replacement" }] }),
            json!({ "assistant": "answer" }),
        ]
    );
    Ok(())
}
