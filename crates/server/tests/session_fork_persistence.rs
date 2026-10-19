use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::SessionId;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;

use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;

struct SingleReplyProvider;

#[async_trait]
impl ModelProviderSDK for SingleReplyProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(model_response("Generated title"))
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Fork persistence reply.".to_string(),
            }),
            Ok(StreamEvent::MessageDone {
                response: model_response("Fork persistence reply."),
            }),
        ])))
    }

    fn name(&self) -> &str {
        "single-reply-test-provider"
    }
}

#[tokio::test]
async fn session_fork_reports_fork_from_id_and_replays_self_contained_history() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let source = start_session(&runtime, connection_id, data_root.path()).await?;
    start_and_complete_turn(&runtime, connection_id, &mut notifications_rx, source).await?;

    let fork_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 4,
                "method": "session/fork",
                "params": {
                    "sessionId": source
                }
            }),
        )
        .await
        .context("session/fork response")?;
    let fork = serde_json::from_value::<
        devo_server::SuccessResponse<devo_protocol::native::rpc_session::SessionForkResult>,
    >(fork_response)?
    .result;
    let fork_title_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 6,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": fork.session.id,
                    "expectedVersion": 0,
                    "title": "Forked session"
                }
            }),
        )
        .await
        .context("fork title update response")?;
    let fork_session = serde_json::from_value::<
        devo_server::SuccessResponse<
            devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
        >,
    >(fork_title_response)?
    .result
    .session;
    let fork_session_id = SessionId::from(fork_session.id.as_str());

    assert_eq!(fork_session.parent, None);
    assert_eq!(
        fork_session.fork_from_id.as_ref().map(ToString::to_string),
        Some(source.to_string())
    );
    assert_eq!(fork_session.title.as_deref(), Some("Forked session"));

    let source_items = list_session_items(&runtime, connection_id, source).await?;
    let fork_items = list_session_items(&runtime, connection_id, fork_session_id).await?;
    assert_eq!(
        item_payloads_for_compare(&fork_items),
        item_payloads_for_compare(&source_items)
    );

    let rebuilt_runtime = build_runtime(data_root.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    let sessions = list_sessions(&rebuilt_runtime, rebuilt_connection_id).await?;
    let replayed_fork = sessions
        .iter()
        .find(|session| session.id.as_str() == fork_session_id.to_string())
        .context("replayed fork session")?;
    assert_eq!(
        replayed_fork.fork_from_id.as_ref().map(ToString::to_string),
        Some(source.to_string())
    );
    assert!(replayed_fork.parent.is_none());

    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 5,
                "method": "session/resume",
                "params": {
                "sessionId": fork_session.id
                }
            }),
        )
        .await
        .context("session/resume forked child")?;
    assert!(
        resume_response.get("result").is_some(),
        "forked child must resume after restart: {resume_response}"
    );
    let resumed_items =
        list_session_items(&rebuilt_runtime, rebuilt_connection_id, fork_session_id).await?;
    assert_eq!(
        item_payloads_for_compare(&resumed_items),
        item_payloads_for_compare(&source_items)
    );

    Ok(())
}

#[tokio::test]
async fn failed_session_fork_metadata_persistence_does_not_register_fork() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let source = start_session(&runtime, connection_id, data_root.path()).await?;
    start_and_complete_turn(&runtime, connection_id, &mut notifications_rx, source).await?;
    let sessions_before = list_sessions(&runtime, connection_id).await?;
    assert_eq!(sessions_before.len(), 1);

    let sessions_root = data_root.path().join("sessions");
    std::fs::remove_dir_all(&sessions_root)?;
    std::fs::write(&sessions_root, "not a directory")?;

    let fork_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 4,
                "method": "session/fork",
                "params": {
                    "sessionId": source
                }
            }),
        )
        .await
        .context("failed session/fork response")?;

    assert_eq!(
        fork_response["error"]["code"],
        serde_json::json!("InternalError")
    );
    assert!(
        fork_response["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("failed to persist forked session metadata")
    );
    let sessions_after = list_sessions(&runtime, connection_id).await?;
    assert!(
        sessions_after
            .iter()
            .all(|session| session.id.as_str() == source.to_string()),
        "a failed fork must not register a new session"
    );

    Ok(())
}

