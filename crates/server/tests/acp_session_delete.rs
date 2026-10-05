#[path = "support/acp_runtime_harness.rs"]
mod acp_runtime_harness;

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::SessionId;
use devo_protocol::StreamEvent;
use devo_protocol::native::session::Session;
use devo_provider::ModelProviderSDK;
use devo_server::AcpDeleteSessionResult;
use devo_server::AcpListSessionsResult;
use devo_server::AcpSessionDeleteCapabilities;
use devo_server::AcpSuccessResponse;
use devo_server::ServerRuntime;
use devo_server::acp_session_info_from_native_session;
use futures::Stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Duration;
use tokio::time::timeout;

use acp_runtime_harness::AcpTestClient;
use acp_runtime_harness::build_acp_test_runtime;
use acp_runtime_harness::connect_acp_stdio;
use acp_runtime_harness::connect_native_stdio;
use acp_runtime_harness::create_acp_session;
use acp_runtime_harness::delete_acp_session;
use acp_runtime_harness::list_acp_sessions;
use acp_runtime_harness::native_session_from_new;
use acp_runtime_harness::wait_for_notification_method;

struct BlockingProvider {
    started_tx: std::sync::Mutex<Option<oneshot::Sender<()>>>,
}

impl BlockingProvider {
    fn new(started_tx: oneshot::Sender<()>) -> Self {
        Self {
            started_tx: std::sync::Mutex::new(Some(started_tx)),
        }
    }

    fn mark_started(&self) {
        if let Some(started_tx) = self.started_tx.lock().expect("started mutex").take() {
            let _ = started_tx.send(());
        }
    }
}

#[async_trait]
impl ModelProviderSDK for BlockingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        self.mark_started();
        std::future::pending::<Result<ModelResponse>>().await
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.mark_started();
        std::future::pending::<Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>>>()
            .await
    }

    fn name(&self) -> &str {
        "blocking-acp-delete-provider"
    }
}

#[tokio::test]
async fn acp_session_delete_removes_session_from_history_and_is_idempotent() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_delete_runtime(data_root.path(), noop_provider())?;
    let connection = connect_delete_acp(&runtime).await?;
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;
    let new_session = create_acp_session(
        &runtime,
        connection.connection_id,
        &cwd,
        2,
        serde_json::json!({}),
    )
    .await?;
    let native_session: Session = native_session_from_new(&new_session)?;
    let session_id = new_session.session_id;

    assert_eq!(
        list_acp_sessions(&runtime, connection.connection_id, 3, Some(&cwd), None).await?,
        AcpListSessionsResult {
            sessions: vec![acp_session_info_from_native_session(&native_session)],
            next_cursor: None,
            meta: None,
        }
    );
    assert_eq!(
        delete_acp_session(&runtime, connection.connection_id, 4, &session_id).await?,
        AcpSuccessResponse::new(serde_json::json!(4), AcpDeleteSessionResult::default())
    );
    assert_eq!(
        list_acp_sessions(&runtime, connection.connection_id, 5, Some(&cwd), None).await?,
        AcpListSessionsResult {
            sessions: Vec::new(),
            next_cursor: None,
            meta: None,
        }
    );
    assert_eq!(
        delete_acp_session(&runtime, connection.connection_id, 6, &session_id).await?,
        AcpSuccessResponse::new(serde_json::json!(6), AcpDeleteSessionResult::default())
    );
    Ok(())
}

#[tokio::test]
async fn acp_session_delete_cancels_running_session_before_removal() -> Result<()> {
    let data_root = TempDir::new()?;
    let (started_tx, started_rx) = oneshot::channel();
    let runtime = build_delete_runtime(
        data_root.path(),
        Arc::new(BlockingProvider::new(started_tx)),
    )?;
    let connection = connect_delete_acp(&runtime).await?;
    let (native_connection_id, mut notifications_rx) = connect_native_stdio(&runtime, 1).await?;
    let session =
        create_delete_session(&runtime, connection.connection_id, 29, data_root.path()).await?;
    // Turn lifecycle events ride session subscriptions (08 §4): the native
    // connection must subscribe before it can observe turn/interrupted.
    subscribe_to_session_events(&runtime, native_connection_id, 28, session).await?;
    start_turn(&runtime, native_connection_id, 30, session).await?;
    timeout(Duration::from_secs(5), started_rx)
        .await
        .context("timed out waiting for blocking provider to start")?
        .context("blocking provider start signal dropped")?;
    assert!(session_rollout_exists(data_root.path(), session)?);

    assert_eq!(
        timeout(
            Duration::from_secs(5),
            delete_acp_session(&runtime, connection.connection_id, 31, &session)
        )
        .await
        .context("session/delete timed out while turn was running")??,
        AcpSuccessResponse::new(serde_json::json!(31), AcpDeleteSessionResult::default())
    );
    // Interruption surfaces as a `turn/completed` lifecycle event with an
    // interrupted status — there is no separate `turn/interrupted` method.
    let completed = wait_for_notification_method(&mut notifications_rx, "turn/completed")
        .await
        .context("running delete should interrupt the active turn")?;
    assert_eq!(
        completed["params"]["turn"]["status"],
        serde_json::json!("interrupted")
    );
    assert_eq!(
        list_acp_sessions(
            &runtime,
            connection.connection_id,
            32,
            Some(data_root.path()),
            None
        )
        .await?,
        AcpListSessionsResult {
            sessions: Vec::new(),
            next_cursor: None,
            meta: None,
        }
    );
    assert!(!session_rollout_exists(data_root.path(), session)?);
    Ok(())
}

