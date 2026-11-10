//! Deterministically hold the terminal-status -> registry-release gap.
use super::*;

use anyhow::{Context, Result};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::time::{Duration, timeout};

struct Fixture {
    _root: TempDir,
    runtime: Arc<ServerRuntime>,
    connection: u64,
    session: SessionId,
    turn: crate::turn::RuntimeTurn,
    item: devo_protocol::native::item::ItemEnvelope,
}

impl Fixture {
    async fn new() -> Result<Self> {
        let root = TempDir::new()?;
        let runtime = crate::test_support::TestRuntime::noop_text()
            .with_named_model("test-model", "Test Model")
            .runtime(root.path());
        let (tx, mut events) = crate::test_outbound_channel(256);
        let connection = runtime
            .register_connection(crate::ClientTransportKind::Stdio, tx)
            .await;
        let request = |method, params| json!({"id": 1, "method": method, "params": params});
        let reply = runtime
            .handle_incoming(
                connection,
                request(
                    "initialize",
                    json!({
                        "protocolVersion": 1, "clientCapabilities": {},
                        "_meta": {"devo": {"protocol": "native"}},
                        "clientInfo": {"name": "edit-release-test", "version": "1"}
                    }),
                ),
            )
            .await
            .context("initialize")?;
        assert_eq!(reply.get("error"), None);
        let reply = runtime
            .handle_incoming(
                connection,
                request(
                    "session/new",
                    json!({
                        "cwd": root.path(), "idempotencyKey": "new"
                    }),
                ),
            )
            .await
            .context("session/new")?;
        let session: SessionId = serde_json::from_value(reply["result"]["session"]["id"].clone())?;
        let reply = runtime
            .handle_incoming(
                connection,
                request(
                    "session/metadata/update",
                    json!({
                        "sessionId": session, "expectedVersion": 0,
                        "model": {"provider": "", "model": "test-model"}
                    }),
                ),
            )
            .await
            .context("model")?;
        assert_eq!(reply.get("error"), None);
        let reply = runtime
            .handle_incoming(
                connection,
                request(
                    "turn/start",
                    json!({
                        "sessionId": session, "input": [{"type": "text", "text": "original"}],
                        "idempotencyKey": "start"
                    }),
                ),
            )
            .await
            .context("turn/start")?;
        assert_eq!(reply.get("error"), None);
        // RecoveryUpdated is emitted after MergeTurn and registry cleanup.
        timeout(Duration::from_secs(20), async {
            while let Some(event) = events.recv().await {
                if event["method"] == "turn/recoveryUpdated"
                    && event["params"]["sessionId"] == session.to_string()
                {
                    return Ok(());
                }
            }
            anyhow::bail!("missing post-merge notification")
        })
        .await??;
        let handle = runtime.session(session).await.context("handle")?;
        let turn = handle
            .turn_reservation_snapshot()
            .await
            .context("snapshot")?
            .latest_turn
            .context("finished turn")?;
        let history = runtime
            .load_canonical_history(&json!(1), session)
            .await
            .map_err(|value| anyhow::anyhow!("{value}"))?;
        let item = history
            .items
            .iter()
            .find(|item| {
                matches!(
                    item.item,
                    devo_protocol::native::item::Item::UserMessage { .. }
                )
            })
            .cloned()
            .context("user item")?;
        assert!(
            runtime
                .recent_terminal_turn_status(turn.turn_id())
                .await
                .is_some()
        );
        // Restore ONLY the still-registered execution: durable state and terminal
        // status already exist, exactly as in the post-MergeTurn cleanup gap.
        runtime
            .register_runtime_active_turn(session, turn.clone())
            .await;
        Ok(Self {
            _root: root,
            runtime,
            connection,
            session,
            turn,
            item,
        })
    }