fn model_response(text: &str) -> ModelResponse {
    ModelResponse {
        id: "response-1".to_string(),
        content: vec![ResponseContent::Text(text.to_string())],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage::default(),
        metadata: ResponseMetadata::default(),
    }
}

/// Same replies as [`SingleReplyProvider`] but records every model request,
/// so a test can assert on the prompt the fork actually ran.
struct CapturingForkProvider {
    requests: std::sync::Mutex<Vec<ModelRequest>>,
}

#[async_trait]
impl ModelProviderSDK for CapturingForkProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(model_response("Generated title"))
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().expect("lock requests").push(request);
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Fork persistence reply.".to_string(),
            }),
            Ok(StreamEvent::MessageDone {
                response: model_response("Fork persistence reply."),
            }),
        ])))
    }

    fn name(&self) -> &str {
        "capturing-fork-test-provider"
    }
}

/// A fork cut that keeps the compaction turn must keep the compacted prompt:
/// the fork's first model turn runs summary + preserved suffix, not the full
/// linear history of the source session.
#[tokio::test]
async fn fork_of_compacted_session_keeps_compacted_prompt() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(CapturingForkProvider {
        requests: std::sync::Mutex::new(Vec::new()),
    });
    let runtime = devo_server::test_support::TestRuntime::new(provider.clone())
        .with_named_model("test-model", "Test Model")
        .db_file("session_fork_compaction.db")
        .runtime(data_root.path());
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let source = start_session(&runtime, connection_id, data_root.path()).await?;

    // Distinct payloads: the snapshot preserves the last complete turn
    // verbatim, so only the first turn's "a" run is guaranteed compacted away.
    for (request_id, letter) in [(3_u64, 'a'), (4, 'b')] {
        let response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": request_id,
                    "method": "turn/start",
                    "params": {
                        "sessionId": source,
                        "input": [{ "type": "text", "text": letter.to_string().repeat(30_000) }],
                        "idempotencyKey": format!("fork-compaction-seed-{letter}")
                    }
                }),
            )
            .await
            .context("turn/start response")?;
        assert!(
            serde_json::from_value::<
                devo_server::SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult>,
            >(response)
            .is_ok(),
            "turn/start must succeed"
        );
        wait_for_turn_completed(&mut notifications_rx).await?;
    }

    let compact_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 5,
                "method": "session/compact/start",
                "params": { "sessionId": source }
            }),
        )
        .await
        .context("session/compact/start response")?;
    assert!(
        compact_response.get("result").is_some(),
        "compaction must start: {compact_response}"
    );
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") == Some(&serde_json::json!("context/compactionCompleted"))
                || has_original_method(&value, "context/compactionCompleted")
            {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before compaction completed")
    })
    .await
    .context("timed out waiting for context/compactionCompleted")??;
    // The compaction turn finalizes after compactionCompleted; wait for its
    // terminal turn/completed before forking.
    wait_for_turn_completed(&mut notifications_rx).await?;

    let fork_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 6,
                "method": "session/fork",
                "params": { "sessionId": source }
            }),
        )
        .await
        .context("session/fork response")?;
    let fork = serde_json::from_value::<
        devo_server::SuccessResponse<devo_protocol::native::rpc_session::SessionForkResult>,
    >(fork_response)?
    .result;
    let fork_session_id = SessionId::from(fork.session.id.as_str());

    let resume_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 7,
                "method": "session/resume",
                "params": { "sessionId": fork.session.id }
            }),
        )
        .await
        .context("session/resume forked child")?;
    assert!(
        resume_response.get("result").is_some(),
        "forked session must resume: {resume_response}"
    );

    let turn_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 8,
                "method": "turn/start",
                "params": {
                    "sessionId": fork_session_id,
                    "input": [{ "type": "text", "text": "hello from the fork" }],
                    "idempotencyKey": "fork-compaction-probe"
                }
            }),
        )
        .await
        .context("turn/start on fork response")?;
    assert!(
        serde_json::from_value::<
            devo_server::SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult>,
        >(turn_response)
        .is_ok(),
        "turn/start on the fork must succeed"
    );
    wait_for_turn_completed(&mut notifications_rx).await?;

    let requests = provider.requests.lock().expect("lock requests");
    let last = serde_json::to_string(requests.last().context("captured fork request")?)?;
    assert!(
        last.contains("<compaction_summary>"),
        "a fork that keeps the compaction turn must run the compacted prompt"
    );
    assert!(
        !last.contains(&"a".repeat(2_000)),
        "fork reverted the compacted session to the full linear history"
    );
    Ok(())
}