#[tokio::test]
async fn acp_session_delete_keeps_user_fork_children() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_delete_runtime(data_root.path(), noop_provider())?;
    let connection = connect_delete_acp(&runtime).await?;
    let (native_connection_id, mut notifications_rx) = connect_native_stdio(&runtime, 1).await?;
    let root =
        create_delete_session(&runtime, connection.connection_id, 7, data_root.path()).await?;
    // Turn lifecycle events ride session subscriptions (08 §4): the native
    // connection must subscribe before it can observe turn/completed.
    subscribe_to_session_events(&runtime, native_connection_id, 6, root).await?;
    start_and_complete_turn(&runtime, native_connection_id, &mut notifications_rx, root).await?;
    let child_session_id = fork_session(&runtime, native_connection_id, root).await?;

    assert_eq!(
        list_acp_sessions(
            &runtime,
            connection.connection_id,
            11,
            Some(data_root.path()),
            None
        )
        .await?
        .sessions
        .len(),
        2
    );
    assert!(session_rollout_exists(data_root.path(), root)?);
    assert!(session_rollout_exists(data_root.path(), child_session_id)?);

    delete_acp_session(&runtime, connection.connection_id, 12, &root).await?;
    let listed = list_acp_sessions(
        &runtime,
        connection.connection_id,
        13,
        Some(data_root.path()),
        None,
    )
    .await?;
    assert_eq!(listed.sessions.len(), 1);
    assert_eq!(listed.sessions[0].session_id, child_session_id);
    assert!(!session_rollout_exists(data_root.path(), root)?);
    assert!(session_rollout_exists(data_root.path(), child_session_id)?);

    let rebuilt = build_delete_runtime(data_root.path(), noop_provider())?;
    rebuilt.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _) = connect_native_stdio(&rebuilt, 1).await?;
    let resume_response = rebuilt
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 14,
                "method": "session/resume",
                "params": { "sessionId": child_session_id }
            }),
        )
        .await
        .context("session/resume forked child after parent delete")?;
    assert!(
        resume_response.get("result").is_some(),
        "forked child must remain resumable after parent delete: {resume_response}"
    );
    Ok(())
}

#[tokio::test]
async fn acp_session_delete_broadcasts_deleted_session_ids() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_delete_runtime(data_root.path(), noop_provider())?;
    let owner = connect_delete_acp(&runtime).await?;
    let (observer_connection_id, mut observer_notifications_rx) =
        connect_native_stdio(&runtime, 1).await?;
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;
    let new_session = create_delete_session(&runtime, owner.connection_id, 21, &cwd).await?;
    subscribe_to_session_events(&runtime, observer_connection_id, 20, new_session).await?;

    assert_eq!(
        delete_acp_session(&runtime, owner.connection_id, 22, &new_session).await?,
        AcpSuccessResponse::new(serde_json::json!(22), AcpDeleteSessionResult::default())
    );
    let notification =
        wait_for_notification_method(&mut observer_notifications_rx, "session/deleted")
            .await
            .context("observer should receive session/deleted broadcast")?;
    assert_eq!(
        notification["params"]["deletedSessionIds"],
        serde_json::json!([new_session])
    );
    Ok(())
}