    fn edit(&self) -> tokio::task::JoinHandle<Value> {
        let runtime = Arc::clone(&self.runtime);
        let connection = self.connection;
        let params = json!({
            "sessionId": self.session, "itemId": self.item.id, "expectedRevision": 0,
            "content": [{"type": "text", "text": "replacement"}],
            "workspaceRestore": "skip", "idempotencyKey": "edit"
        });
        tokio::spawn(async move {
            runtime
                .handle_native_session_message_edit(connection, json!(2), params)
                .await
        })
    }
}

#[tokio::test]
async fn terminal_busy_edit_waits_for_release_without_holding_state_gate() -> Result<()> {
    let fixture = Fixture::new().await?;
    tokio::time::pause();
    let mut edit = fixture.edit();
    assert!(
        timeout(Duration::from_secs(1), &mut edit).await.is_err(),
        "terminal status alone must not finish the edit"
    );
    let handle = fixture
        .runtime
        .session(fixture.session)
        .await
        .context("handle")?;
    let guard = timeout(Duration::from_secs(1), handle.lock_state_change()).await?;
    drop(guard);
    fixture
        .runtime
        .clear_active_turn_runtime_handles(fixture.session)
        .await;
    let response = timeout(Duration::from_secs(2), edit).await??;
    assert_eq!(response.get("error"), None, "{response}");
    assert_eq!(response["result"]["editState"], json!("accepted"));
    assert_eq!(
        response["result"]["item"]["item"]["content"],
        json!([{"type": "text", "text": "replacement"}])
    );
    Ok(())
}

#[tokio::test]
async fn terminal_busy_edit_rejects_newer_turn_instead_of_canceling_it() -> Result<()> {
    let fixture = Fixture::new().await?;
    tokio::time::pause();
    let mut edit = fixture.edit();
    assert!(timeout(Duration::from_secs(1), &mut edit).await.is_err());
    let handle = fixture
        .runtime
        .session(fixture.session)
        .await
        .context("handle")?;
    let gate = handle.lock_state_change().await;
    fixture
        .runtime
        .clear_active_turn_runtime_handles(fixture.session)
        .await;
    let mut newer = fixture.turn.clone();
    newer.native.id = TurnId::new();
    newer.native.status = devo_protocol::native::turn::TurnStatus::InProgress;
    fixture
        .runtime
        .register_runtime_active_turn(fixture.session, newer.clone())
        .await;
    drop(gate);
    let response = timeout(Duration::from_secs(2), edit).await??;
    assert_eq!(response["error"]["code"], json!("ActiveTurnEditRejected"));
    assert_eq!(
        fixture
            .runtime
            .active_turns
            .active_turn(fixture.session)
            .await,
        Some(newer.native)
    );
    let history = fixture
        .runtime
        .load_canonical_history(&json!(1), fixture.session)
        .await
        .map_err(|value| anyhow::anyhow!("{value}"))?;
    assert_eq!(
        history
            .items
            .iter()
            .find(|item| item.id == fixture.item.id)
            .cloned(),
        Some(fixture.item)
    );
    Ok(())
}

#[tokio::test]
async fn terminal_busy_edit_times_out_without_rewriting_or_clearing_execution() -> Result<()> {
    let fixture = Fixture::new().await?;
    tokio::time::pause();
    let response = timeout(Duration::from_secs(6), fixture.edit()).await??;
    assert_eq!(response["error"]["code"], json!("ActiveTurnEditRejected"));
    assert_eq!(
        response["error"]["message"],
        json!("the interrupted turn is still finalizing; retry when it has finished")
    );
    assert_eq!(
        fixture
            .runtime
            .active_turns
            .active_turn(fixture.session)
            .await,
        Some(fixture.turn.native)
    );
    let history = fixture
        .runtime
        .load_canonical_history(&json!(1), fixture.session)
        .await
        .map_err(|value| anyhow::anyhow!("{value}"))?;
    assert_eq!(
        history
            .items
            .iter()
            .find(|item| item.id == fixture.item.id)
            .cloned(),
        Some(fixture.item)
    );
    Ok(())
}