fn build_runtime(data_root: &Path) -> Result<Arc<ServerRuntime>> {
    Ok(
        devo_server::test_support::TestRuntime::new(Arc::new(SingleReplyProvider))
            .with_named_model("test-model", "Test Model")
            .db_file("session_fork_persistence.db")
            .runtime(data_root),
    )
}

async fn initialize_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(128);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let initialize_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "_meta": { "devo": { "protocol": "native" } },
                    "clientInfo": {
                        "name": "session-fork-persistence-test",
                        "title": "session-fork-persistence-test",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    let response: serde_json::Value = initialize_response;
    assert_eq!(
        response["result"]["agentInfo"]["name"],
        serde_json::json!("devo-server")
    );
    Ok((connection_id, notifications_rx))
}

async fn start_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &Path,
) -> Result<SessionId> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": cwd,
                    "idempotencyKey": "session-fork-source"
                }
            }),
        )
        .await
        .context("session/new response")?;
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionNewResult,
    > = serde_json::from_value(response)?;
    Ok(SessionId::from(response.result.session.id.as_str()))
}

async fn start_and_complete_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    session_id: SessionId,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "seed fork history" }],
                    "idempotencyKey": "fork-seed-turn"
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult> =
        serde_json::from_value(response)?;
    wait_for_turn_completed(notifications_rx).await
}

async fn wait_for_turn_completed(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") == Some(&serde_json::json!("turn/completed"))
                || has_original_method(&value, "turn/completed")
            {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before turn/completed")
    })
    .await
    .context("timed out waiting for turn/completed")??;
    Ok(())
}

fn has_original_method(value: &serde_json::Value, method: &str) -> bool {
    value.get("method") == Some(&serde_json::json!("session/update"))
        && value["params"]["_meta"]["devo/originalMethod"].as_str() == Some(method)
}

async fn list_sessions(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
) -> Result<Vec<devo_protocol::native::session::Session>> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 5,
                "method": "session/list",
                "params": {}
            }),
        )
        .await
        .context("session/list response")?;
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionListResult,
    > = serde_json::from_value(response)?;
    Ok(response.result.data)
}

async fn list_session_items(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
) -> Result<Vec<devo_protocol::native::item::ItemEnvelope>> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 50,
                "method": "session/items/list",
                "params": { "sessionId": session_id }
            }),
        )
        .await
        .context("session/items/list response")?;
    let page: devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope> =
        serde_json::from_value(response["result"].clone())?;
    Ok(page.data)
}

fn item_payloads_for_compare(
    items: &[devo_protocol::native::item::ItemEnvelope],
) -> Vec<(String, String, serde_json::Value)> {
    items
        .iter()
        .map(|item| {
            (
                item.turn_id.as_str().to_string(),
                item.id.as_str().to_string(),
                serde_json::to_value(&item.item).expect("serialize item payload"),
            )
        })
        .collect()
}