#[tokio::test]
async fn delete_session_with_active_turn_and_pending_queue() -> Result<()> {
    let data_root = TempDir::new()?;
    let (started_tx, started_rx) = oneshot::channel();
    let runtime = build_delete_runtime(
        data_root.path(),
        Arc::new(BlockingProvider::new(started_tx)),
    )?;
    let connection = connect_delete_acp(&runtime).await?;
    let (native_connection_id, _) = connect_native_stdio(&runtime, 1).await?;
    let session_id =
        create_delete_session(&runtime, connection.connection_id, 40, data_root.path()).await?;
    start_turn(&runtime, native_connection_id, 41, session_id).await?;
    timeout(Duration::from_secs(5), started_rx)
        .await
        .context("timed out waiting for blocking provider to start")?
        .context("blocking provider start signal dropped")?;

    for (request_id, text) in [(42, "queued one"), (43, "queued two")] {
        let response = runtime
            .handle_incoming(
                native_connection_id,
                serde_json::json!({
                    "id": request_id,
                    "method": "session/queue/push",
                    "params": {
                        "sessionId": session_id.to_string(),
                        "input": [{ "type": "text", "text": text }],
                        "idempotencyKey": format!("queue-{request_id}"),
                    }
                }),
            )
            .await
            .with_context(|| format!("session/queue/push {text} response"))?;
        anyhow::ensure!(
            response.get("error").is_none(),
            "session/queue/push failed: {response}"
        );
    }

    delete_acp_session(&runtime, connection.connection_id, 44, &session_id).await?;
    let queue_list_response = runtime
        .handle_incoming(
            native_connection_id,
            serde_json::json!({
                "id": 45,
                "method": "session/queue/list",
                "params": { "sessionId": session_id.to_string() },
            }),
        )
        .await
        .context("session/queue/list response")?;
    assert_eq!(
        queue_list_response["error"]["code"],
        serde_json::json!("SessionNotFound")
    );
    let db = devo_server::db::Database::open(data_root.path().join("acp_session_delete.db"))?;
    assert!(
        db.list_pending(&session_id, devo_server::db::QueueType::Turn)?
            .is_empty()
    );
    assert!(
        db.list_pending(&session_id, devo_server::db::QueueType::Steer)?
            .is_empty()
    );
    assert!(!session_rollout_exists(data_root.path(), session_id)?);
    Ok(())
}

fn noop_provider() -> Arc<dyn ModelProviderSDK> {
    Arc::new(devo_server::test_support::NoopProvider::text().named("noop-acp-delete-provider"))
}

fn build_delete_runtime(
    data_root: &Path,
    provider: Arc<dyn ModelProviderSDK>,
) -> Result<Arc<ServerRuntime>> {
    build_acp_test_runtime(data_root, provider, "acp_session_delete.db")
}

async fn connect_delete_acp(
    runtime: &Arc<ServerRuntime>,
) -> Result<acp_runtime_harness::AcpConnection> {
    let connection = connect_acp_stdio(
        runtime,
        AcpTestClient::new("acp-session-delete-test", "ACP Session Delete Test"),
    )
    .await?;
    assert_eq!(
        connection
            .initialize
            .agent_capabilities
            .session_capabilities
            .delete,
        Some(AcpSessionDeleteCapabilities::default())
    );
    Ok(connection)
}

async fn create_delete_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    cwd: &Path,
) -> Result<SessionId> {
    let result = create_acp_session(
        runtime,
        connection_id,
        cwd,
        request_id,
        serde_json::json!({}),
    )
    .await?;
    let session: Session = native_session_from_new(&result)?;
    // Native ACP sessions expose their real session id (`ses_…`); the legacy
    // UUID shape is gone from the adapter surface.
    Ok(session.id)
}

async fn start_and_complete_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    session_id: SessionId,
) -> Result<()> {
    start_turn(runtime, connection_id, 8, session_id).await?;
    wait_for_notification_method(notifications_rx, "turn/completed")
        .await
        .map(|_| ())
}

async fn start_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: SessionId,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "seed fork history" }],
                    "idempotencyKey": format!("turn-{request_id}")
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult> =
        serde_json::from_value(response)?;
    Ok(())
}

async fn fork_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
) -> Result<SessionId> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 9,
                "method": "session/fork",
                "params": { "sessionId": session_id }
            }),
        )
        .await
        .context("session/fork response")?;
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionForkResult,
    > = serde_json::from_value(response)?;
    Ok(SessionId::from(response.result.session.id.as_str()))
}

async fn subscribe_to_session_events(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: SessionId,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "subscription/create",
                "params": {
                    "selectors": [{ "kind": "session", "sessionId": session_id }],
                    "includeSnapshot": false
                }
            }),
        )
        .await
        .context("subscription/create response")?;
    anyhow::ensure!(response["result"]["subscriptionId"].is_string());
    Ok(())
}

fn session_rollout_exists(data_root: &Path, session_id: SessionId) -> Result<bool> {
    fn visit(path: &Path, session_id: SessionId) -> Result<bool> {
        if !path.exists() {
            return Ok(false);
        }
        for entry in std::fs::read_dir(path)
            .with_context(|| format!("read rollout directory {}", path.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if visit(&path, session_id)? {
                    return Ok(true);
                }
                continue;
            }
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == format!("{session_id}.jsonl"))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    if visit(&data_root.join("sessions"), session_id)? {
        return Ok(true);
    }
    visit(&data_root.join("session-artifacts"), session_id)
}
