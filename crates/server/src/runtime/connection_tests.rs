use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use devo_core::AppConfigStore;
use devo_core::PresetModelCatalog;
use devo_protocol::DEVO_ACTIVITY_AT_META;
use devo_protocol::DEVO_ITEM_ID_META;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::StreamEvent;
use devo_protocol::native::ids::{
    ItemId as NativeItemId, SessionId as NativeSessionId, TurnId as NativeTurnId,
};
use devo_protocol::native::item::ItemState;
use devo_protocol::native::wire_projector::typed_item_envelope;
use devo_provider::ModelProviderSDK;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::*;
use crate::ItemDeltaKind;
use crate::item_delta_notification;
use crate::test_support::NoopProvider;
use crate::test_support::TestRuntime;

fn build_runtime(data_root: &std::path::Path) -> Arc<ServerRuntime> {
    TestRuntime::noop()
        .db_file("connection.db")
        .runtime(data_root)
}

fn build_runtime_with_provider(
    data_root: &std::path::Path,
    provider: Arc<dyn ModelProviderSDK>,
) -> Arc<ServerRuntime> {
    TestRuntime::new(provider)
        .db_file("connection.db")
        .runtime(data_root)
}

fn build_runtime_with_provider_and_catalog(
    data_root: &std::path::Path,
    provider: Arc<dyn ModelProviderSDK>,
    model_catalog: Arc<PresetModelCatalog>,
) -> Arc<ServerRuntime> {
    TestRuntime::new(provider)
        .catalog(model_catalog)
        .db_file("connection.db")
        .runtime(data_root)
}

fn build_runtime_with_provider_catalog_and_protocols(
    data_root: &std::path::Path,
    provider: Arc<dyn ModelProviderSDK>,
    model_catalog: Arc<PresetModelCatalog>,
    protocols: ProtocolSet,
) -> Arc<ServerRuntime> {
    TestRuntime::new(provider)
        .catalog(model_catalog)
        .db_file("connection.db")
        .protocols(protocols)
        .runtime(data_root)
}

async fn rpc(
    runtime: &Arc<ServerRuntime>,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    history_request(
        runtime,
        initialized_connection(runtime).await,
        id,
        method,
        params,
    )
    .await
}

struct NativeEnv {
    data_root: TempDir,
    runtime: Arc<ServerRuntime>,
    connection_id: u64,
}

impl NativeEnv {
    async fn connect() -> Result<Self> {
        Self::with_provider(Arc::new(NoopProvider::failing())).await
    }

    async fn with_provider(provider: Arc<dyn ModelProviderSDK>) -> Result<Self> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime_with_provider(data_root.path(), provider);
        let connection_id = initialized_connection(&runtime).await;
        Ok(Self {
            data_root,
            runtime,
            connection_id,
        })
    }

    async fn native() -> Result<Self> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_with_protocol_meta(&runtime, true).await;
        Ok(Self {
            data_root,
            runtime,
            connection_id,
        })
    }

    async fn gated() -> Result<(
        Self,
        Arc<std::sync::atomic::AtomicBool>,
        Arc<std::sync::atomic::AtomicBool>,
    )> {
        let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let data_root = TempDir::new()?;
        let runtime = build_runtime_with_provider(
            data_root.path(),
            Arc::new(GatedProvider {
                open: Arc::clone(&open),
                started: Arc::clone(&started),
            }),
        );
        let connection_id = initialized_connection(&runtime).await;
        Ok((
            Self {
                data_root,
                runtime,
                connection_id,
            },
            open,
            started,
        ))
    }

    fn cwd(&self) -> &std::path::Path {
        self.data_root.path()
    }

    async fn session(&self) -> Result<SessionId> {
        start_durable_session(&self.runtime, self.connection_id, self.cwd()).await
    }

    async fn rpc(&self, id: u64, method: &str, params: serde_json::Value) -> serde_json::Value {
        history_request(&self.runtime, self.connection_id, id, method, params).await
    }
}

struct DualConn {
    runtime: Arc<ServerRuntime>,
    session_id: SessionId,
    native_id: u64,
    acp_id: u64,
    native_rx: tokio::sync::mpsc::Receiver<serde_json::Value>,
    acp_rx: tokio::sync::mpsc::Receiver<serde_json::Value>,
}

impl DualConn {
    async fn subscribed(data_root: &std::path::Path, buffer: usize) -> Result<Self> {
        let runtime = build_runtime(data_root);
        let session_id = SessionId::new();
        let (native_out, native_rx) = super::outbound::test_outbound_channel(buffer);
        let (acp_out, acp_rx) = super::outbound::test_outbound_channel(buffer);
        let native_id = runtime
            .register_connection(ClientTransportKind::Stdio, native_out)
            .await;
        let acp_id = runtime
            .register_connection(ClientTransportKind::Stdio, acp_out)
            .await;
        for (id, native) in [(native_id, true), (acp_id, false)] {
            let mut params = serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "terminal": false },
            });
            if native {
                params["_meta"] = serde_json::json!({ "devo": { "protocol": "native" } });
            }
            runtime
                .handle_acp_initialize(id, Some(serde_json::json!(id)), params)
                .await;
            runtime
                .subscribe_connection_to_session(id, session_id, None)
                .await;
        }
        Ok(Self {
            runtime,
            session_id,
            native_id,
            acp_id,
            native_rx,
            acp_rx,
        })
    }

    async fn set_native_selector(&self) {
        self.runtime
            .connections
            .lock()
            .await
            .get_mut(&self.native_id)
            .expect("native connection")
            .event_selectors = vec![devo_protocol::native::event::StreamSelector::Session {
            session_id: self.session_id,
        }];
    }
}

async fn recv_frame(
    rx: &mut tokio::sync::mpsc::Receiver<serde_json::Value>,
) -> Result<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await?
        .context("recv")
}

async fn recv_none(rx: &mut tokio::sync::mpsc::Receiver<serde_json::Value>, ms: u64) {
    assert!(
        tokio::time::timeout(Duration::from_millis(ms), rx.recv())
            .await
            .is_err()
    );
}

fn json_result<T: serde::de::DeserializeOwned>(response: &serde_json::Value, method: &str) -> T {
    serde_json::from_value(response["result"].clone())
        .unwrap_or_else(|error| panic!("{method} result from {response}: {error}"))
}

async fn wait_turn_idle(runtime: &Arc<ServerRuntime>, session_id: SessionId) {
    let wait = async {
        while runtime.runtime_active_turn_id(session_id).await.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    // Kernel startup can take several seconds on Windows under parallel load.
    // Fail at the wait itself instead of silently continuing with a busy turn.
    tokio::time::timeout(Duration::from_secs(30), wait)
        .await
        .expect("turn must reach idle before the next request");
}

async fn wait_flag(flag: &std::sync::atomic::AtomicBool, message: &str) {
    let wait = async {
        while !flag.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .expect(message);
}

fn init_git_repo(repo: &std::path::Path) {
    git_in(repo, &["init"]);
    git_in(repo, &["config", "user.email", "rollback@example.com"]);
    git_in(repo, &["config", "user.name", "Rollback Test"]);
    std::fs::write(repo.join("tracked.txt"), "initial\n").expect("write tracked");
    git_in(repo, &["add", "tracked.txt"]);
    git_in(repo, &["commit", "-m", "initial"]);
}

fn git_in(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(output.status.success(), "git {args:?} failed");
    output
}

async fn connect_stdio(
    runtime: &Arc<ServerRuntime>,
    native: bool,
    buffer: usize,
) -> (u64, tokio::sync::mpsc::Receiver<serde_json::Value>) {
    let (outbound, rx) = super::outbound::test_outbound_channel(buffer);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    let mut params = serde_json::json!({
        "protocolVersion": 1,
        "clientCapabilities": { "terminal": false },
    });
    if native {
        params["_meta"] = serde_json::json!({ "devo": { "protocol": "native" } });
    }
    runtime
        .handle_acp_initialize(connection_id, Some(serde_json::json!(1)), params)
        .await;
    (connection_id, rx)
}

#[tokio::test]
async fn mcp_rpc_branches() {
    let temp = TempDir::new().expect("temp dir");
    {
        let mut store =
            AppConfigStore::load(temp.path().to_path_buf(), None).expect("load app config store");
        store
            .upsert_mcp_server(devo_core::McpServerRecord {
                id: devo_core::McpServerId("bad_mcp".to_string()),
                display_name: "Bad MCP".to_string(),
                transport: devo_core::McpTransportConfig::Stdio {
                    command: vec!["__devo_missing_mcp_binary__".to_string()],
                    cwd: None,
                    env: Default::default(),
                    env_vars: Vec::new(),
                },
                startup_policy: devo_core::McpStartupPolicy::Lazy,
                enabled: false,
                trust_policy: Default::default(),
                allowed_capabilities: Vec::new(),
                roots_policy: Default::default(),
                output_limits: Default::default(),
                auth_ref: None,
            })
            .expect("upsert bad mcp server");
    }
    let mcp_manager = Arc::new(devo_mcp::manager::RmcpMcpManager::new(
        {
            let store = AppConfigStore::load(temp.path().to_path_buf(), None)
                .expect("reload app config store");
            store.effective_config().mcp_runtime.clone()
        },
        Default::default(),
    ));
    let runtime = TestRuntime::noop()
        .mcp(mcp_manager)
        .db_file("connection.db")
        .runtime(temp.path());

    let unknown_tools = rpc(
        &runtime,
        3,
        "mcp/tools",
        serde_json::json!({ "name": "missing-server" }),
    )
    .await;
    let error: ErrorResponse = serde_json::from_value(unknown_tools).expect("deserialize error");
    assert_eq!(error.error.code, ProtocolErrorCode::InvalidParams);

    let disabled_tools = rpc(
        &runtime,
        7,
        "mcp/tools",
        serde_json::json!({ "name": "bad_mcp" }),
    )
    .await;
    let result: SuccessResponse<devo_protocol::native::rpc_admin::McpToolsResult> =
        serde_json::from_value(disabled_tools).expect("deserialize mcp/tools");
    assert_eq!(
        result.result,
        devo_protocol::native::rpc_admin::McpToolsResult { tools: Vec::new() }
    );

    let list = rpc(&runtime, 2, "mcp/list", serde_json::json!({})).await;
    let result: SuccessResponse<devo_protocol::native::rpc_admin::McpListResult> =
        serde_json::from_value(list).expect("deserialize mcp/list");
    let disabled_server = result
        .result
        .servers
        .iter()
        .find(|server| server.name == "bad_mcp")
        .expect("configured disabled server should be listed");
    assert_eq!(
        (disabled_server.status.as_str(), disabled_server.tool_count),
        ("disabled", 0)
    );

    let connection_id = initialized_connection(&runtime).await;
    let enabled = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 4,
                "method": "mcp/set_enabled",
                "params": { "name": "bad_mcp", "enabled": true }
            }),
        )
        .await
        .expect("mcp/set_enabled response");
    let result: SuccessResponse<devo_protocol::native::rpc_admin::McpSetEnabledResult> =
        serde_json::from_value(enabled).expect("deserialize mcp/set_enabled");
    let bad = result
        .result
        .servers
        .iter()
        .find(|server| server.name == "bad_mcp")
        .expect("bad_mcp should be listed");
    assert_eq!((bad.status.as_str(), bad.tool_count), ("failed", 0));

    let unknown_enabled = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 6,
                "method": "mcp/set_enabled",
                "params": { "name": "missing-server", "enabled": true }
            }),
        )
        .await
        .expect("mcp/set_enabled response");
    let error: ErrorResponse = serde_json::from_value(unknown_enabled).expect("deserialize error");
    assert_eq!(error.error.code, ProtocolErrorCode::InternalError);
}

fn assert_agent_message_chunk_update(update: &serde_json::Value, item_id: NativeItemId) {
    assert!(update["_meta"][DEVO_ACTIVITY_AT_META].is_string());
    let mut stable_update = update.clone();
    stable_update["_meta"]
        .as_object_mut()
        .expect("ACP update meta")
        .remove(DEVO_ACTIVITY_AT_META);
    assert_eq!(
        stable_update,
        serde_json::json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {
                "type": "text",
                "text": "hello",
            },
            "messageId": item_id.as_str(),
            "_meta": {
                DEVO_ITEM_ID_META: item_id.as_str(),
            },
        })
    );
}

#[test]
fn notification_policy_and_subscription_filter_unit_tests() {
    let session_id = SessionId::new();
    let item_id = NativeItemId::new();
    let item = ServerNotification::ItemCompleted {
        item: Box::new(typed_item_envelope(
            NativeSessionId::new(),
            NativeTurnId::new(),
            NativeItemId::new(),
            0,
            &devo_protocol::native::item::Item::ContextCompaction {
                trigger: devo_protocol::native::item::CompactionTrigger::AutoThreshold,
                before: Default::default(),
                after: None,
                summary: Some("Context compacted".into()),
            },
            ItemState::Completed,
            Utc::now(),
            None,
        )),
    };
    let delta = item_delta_notification(
        ItemDeltaKind::AgentMessageDelta,
        session_id,
        item_id,
        0,
        "token",
    );
    assert_eq!(
        (
            notification_delivery_policy(&item),
            notification_delivery_policy(&delta),
        ),
        (
            OutboundDeliveryPolicy::Reliable,
            OutboundDeliveryPolicy::BestEffort,
        )
    );

    let parent = SessionId::new();
    let child = SessionId::new();
    let unrelated = SessionId::new();
    let child_parent_by_session = HashMap::from([(child, parent)]);
    let subscription = SubscriptionFilter {
        session_id: Some(parent),
        event_types: HashSet::new(),
        include_child_agents: true,
    };
    assert_eq!(
        vec![true, true, false],
        vec![
            subscription.session_matches(Some(parent), &child_parent_by_session),
            subscription.session_matches(Some(child), &child_parent_by_session),
            subscription.session_matches(Some(unrelated), &child_parent_by_session),
        ]
    );

    let subscribed_session = SessionId::new();
    let scoped = SubscriptionFilter {
        session_id: Some(subscribed_session),
        event_types: HashSet::new(),
        include_child_agents: false,
    };
    assert!(!scoped.session_matches(None, &HashMap::new()));
    assert!(scoped.session_matches(Some(subscribed_session), &HashMap::new()));
}

#[tokio::test]
async fn post_response_actions_run_after_backpressured_response_enqueue() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let (outbound_tx, mut receiver) = super::outbound::test_outbound_channel(1);
    let transport_outbound = outbound_tx.clone();
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, outbound_tx)
        .await;
    let session_id = SessionId::new();
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {},
    });
    let expected_response = response.clone();

    assert!(
        enqueue_outbound(
            &transport_outbound,
            OutboundFrame::json_rpc_response(
                connection_id,
                serde_json::json!({ "queued": "backpressure" }),
            ),
            "test_prefill",
        )
        .await
    );
    let runtime_for_task = Arc::clone(&runtime);
    let snapshot_session_id = session_id;
    let mut transport_task = tokio::spawn(async move {
        let incoming = IncomingResponse::new(response).with_post_response_action(
            PostResponseAction::SendAcpSessionStateSnapshot {
                connection_id,
                session_id: snapshot_session_id,
            },
        );
        let (response, post_response_actions) = incoming.into_parts();
        assert!(
            enqueue_outbound(
                &transport_outbound,
                OutboundFrame::json_rpc_response(connection_id, response),
                "test_response",
            )
            .await
        );
        runtime_for_task
            .run_post_response_actions(post_response_actions)
            .await;
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut transport_task)
            .await
            .is_err()
    );
    assert_eq!(
        receiver.recv().await.expect("prefilled queue item"),
        serde_json::json!({ "queued": "backpressure" })
    );
    assert_eq!(
        receiver.recv().await.expect("json-rpc response"),
        expected_response
    );
    let notification = receiver.recv().await.expect("post-response notification");
    assert_eq!(
        notification.get("method"),
        Some(&serde_json::json!(crate::ACP_SESSION_UPDATE_METHOD))
    );
    assert_eq!(
        notification
            .get("params")
            .and_then(|params| params.get("sessionId")),
        Some(&serde_json::to_value(session_id).expect("serialize session id"))
    );
    assert_eq!(
        notification
            .get("params")
            .and_then(|params| params.get("update"))
            .and_then(|update| update.get("sessionUpdate")),
        Some(&serde_json::json!("available_commands_update"))
    );
    transport_task.await.expect("transport sequence completes");

    Ok(())
}

async fn setup_acp_delta_pair(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    owner_kind: ClientTransportKind,
    watcher_kind: ClientTransportKind,
) -> (
    u64,
    tokio::sync::mpsc::Receiver<serde_json::Value>,
    u64,
    tokio::sync::mpsc::Receiver<serde_json::Value>,
) {
    let (owner_out, owner_rx) = super::outbound::test_outbound_channel(4);
    let (watcher_out, watcher_rx) = super::outbound::test_outbound_channel(4);
    let owner_id = runtime.register_connection(owner_kind, owner_out).await;
    let watcher_id = runtime.register_connection(watcher_kind, watcher_out).await;
    {
        let mut connections = runtime.connections.lock().await;
        for id in [owner_id, watcher_id] {
            connections.get_mut(&id).expect("connection").protocol = Some(ConnectionProtocol::Acp);
        }
    }
    runtime
        .subscribe_connection_to_session(owner_id, session_id, None)
        .await;
    runtime
        .subscribe_connection_to_session(watcher_id, session_id, None)
        .await;
    runtime
        .active_turns
        .set_connection_id(session_id, owner_id)
        .await;
    (owner_id, owner_rx, watcher_id, watcher_rx)
}

/// Delta delivery rules: any SUBSCRIBED connection receives assistant deltas
/// for the session regardless of transport or which connection drives the
/// turn. A proxy-transport observer is a full client (second TUI on the
/// internal proxy websocket) — suppressing its deltas left its transcript
/// with the user message but no reply. Unsubscribed connections (one-shot
/// CLI clients) still receive nothing via the subscription gate.
#[tokio::test]
async fn stdio_live_agent_delta_delivery_rules() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let item_id = NativeItemId::new();
    let delta = |session_id| {
        item_delta_notification(
            ItemDeltaKind::AgentMessageDelta,
            session_id,
            item_id,
            0,
            "hello",
        )
    };

    let session_id = SessionId::new();
    let (_, mut owner_rx, _, mut observer_rx) = setup_acp_delta_pair(
        &runtime,
        session_id,
        ClientTransportKind::Stdio,
        ClientTransportKind::StdioProxy,
    )
    .await;
    runtime.broadcast_notification(delta(session_id)).await;
    for rx in [&mut owner_rx, &mut observer_rx] {
        assert_agent_message_chunk_update(&recv_frame(rx).await?["params"]["update"], item_id);
    }

    let session_id = SessionId::new();
    let (_, mut owner_rx, _, mut watcher_rx) = setup_acp_delta_pair(
        &runtime,
        session_id,
        ClientTransportKind::StdioProxy,
        ClientTransportKind::Stdio,
    )
    .await;
    runtime.broadcast_notification(delta(session_id)).await;
    for rx in [&mut owner_rx, &mut watcher_rx] {
        assert_agent_message_chunk_update(&recv_frame(rx).await?["params"]["update"], item_id);
    }

    use devo_protocol::native::rpc_search::{SearchId, SearchSnapshot};
    let session_id = SessionId::new();
    let (owner_out, mut owner_rx) = super::outbound::test_outbound_channel(4);
    let (other_out, mut other_rx) = super::outbound::test_outbound_channel(4);
    let owner_id = runtime
        .register_connection(ClientTransportKind::Stdio, owner_out)
        .await;
    let _other_id = runtime
        .register_connection(ClientTransportKind::Stdio, other_out)
        .await;
    runtime
        .connections
        .lock()
        .await
        .get_mut(&owner_id)
        .expect("owner")
        .protocol = Some(ConnectionProtocol::Native);
    runtime
        .subscribe_connection_to_session(owner_id, session_id, None)
        .await;
    runtime
        .emit_connection_local_notification(
            owner_id,
            ServerNotification::SearchCompleted(SearchSnapshot {
                search_id: SearchId::new(),
                query: "src".into(),
                results: Vec::new(),
                total_file_match_count: 0,
                scanned_file_count: 0,
                file_search_complete: true,
            }),
        )
        .await;
    assert_eq!(
        recv_frame(&mut owner_rx).await?["method"],
        "search/completed"
    );
    recv_none(&mut other_rx, 50).await;
    Ok(())
}

#[tokio::test]
async fn native_and_acp_connections_receive_one_projected_event() -> Result<()> {
    use devo_protocol::native::item::{Item, ItemEnvelope, ItemState};

    let data_root = TempDir::new()?;
    let dual = DualConn::subscribed(data_root.path(), 1).await?;
    dual.set_native_selector().await;
    let native_turn_id = NativeTurnId::new();
    let native_item_id = NativeItemId::new();
    dual.runtime
        .broadcast_notification(ServerNotification::ItemCompleted {
            item: Box::new(typed_item_envelope(
                dual.session_id,
                native_turn_id,
                native_item_id,
                3,
                &Item::AssistantMessage {
                    text: "hello".into(),
                },
                ItemState::Completed,
                Utc::now(),
                None,
            )),
        })
        .await;

    let mut native_rx = dual.native_rx;
    let mut acp_rx = dual.acp_rx;
    let typed = recv_frame(&mut native_rx).await?;
    assert_eq!(typed["method"], serde_json::json!("item/completed"));
    let envelope: ItemEnvelope =
        serde_json::from_value(typed["params"]["item"].clone()).expect("native item envelope");
    assert_eq!(envelope.id.as_str(), native_item_id.as_str());
    assert_eq!(envelope.session_id.as_str(), dual.session_id.to_string());
    assert_eq!(envelope.turn_id.as_str(), native_turn_id.as_str());
    assert_eq!((envelope.seq, envelope.revision), (3, 1));
    assert_eq!(envelope.state, ItemState::Completed);
    assert_eq!(
        envelope.item,
        Item::AssistantMessage {
            text: "hello".into(),
        }
    );

    let legacy = recv_frame(&mut acp_rx).await?;
    assert_eq!(
        legacy["method"],
        serde_json::json!(crate::ACP_SESSION_UPDATE_METHOD)
    );
    recv_none(&mut native_rx, 50).await;
    recv_none(&mut acp_rx, 50).await;
    Ok(())
}

#[tokio::test]
async fn native_and_acp_receive_single_terminal_turn_completed() -> Result<()> {
    let data_root = TempDir::new()?;
    let dual = DualConn::subscribed(data_root.path(), 16).await?;
    let (native_seq_before, acp_seq_before) = {
        let connections = dual.runtime.connections.lock().await;
        (
            connections
                .get(&dual.native_id)
                .expect("Native connection")
                .next_event_seq,
            connections
                .get(&dual.acp_id)
                .expect("ACP connection")
                .next_event_seq,
        )
    };

    let native_turn = |sequence, status| devo_protocol::native::turn::Turn {
        id: NativeTurnId::new(),
        session_id: dual.session_id,
        sequence,
        status,
        kind: devo_protocol::native::turn::TurnKind::Regular,
        model: devo_protocol::native::model::ModelBinding {
            provider: "unknown".to_string(),
            model: "test-model".to_string(),
            variant: None,
            reasoning_effort: None,
        },
        collaboration_mode: None,
        started_at: Utc::now(),
        completed_at: Some(Utc::now()),
        usage: None,
        error: None,
    };
    let mut failed_turn = native_turn(1, devo_protocol::native::turn::TurnStatus::Failed);
    failed_turn.error = Some(devo_protocol::native::error::AgentError::new(
        "PROVIDER_SERVER_ERROR",
        "provider failed",
    ));
    let interrupted_turn = native_turn(2, devo_protocol::native::turn::TurnStatus::Interrupted);
    let cases = vec![
        (failed_turn, "failed", Some("provider failed")),
        (interrupted_turn, "interrupted", None),
    ];

    let mut native_rx = dual.native_rx;
    let mut acp_rx = dual.acp_rx;
    for (turn, expected_status, expected_error) in cases {
        dual.runtime
            .broadcast_notification(ServerNotification::TurnCompleted {
                turn: Box::new(turn.clone()),
            })
            .await;

        let native = recv_frame(&mut native_rx).await?;
        assert_eq!(native["method"], serde_json::json!("turn/completed"));
        assert_eq!(
            native["params"]["turn"]["id"],
            serde_json::json!(turn.id.to_string())
        );
        assert_eq!(
            native["params"]["turn"]["status"],
            serde_json::json!(expected_status)
        );
        assert_eq!(
            native["params"]["turn"]["error"]["message"].as_str(),
            expected_error
        );
        recv_none(&mut native_rx, 50).await;

        let notification = recv_frame(&mut acp_rx).await?;
        assert_eq!(
            notification["method"],
            serde_json::json!(crate::ACP_SESSION_UPDATE_METHOD)
        );
        let notification = serde_json::from_value::<devo_protocol::AcpSessionNotification>(
            notification["params"].clone(),
        )
        .expect("decode ACP session notification");
        let (method, _params) = devo_protocol::original_notification_wire_from_acp(&notification)
            .expect("ACP terminal notification preserves original method");
        assert_eq!(method, "turn/completed");
    }

    let connections = dual.runtime.connections.lock().await;
    assert_eq!(
        (
            connections
                .get(&dual.native_id)
                .expect("Native connection")
                .next_event_seq
                - native_seq_before,
            connections
                .get(&dual.acp_id)
                .expect("ACP connection")
                .next_event_seq
                - acp_seq_before,
        ),
        (2, 2),
        "one identity terminal per case on default Native; one ACP update per case"
    );
    Ok(())
}

#[tokio::test]
async fn native_and_acp_projection_preferences() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let session_id = SessionId::new();

    let (typed_out, _typed_rx) = super::outbound::test_outbound_channel(1);
    let (plain_out, _plain_rx) = super::outbound::test_outbound_channel(1);
    let typed_id = runtime
        .register_connection(ClientTransportKind::Stdio, typed_out)
        .await;
    let plain_id = runtime
        .register_connection(ClientTransportKind::Stdio, plain_out)
        .await;
    for (id, meta) in [
        (
            typed_id,
            serde_json::json!({ "devo": { "typedItems": true } }),
        ),
        (plain_id, serde_json::json!({})),
    ] {
        let mut params = serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": { "terminal": false },
        });
        if !meta.as_object().unwrap().is_empty() {
            params["_meta"] = meta;
        }
        let response = runtime
            .handle_acp_initialize(id, Some(serde_json::json!(id)), params)
            .await;
        if id == typed_id {
            assert_eq!(
                response["result"]["_meta"]["devo"]["typedItems"],
                serde_json::json!(true)
            );
        } else {
            assert!(response["result"]["_meta"].get("devo").is_none());
        }
    }
    let connections = runtime.connections.lock().await;
    assert!(connections.get(&typed_id).expect("typed").typed_items);
    assert!(!connections.get(&plain_id).expect("plain").typed_items);
    drop(connections);

    let (native_out, mut native_rx) = super::outbound::test_outbound_channel(1);
    let (acp_out, mut acp_rx) = super::outbound::test_outbound_channel(1);
    let native_id = runtime
        .register_connection(ClientTransportKind::Stdio, native_out)
        .await;
    let acp_id = runtime
        .register_connection(ClientTransportKind::Stdio, acp_out)
        .await;
    runtime
        .subscribe_connection_to_session(native_id, session_id, None)
        .await;
    runtime
        .subscribe_connection_to_session(acp_id, session_id, None)
        .await;
    {
        let mut connections = runtime.connections.lock().await;
        connections.get_mut(&native_id).expect("native").protocol =
            Some(ConnectionProtocol::Native);
        connections.get_mut(&native_id).expect("native").typed_items = true;
        connections.get_mut(&acp_id).expect("acp").protocol = Some(ConnectionProtocol::Acp);
        connections.get_mut(&acp_id).expect("acp").typed_items = true;
    }
    runtime
        .broadcast_notification(item_delta_notification(
            ItemDeltaKind::AgentMessageDelta,
            session_id,
            NativeItemId::new(),
            4,
            "hello",
        ))
        .await;
    let native = recv_frame(&mut native_rx).await?;
    assert_eq!(
        native["method"],
        serde_json::json!("item/assistantMessage/delta")
    );
    let delta: devo_protocol::native::event::ItemDelta =
        serde_json::from_value(native["params"].clone())?;
    assert_eq!(
        (delta.chunk_index, delta.base_revision, delta.delta.as_str()),
        (4, 1, "hello")
    );
    let acp = recv_frame(&mut acp_rx).await?;
    assert_eq!(
        acp["method"],
        serde_json::json!(crate::ACP_SESSION_UPDATE_METHOD)
    );

    let (outbound, mut receiver) = super::outbound::test_outbound_channel(1);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    runtime
        .subscribe_connection_to_session(connection_id, session_id, None)
        .await;
    runtime
        .connections
        .lock()
        .await
        .get_mut(&connection_id)
        .expect("conn")
        .protocol = Some(ConnectionProtocol::Acp);
    runtime
        .connections
        .lock()
        .await
        .get_mut(&connection_id)
        .expect("conn")
        .typed_items = true;
    runtime
        .broadcast_notification(ServerNotification::ItemStarted {
            item: Box::new(typed_item_envelope(
                // The envelope's session must match the subscribed session —
                // should_deliver session-filters notifications, and a fresh
                // random id here meant the frame never reached the receiver.
                session_id,
                NativeTurnId::new(),
                NativeItemId::new(),
                1,
                &devo_protocol::native::item::Item::ToolCall {
                    call_id: "call-1".into(),
                    tool_name: "read".into(),
                    source: devo_protocol::native::item::ToolSource::Builtin,
                    server_name: None,
                    input: Some(serde_json::json!({ "bogus": true })),
                },
                ItemState::Running,
                Utc::now(),
                None,
            )),
        })
        .await;
    let notification = recv_frame(&mut receiver).await?;
    assert_eq!(
        notification["method"],
        serde_json::json!(crate::ACP_SESSION_UPDATE_METHOD)
    );

    let (legacy_out, mut legacy_rx) = super::outbound::test_outbound_channel(8);
    let legacy_id = runtime
        .register_connection(ClientTransportKind::Stdio, legacy_out)
        .await;
    runtime
        .handle_acp_initialize(
            legacy_id,
            Some(serde_json::json!(1)),
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "terminal": false },
                "_meta": {
                    "devo": {
                        "protocol": "native",
                        "sessionEventStream": true
                    }
                },
            }),
        )
        .await;
    runtime
        .subscribe_connection_to_session(legacy_id, session_id, None)
        .await;
    runtime
        .broadcast_notification(ServerNotification::TurnCompleted {
            turn: Box::new(devo_protocol::native::turn::Turn {
                id: NativeTurnId::new(),
                session_id,
                sequence: 1,
                status: devo_protocol::native::turn::TurnStatus::Completed,
                kind: devo_protocol::native::turn::TurnKind::Regular,
                model: devo_protocol::native::model::ModelBinding {
                    provider: "unknown".into(),
                    model: "test-model".into(),
                    variant: None,
                    reasoning_effort: None,
                },
                collaboration_mode: None,
                started_at: Utc::now(),
                completed_at: Some(Utc::now()),
                usage: None,
                error: None,
            }),
        })
        .await;
    assert_eq!(
        recv_frame(&mut legacy_rx).await?["method"],
        serde_json::json!("turn/completed")
    );
    recv_none(&mut legacy_rx, 50).await;
    Ok(())
}

async fn write_history_rollout(data_root: &std::path::Path) -> SessionId {
    use devo_core::{TextItem, TurnItem};

    let rollout_store = crate::persistence::RolloutStore::new(data_root.to_path_buf(), None);
    let record = rollout_store.create_session_record(
        devo_core::SessionId::new(),
        Utc::now(),
        data_root.to_path_buf(),
        Vec::new(),
        Some("history session".into()),
        Some("test-model".into()),
        None,
        None,
        "test-provider".into(),
        None,
    );
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");
    let mut item_seq = 1u64;
    for turn_index in 1..=3u32 {
        let turn_id = devo_core::TurnId::new();
        let turn = devo_core::TurnRecord {
            id: turn_id,
            session_id: record.id,
            sequence: turn_index,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            status: TurnStatus::Completed,
            kind: devo_core::TurnKind::Regular,
            model: "test-model".into(),
            model_binding_id: None,
            reasoning_effort_selection: None,
            request_model: "test-model".into(),
            request_thinking: None,
            input_token_estimate: None,
            usage: None,
            latest_query_usage: None,
            context_occupancy: None,
            stop_reason: None,
            failure_reason: None,
            error: None,
            session_context: None,
            turn_context: None,
            schema_version: 4,
        };
        rollout_store
            .append_turn(&record, turn)
            .expect("append turn");
        // Turns 1 and 2 get two items each, turn 3 gets one.
        for text in match turn_index {
            1 | 2 => vec!["first", "second"],
            _ => vec!["third"],
        } {
            let item = crate::persistence::build_item_record(
                record.id,
                turn_id,
                devo_core::ItemId::new(),
                item_seq,
                TurnItem::AgentMessage(TextItem::text(format!("{text}-t{turn_index}"))),
                Some(TurnStatus::Running),
                None,
                None,
            );
            rollout_store
                .append_item(&record, item)
                .expect("append item");
            item_seq += 1;
        }
    }
    record.id
}

async fn initialized_connection(runtime: &Arc<ServerRuntime>) -> u64 {
    initialized_with_protocol_meta(runtime, true).await
}

async fn initialized_with_protocol_meta(runtime: &Arc<ServerRuntime>, native: bool) -> u64 {
    connect_stdio(runtime, native, 16).await.0
}

#[tokio::test]
async fn initialize_protocol_matrix_and_routing() {
    let init_params = |native: bool| {
        let mut params = serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": { "terminal": false },
        });
        if native {
            params["_meta"] = serde_json::json!({ "devo": { "protocol": "native" } });
        }
        params
    };

    let native_runtime = build_runtime_with_provider_catalog_and_protocols(
        TempDir::new().expect("native temp").path(),
        Arc::new(NoopProvider::failing()),
        Arc::new(PresetModelCatalog::default()),
        ProtocolSet::only(ServerProtocol::Native),
    );
    let (outbound, _rx) = super::outbound::test_outbound_channel(4);
    let acp_id = native_runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    let rejected = native_runtime
        .handle_acp_initialize(acp_id, Some(serde_json::json!(1)), init_params(false))
        .await;
    assert_eq!(rejected["error"]["code"], serde_json::json!(-32600));
    assert_eq!(native_runtime.connection_protocol(acp_id).await, None);
    assert!(!native_runtime.connection_ready(acp_id).await);
    let native_id = initialized_with_protocol_meta(&native_runtime, true).await;
    assert_eq!(
        native_runtime.connection_protocol(native_id).await,
        Some(ConnectionProtocol::Native)
    );

    let acp_runtime = build_runtime_with_provider_catalog_and_protocols(
        TempDir::new().expect("ACP temp").path(),
        Arc::new(NoopProvider::failing()),
        Arc::new(PresetModelCatalog::default()),
        ProtocolSet::only(ServerProtocol::Acp),
    );
    let acp_id = initialized_with_protocol_meta(&acp_runtime, false).await;
    assert_eq!(
        acp_runtime.connection_protocol(acp_id).await,
        Some(ConnectionProtocol::Acp)
    );
    let (outbound, _rx) = super::outbound::test_outbound_channel(4);
    let native_id = acp_runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    let rejected = acp_runtime
        .handle_acp_initialize(native_id, Some(serde_json::json!(2)), init_params(true))
        .await;
    assert_eq!(rejected["error"]["code"], serde_json::json!(-32600));
    assert_eq!(acp_runtime.connection_protocol(native_id).await, None);

    let temp = TempDir::new().expect("temp");
    let runtime = build_runtime_with_provider_catalog_and_protocols(
        temp.path(),
        Arc::new(NoopProvider::failing()),
        Arc::new(PresetModelCatalog::default()),
        ProtocolSet::only(ServerProtocol::Native),
    );
    let (outbound, _rx) = super::outbound::test_outbound_channel(4);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    let params = init_params(false);
    let rejected = runtime
        .handle_acp_initialize(connection_id, Some(serde_json::json!(1)), params.clone())
        .await;
    assert_eq!(rejected["error"]["code"], serde_json::json!(-32600));
    assert_eq!(
        runtime
            .enable_protocols(&ProtocolSet::only(ServerProtocol::Acp))
            .await,
        ProtocolSet::all()
    );
    let accepted = runtime
        .handle_acp_initialize(connection_id, Some(serde_json::json!(2)), params)
        .await;
    assert!(
        accepted.get("result").is_some(),
        "initialize failed: {accepted}"
    );
    assert_eq!(
        runtime.connection_protocol(connection_id).await,
        Some(ConnectionProtocol::Acp)
    );

    let runtime = build_runtime(temp.path());
    let connection_id = initialized_with_protocol_meta(&runtime, true).await;
    let response = runtime
        .handle_acp_initialize(
            connection_id,
            Some(serde_json::json!(2)),
            init_params(false),
        )
        .await;
    assert_eq!(response["error"]["code"], serde_json::json!(-32600));
    assert_eq!(
        runtime.connection_protocol(connection_id).await,
        Some(ConnectionProtocol::Native)
    );

    let connection_id = initialized_with_protocol_meta(&runtime, false).await;
    let response = history_request(
        &runtime,
        connection_id,
        9,
        "runtime/ping",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(response["error"]["code"], serde_json::json!(-32601));

    let temp = TempDir::new().expect("temp dir");
    let runtime = build_runtime(temp.path());
    let owner_id = initialized_connection(&runtime).await;
    let subscriber_id = initialized_connection(&runtime).await;
    let unrelated_id = initialized_connection(&runtime).await;
    let session_id = SessionId::new();
    {
        let mut connections = runtime.connections.lock().await;
        connections
            .get_mut(&subscriber_id)
            .expect("subscriber connection")
            .event_selectors =
            vec![devo_protocol::native::event::StreamSelector::Session { session_id }];
    }
    assert_eq!(
        runtime
            .controlling_connection_ids(session_id, Some(owner_id))
            .await,
        vec![owner_id, subscriber_id]
    );
    assert!(
        !runtime
            .controlling_connection_ids(session_id, Some(owner_id))
            .await
            .contains(&unrelated_id)
    );
}

async fn history_request(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({ "id": id, "method": method, "params": params }),
        )
        .await
        .expect("history response")
}

async fn conn_page<T: serde::de::DeserializeOwned>(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> T {
    json_result(
        &history_request(runtime, connection_id, id, method, params).await,
        "rpc",
    )
}

#[tokio::test]
async fn turns_and_items_list_paginate_without_gaps_or_duplicates() -> Result<()> {
    use devo_protocol::native::item::Item;
    use devo_protocol::native::page::Page;
    use devo_protocol::native::turn::Turn;

    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let session_id = write_history_rollout(data_root.path()).await;
    let connection_id = initialized_connection(&runtime).await;

    let first: Page<Turn> = conn_page(
        &runtime,
        connection_id,
        1,
        "session/turns/list",
        serde_json::json!({ "sessionId": session_id.to_string(), "limit": 2 }),
    )
    .await;
    assert_eq!(
        (
            first.data.len(),
            first
                .data
                .iter()
                .map(|turn| turn.sequence)
                .collect::<Vec<_>>(),
            first.next_cursor.as_deref(),
        ),
        (2, vec![1, 2], Some("2"))
    );
    let turn = &first.data[0];
    assert_eq!(
        (turn.session_id.as_str(), turn.status, turn.kind,),
        (
            session_id.as_str(),
            devo_protocol::native::turn::TurnStatus::Completed,
            devo_protocol::native::turn::TurnKind::Regular,
        )
    );

    let second: Page<Turn> = conn_page(
        &runtime,
        connection_id,
        2,
        "session/turns/list",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "limit": 2,
            "cursor": first.next_cursor.expect("cursor"),
        }),
    )
    .await;
    assert_eq!(
        (
            second.data.len(),
            second.data[0].sequence,
            second.next_cursor
        ),
        (1, 3, None)
    );

    let first_items: Page<devo_protocol::native::item::ItemEnvelope> = conn_page(
        &runtime,
        connection_id,
        3,
        "session/items/list",
        serde_json::json!({ "sessionId": session_id.to_string(), "limit": 3 }),
    )
    .await;
    assert_eq!(
        (
            first_items
                .data
                .iter()
                .map(|item| item.seq)
                .collect::<Vec<_>>(),
            first_items.next_cursor.as_deref(),
        ),
        (vec![1, 2, 3], Some("3"))
    );
    let envelope = &first_items.data[0];
    assert_eq!(
        (
            envelope.session_id.as_str(),
            envelope.revision,
            envelope.state
        ),
        (
            session_id.as_str(),
            1u32,
            devo_protocol::native::item::ItemState::Completed,
        )
    );
    assert!(matches!(
        &envelope.item,
        Item::AssistantMessage { text } if text == "first-t1"
    ));

    let second_items: Page<devo_protocol::native::item::ItemEnvelope> = conn_page(
        &runtime,
        connection_id,
        4,
        "session/items/list",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "limit": 3,
            "cursor": first_items.next_cursor.expect("cursor"),
        }),
    )
    .await;
    assert_eq!(
        (
            second_items
                .data
                .iter()
                .map(|item| item.seq)
                .collect::<Vec<_>>(),
            second_items.next_cursor,
        ),
        (vec![4, 5], None)
    );

    let turns: Page<Turn> = conn_page(
        &runtime,
        connection_id,
        5,
        "session/turns/list",
        serde_json::json!({ "sessionId": session_id.to_string() }),
    )
    .await;
    let turn_two = &turns.data[1];
    let filtered: Page<devo_protocol::native::item::ItemEnvelope> = conn_page(
        &runtime,
        connection_id,
        6,
        "session/items/list",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "turnId": turn_two.id.as_str(),
        }),
    )
    .await;
    assert_eq!(
        (
            filtered.data.len(),
            filtered.data.iter().all(|item| item.turn_id == turn_two.id)
        ),
        (2, true)
    );

    drop(runtime);
    let cold_runtime = build_runtime(data_root.path());
    let cold_connection = initialized_connection(&cold_runtime).await;
    let cold_items: Page<devo_protocol::native::item::ItemEnvelope> = conn_page(
        &cold_runtime,
        cold_connection,
        7,
        "session/items/list",
        serde_json::json!({ "sessionId": session_id.to_string() }),
    )
    .await;
    assert_eq!((cold_items.data.len(), cold_items.next_cursor), (5, None));

    let rollout_store = crate::persistence::RolloutStore::new(data_root.path().to_path_buf(), None);
    let empty_record = rollout_store.create_session_record(
        devo_core::SessionId::new(),
        Utc::now(),
        data_root.path().to_path_buf(),
        Vec::new(),
        None,
        Some("test-model".into()),
        None,
        None,
        "test-provider".into(),
        None,
    );
    rollout_store
        .append_session_meta(&empty_record)
        .expect("append session meta");
    assert_eq!(
        conn_page::<Page<Turn>>(
            &cold_runtime,
            cold_connection,
            8,
            "session/turns/list",
            serde_json::json!({ "sessionId": empty_record.id.to_string() }),
        )
        .await,
        Page {
            data: Vec::new(),
            next_cursor: None,
        }
    );
    for (id, method, params) in [
        (
            9,
            "session/items/list",
            serde_json::json!({ "sessionId": empty_record.id.to_string(), "cursor": "not-a-cursor" }),
        ),
        (
            10,
            "session/turns/list",
            serde_json::json!({ "sessionId": SessionId::new().to_string() }),
        ),
    ] {
        assert!(
            history_request(&cold_runtime, cold_connection, id, method, params)
                .await
                .get("error")
                .is_some()
        );
    }
    Ok(())
}

/// Journal of a rewound session: turn 1 kept, turn 2 abandoned (dead branch),
/// turn 3 the live branch. Every tree-visible item gets an explicit edge so
/// the reader does not infer a linear chain, and the leaf points at turn 3's
/// last item.
async fn write_branched_history_rollout(data_root: &std::path::Path) -> SessionId {
    use devo_core::{TextItem, TurnItem};

    let rollout_store = crate::persistence::RolloutStore::new(data_root.to_path_buf(), None);
    let record = rollout_store.create_session_record(
        devo_core::SessionId::new(),
        Utc::now(),
        data_root.to_path_buf(),
        Vec::new(),
        Some("branched history session".into()),
        Some("test-model".into()),
        None,
        None,
        "test-provider".into(),
        None,
    );
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");
    // (turn_index, is_live_branch, texts) — turn 2 writes the dead branch.
    let item_ids: Vec<devo_core::ItemId> = (0..6).map(|_| devo_core::ItemId::new()).collect();
    let mut seq = 1u64;
    for turn_index in 1..=3u32 {
        let turn_id = devo_core::TurnId::new();
        let turn = devo_core::TurnRecord {
            id: turn_id,
            session_id: record.id,
            sequence: turn_index,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            status: TurnStatus::Completed,
            kind: devo_core::TurnKind::Regular,
            model: "test-model".into(),
            model_binding_id: None,
            reasoning_effort_selection: None,
            request_model: "test-model".into(),
            request_thinking: None,
            input_token_estimate: None,
            usage: None,
            latest_query_usage: None,
            context_occupancy: None,
            stop_reason: None,
            failure_reason: None,
            error: None,
            session_context: None,
            turn_context: None,
            schema_version: 4,
        };
        rollout_store
            .append_turn(&record, turn)
            .expect("append turn");
        for text_idx in 0..2usize {
            let item_id = item_ids[(turn_index as usize - 1) * 2 + text_idx];
            let item = crate::persistence::build_item_record(
                record.id,
                turn_id,
                item_id,
                seq,
                TurnItem::AgentMessage(TextItem::text(format!("t{turn_index}-i{text_idx}"))),
                Some(TurnStatus::Running),
                None,
                None,
            );
            rollout_store
                .append_item(&record, item)
                .expect("append item");
            seq += 1;
        }
    }
    // Tree: t1 = ids[0..2]; dead branch t2 = ids[2..4] under ids[1]; live
    // branch t3 = ids[4..6] ALSO under ids[1] (rewind to turn 1's reply).
    // Leaf = ids[5] (turn 3's last item).
    let parents: [(usize, Option<usize>); 6] = [
        (0, None),
        (1, Some(0)),
        (2, Some(1)),
        (3, Some(2)),
        (4, Some(1)),
        (5, Some(4)),
    ];
    for (child, parent) in parents {
        rollout_store
            .append_tree_edge_at(
                &record.rollout_path,
                record.id,
                item_ids[child],
                parent.map(|index| item_ids[index]),
            )
            .expect("append tree edge");
    }
    rollout_store
        .append_session_leaf_at(&record.rollout_path, record.id, Some(item_ids[5]), 1)
        .expect("append session leaf");
    record.id
}

#[tokio::test]
async fn turns_and_items_list_scope_to_the_active_path() -> Result<()> {
    use devo_protocol::native::page::Page;
    use devo_protocol::native::turn::Turn;

    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let session_id = write_branched_history_rollout(data_root.path()).await;
    let connection_id = initialized_connection(&runtime).await;

    // The journal holds 6 items / 3 turns linearly; the active path keeps
    // turns 1 and 3 (seqs 1,2,5,6) and hides the abandoned turn 2.
    let items: Page<devo_protocol::native::item::ItemEnvelope> = conn_page(
        &runtime,
        connection_id,
        1,
        "session/items/list",
        serde_json::json!({ "sessionId": session_id.to_string() }),
    )
    .await;
    assert_eq!(
        (
            items.data.iter().map(|item| item.seq).collect::<Vec<_>>(),
            items.next_cursor,
        ),
        (vec![1, 2, 5, 6], None)
    );
    assert!(items.data.iter().all(|item| matches!(&item.item,
            devo_protocol::native::item::Item::AssistantMessage { text }
                if !text.contains("t2-"))));

    let turns: Page<Turn> = conn_page(
        &runtime,
        connection_id,
        2,
        "session/turns/list",
        serde_json::json!({ "sessionId": session_id.to_string() }),
    )
    .await;
    assert_eq!(
        (
            turns
                .data
                .iter()
                .map(|turn| turn.sequence)
                .collect::<Vec<_>>(),
            turns.next_cursor,
        ),
        (vec![1, 3], None)
    );

    // The abandoned turn is dead history: turns/list no longer exposes it,
    // and an explicit turnId query into it returns nothing (the scope is the
    // current conversation, not the whole journal). Recover the id from the
    // rollout file directly to prove it.
    let history = devo_core::read_canonical_history(&record_rollout_path(&runtime, &session_id))?;
    let dead_turn_id = history
        .turns
        .iter()
        .find(|turn| turn.sequence == 2)
        .expect("dead turn in journal")
        .id;
    let dead_items: Page<devo_protocol::native::item::ItemEnvelope> = conn_page(
        &runtime,
        connection_id,
        4,
        "session/items/list",
        serde_json::json!({ "sessionId": session_id.to_string(), "turnId": dead_turn_id.as_str() }),
    )
    .await;
    assert_eq!(dead_items.data.len(), 0);

    Ok(())
}

fn record_rollout_path(runtime: &Arc<ServerRuntime>, session_id: &SessionId) -> std::path::PathBuf {
    runtime.rollout_store.allocate_rollout_path(session_id)
}

async fn write_subscribed_rollout(runtime: &Arc<ServerRuntime>) -> SessionId {
    use devo_core::{TextItem, TurnItem};

    let store = &runtime.rollout_store;
    let record = store.create_session_record(
        devo_core::SessionId::new(),
        Utc::now(),
        std::path::PathBuf::from("/tmp/subscription-test"),
        Vec::new(),
        Some("subscribed session".into()),
        Some("test-model".into()),
        None,
        None,
        "test-provider".into(),
        None,
    );
    store.append_session_meta(&record).expect("append meta");
    let turn_id = devo_core::TurnId::new();
    let turn = devo_core::TurnRecord {
        id: turn_id,
        session_id: record.id,
        sequence: 1,
        started_at: Utc::now(),
        completed_at: Some(Utc::now()),
        status: TurnStatus::Completed,
        kind: devo_core::TurnKind::Regular,
        model: "test-model".into(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        request_model: "test-model".into(),
        request_thinking: None,
        input_token_estimate: None,
        usage: None,
        latest_query_usage: None,
        context_occupancy: None,
        stop_reason: None,
        failure_reason: None,
        error: None,
        session_context: None,
        turn_context: None,
        schema_version: 4,
    };
    store.append_turn(&record, turn).expect("append turn");
    for (seq, text) in [(1u64, "one"), (2, "two")] {
        let item = crate::persistence::build_item_record(
            record.id,
            turn_id,
            devo_core::ItemId::new(),
            seq,
            TurnItem::AgentMessage(TextItem::text(text)),
            Some(TurnStatus::Running),
            None,
            None,
        );
        store.append_item(&record, item).expect("append item");
    }
    record.id
}

async fn write_abandoned_in_progress_rollout(runtime: &Arc<ServerRuntime>) -> SessionId {
    let store = &runtime.rollout_store;
    let record = store.create_session_record(
        devo_core::SessionId::new(),
        Utc::now(),
        std::path::PathBuf::from("/tmp/abandoned-in-progress"),
        Vec::new(),
        Some("abandoned session".into()),
        Some("test-model".into()),
        None,
        None,
        "test-provider".into(),
        None,
    );
    store.append_session_meta(&record).expect("append meta");
    let turn_id = devo_core::TurnId::new();
    let turn = devo_core::TurnRecord {
        id: turn_id,
        session_id: record.id,
        sequence: 1,
        started_at: Utc::now(),
        completed_at: None,
        status: TurnStatus::Running,
        kind: devo_core::TurnKind::Regular,
        model: "test-model".into(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        request_model: "test-model".into(),
        request_thinking: None,
        input_token_estimate: None,
        usage: None,
        latest_query_usage: None,
        context_occupancy: None,
        stop_reason: None,
        failure_reason: None,
        error: None,
        session_context: None,
        turn_context: None,
        schema_version: 4,
    };
    store.append_turn(&record, turn).expect("append turn");
    runtime
        .rollout_store
        .index_rollout_metadata(&runtime.deps.db)
        .expect("index rollout");
    record.id
}

#[tokio::test]
async fn subscription_snapshot_ignores_abandoned_in_progress_turn() -> Result<()> {
    use devo_protocol::native::event::{SnapshotData, SubscriptionCreateResult};
    use devo_protocol::native::rpc_session::SessionResumeResult;
    use devo_protocol::native::session::SessionStatus;

    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let session_id = write_abandoned_in_progress_rollout(&runtime).await;
    let connection_id = initialized_connection(&runtime).await;

    let resumed: SessionResumeResult = conn_page(
        &runtime,
        connection_id,
        1,
        "session/resume",
        serde_json::json!({ "sessionId": session_id.to_string() }),
    )
    .await;
    assert_eq!(resumed.session.status, SessionStatus::Idle);
    assert!(resumed.session.active_turn_id.is_none());
    assert!(
        resumed.recovery.is_some(),
        "abandoned InProgress should expose turn recovery after hydrate"
    );
    assert!(runtime.runtime_active_turn_id(session_id).await.is_none());

    let with_snapshot: SubscriptionCreateResult = conn_page(
        &runtime,
        connection_id,
        2,
        "subscription/create",
        serde_json::json!({
            "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
            "includeSnapshot": true,
        }),
    )
    .await;
    let SnapshotData::Session {
        session,
        active_turn,
        ..
    } = &with_snapshot.snapshots[0].data
    else {
        panic!("expected session snapshot");
    };
    assert_eq!(session.status, SessionStatus::Idle);
    assert!(session.active_turn_id.is_none());
    assert_eq!(active_turn, &None);

    Ok(())
}

#[tokio::test]
async fn subscription_create_replay_live_ack_and_unsubscribe() -> Result<()> {
    use devo_protocol::native::event::{SnapshotData, SubscriptionCreateResult};

    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let session_id = write_subscribed_rollout(&runtime).await;
    let stream_id = devo_core::session_stream_id(
        &devo_protocol::native::ids::SessionId::from_string(session_id.to_string()),
    );
    let connection_id = initialized_connection(&runtime).await;

    let future_create = history_request(
        &runtime,
        connection_id,
        0,
        "subscription/create",
        serde_json::json!({
            "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
            "includeSnapshot": false,
            "after": [{ "streamId": stream_id, "seq": 999 }],
        }),
    )
    .await;
    assert_eq!(
        future_create["error"]["code"],
        serde_json::json!("CursorExpired")
    );

    let with_snapshot: SubscriptionCreateResult = conn_page(
        &runtime,
        connection_id,
        1,
        "subscription/create",
        serde_json::json!({
            "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
            "includeSnapshot": true,
        }),
    )
    .await;
    assert_eq!(with_snapshot.cursors[0].seq, 4);
    assert_eq!(
        with_snapshot
            .replay
            .iter()
            .map(|event| event.meta.seq.expect("hydrated seq"))
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    let SnapshotData::Session {
        session,
        active_turn,
        queue,
    } = &with_snapshot.snapshots[0].data
    else {
        panic!("expected session snapshot");
    };
    assert_eq!(session.id.as_str(), session_id.to_string());
    assert_eq!(active_turn, &None);
    assert!(queue.is_empty());

    let (live_id, mut receiver) = connect_stdio(&runtime, true, 4).await;
    assert!(
        history_request(
            &runtime,
            live_id,
            2,
            "subscription/create",
            serde_json::json!({
                "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
                "includeSnapshot": false,
            }),
        )
        .await
        .get("error")
        .is_none()
    );
    for text in ["live", "second"] {
        runtime
            .broadcast_notification(ServerNotification::ItemCompleted {
                item: Box::new(typed_item_envelope(
                    session_id,
                    NativeTurnId::new(),
                    NativeItemId::new(),
                    5,
                    &devo_protocol::native::item::Item::AssistantMessage { text: text.into() },
                    ItemState::Completed,
                    Utc::now(),
                    None,
                )),
            })
            .await;
        assert_eq!(
            recv_frame(&mut receiver).await?["params"]["item"]["item"]["text"],
            serde_json::json!(text)
        );
    }
    let (_other_id, mut other_rx) = connect_stdio(&runtime, true, 4).await;
    runtime
        .broadcast_notification(ServerNotification::ItemCompleted {
            item: Box::new(typed_item_envelope(
                session_id,
                NativeTurnId::new(),
                NativeItemId::new(),
                7,
                &devo_protocol::native::item::Item::AssistantMessage {
                    text: "ignored".into(),
                },
                ItemState::Completed,
                Utc::now(),
                None,
            )),
        })
        .await;
    recv_none(&mut other_rx, 100).await;

    let created: SubscriptionCreateResult = conn_page(
        &runtime,
        connection_id,
        3,
        "subscription/create",
        serde_json::json!({
            "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
            "includeSnapshot": false,
        }),
    )
    .await;
    let subscription_id = created.subscription_id.as_str().to_owned();
    for (id, seq, code) in [(4, 99, "CursorExpired"), (5, 2, "CursorExpired")] {
        let response = history_request(
            &runtime,
            connection_id,
            id,
            "subscription/ack",
            serde_json::json!({
                "subscriptionId": subscription_id,
                "cursors": [{ "streamId": stream_id, "seq": seq }],
            }),
        )
        .await;
        assert_eq!(response["error"]["code"], serde_json::json!(code));
    }
    assert!(
        history_request(
            &runtime,
            connection_id,
            6,
            "subscription/ack",
            serde_json::json!({
                "subscriptionId": subscription_id,
                "cursors": [{ "streamId": stream_id, "seq": 4 }],
            }),
        )
        .await
        .get("error")
        .is_none()
    );
    assert!(
        history_request(
            &runtime,
            connection_id,
            7,
            "subscription/unsubscribe",
            serde_json::json!({ "subscriptionId": subscription_id }),
        )
        .await
        .get("error")
        .is_none()
    );
    assert!(
        history_request(
            &runtime,
            connection_id,
            8,
            "subscription/unsubscribe",
            serde_json::json!({ "subscriptionId": subscription_id }),
        )
        .await
        .get("error")
        .is_some()
    );
    Ok(())
}
#[tokio::test]
async fn second_subscription_keeps_connection_session_selectors() -> Result<()> {
    use devo_protocol::native::event::StreamSelector;

    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let session_id = write_subscribed_rollout(&runtime).await;
    let connection_id = initialized_connection(&runtime).await;

    // The TUI's main subscription targets the session...
    assert!(
        history_request(
            &runtime,
            connection_id,
            0,
            "subscription/create",
            serde_json::json!({
                "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
                "includeSnapshot": false,
            }),
        )
        .await
        .get("error")
        .is_none()
    );
    // ...and the agents-view roster later adds a cwd-wide subscription on the
    // same connection. The connection's cached selector view must stay the
    // UNION: replacing it with the new subscription's selectors drops the
    // session selector, after which `controlling_connection_ids` stops
    // routing permission requests to the TUI ("no client connection is
    // available for permission request").
    assert!(
        history_request(
            &runtime,
            connection_id,
            1,
            "subscription/create",
            serde_json::json!({
                "selectors": [{ "kind": "sessionsByCwd", "cwd": data_root.path() }],
                "includeSnapshot": false,
            }),
        )
        .await
        .get("error")
        .is_none()
    );

    let selectors = runtime
        .connections
        .lock()
        .await
        .get(&connection_id)
        .expect("connection")
        .event_selectors
        .clone();
    // The union is built from a registry HashMap: assert membership, not order.
    let mut selectors = selectors;
    selectors.sort_by_key(|selector| match selector {
        StreamSelector::SessionsByCwd { .. } => 0,
        StreamSelector::Session { .. } => 1,
        StreamSelector::BackgroundTask { .. } => 2,
    });
    assert_eq!(
        selectors,
        vec![
            StreamSelector::SessionsByCwd {
                cwd: data_root.path().to_path_buf(),
            },
            StreamSelector::Session {
                session_id: SessionId::from_string(session_id.to_string()),
            },
        ]
    );
    assert_eq!(
        runtime
            .controlling_connection_ids(session_id, /*owner_connection_id*/ None)
            .await,
        vec![connection_id],
        "session-subscribed connection must stay a controlling connection after a second subscription"
    );

    Ok(())
}

/// A subscribed proxy-transport connection (second TUI on the internal proxy
/// websocket) must stream assistant deltas for turns driven by ANOTHER
/// connection — suppressing them leaves the other client's transcript without
/// the reply (user messages arrive via item lifecycle, replies only via
/// deltas).
#[tokio::test]
async fn proxy_connection_receives_assistant_deltas_for_other_connections_turns() -> Result<()> {
    use devo_protocol::native::event::ItemDelta;

    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let session_id = write_subscribed_rollout(&runtime).await;
    let owner = initialized_connection(&runtime).await;

    let (proxy_outbound, mut proxy_rx) = super::outbound::test_outbound_channel(16);
    let proxy = runtime
        .register_connection(ClientTransportKind::StdioProxy, proxy_outbound)
        .await;
    let initialize_response = runtime
        .handle_incoming(
            proxy,
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "_meta": { "devo": { "protocol": "native" } },
                    "clientInfo": { "name": "proxy-client", "version": "1.0.0" }
                }
            }),
        )
        .await;
    assert!(initialize_response.is_some());
    for connection_id in [owner, proxy] {
        assert!(
            history_request(
                &runtime,
                connection_id,
                2,
                "subscription/create",
                serde_json::json!({
                    "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
                    "includeSnapshot": false,
                }),
            )
            .await
            .get("error")
            .is_none()
        );
    }

    // The owner connection drives an in-flight turn for the session.
    runtime
        .active_turns
        .register_turn(
            session_id,
            devo_protocol::native::turn::Turn {
                id: NativeTurnId::new(),
                session_id: NativeSessionId::from_string(session_id.to_string()),
                sequence: 1,
                kind: devo_protocol::native::turn::TurnKind::Regular,
                status: devo_protocol::native::turn::TurnStatus::InProgress,
                model: devo_protocol::native::model::ModelBinding {
                    provider: "test-provider".to_string(),
                    model: "test-model".to_string(),
                    variant: None,
                    reasoning_effort: None,
                },
                collaboration_mode: None,
                started_at: Utc::now(),
                completed_at: None,
                error: None,
                usage: None,
            },
        )
        .await;
    runtime
        .active_turns
        .set_connection_id(session_id, owner)
        .await;

    runtime
        .broadcast_notification(ServerNotification::ItemAssistantMessageDelta(ItemDelta {
            item_id: NativeItemId::new(),
            session_id: NativeSessionId::from_string(session_id.to_string()),
            base_revision: 1,
            chunk_index: 0,
            delta: "streamed reply".to_string(),
        }))
        .await;

    let received = tokio::time::timeout(Duration::from_secs(2), proxy_rx.recv())
        .await
        .expect("proxy connection receives a notification")
        .expect("proxy outbound channel open");
    assert_eq!(
        received.get("method"),
        Some(&serde_json::json!("item/assistantMessage/delta")),
        "subscribed proxy connections must stream other connections' assistant deltas: {received}"
    );

    Ok(())
}

#[tokio::test]
async fn legacy_implicit_filters_prune_to_live_session_subscriptions() -> Result<()> {
    // Regression (live r33): session/resume pushed a permanent implicit
    // delivery filter and nothing ever removed it — after the client forked
    // and switched sessions, it kept receiving (and rendering) the previous
    // session's entire item stream. A live session-kind subscription must
    // prune implicit filters for sessions it does not cover.
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let session_a = write_subscribed_rollout(&runtime).await;
    let session_b = write_subscribed_rollout(&runtime).await;
    runtime
        .rollout_store
        .index_rollout_metadata(&runtime.deps.db)
        .expect("index rollouts");
    let (connection_id, mut rx) = connect_stdio(&runtime, true, 16).await;

    // Boot on session A: implicit filter for A alone.
    conn_page::<serde_json::Value>(
        &runtime,
        connection_id,
        1,
        "session/resume",
        serde_json::json!({ "sessionId": session_a.to_string() }),
    )
    .await;

    let item_id = NativeItemId::new();
    let delta = |session_id| {
        crate::item_delta_notification(
            ItemDeltaKind::AgentMessageDelta,
            session_id,
            item_id,
            0,
            "hello",
        )
    };

    // Pre-switch baseline: the implicit filter delivers A's items.
    runtime.broadcast_notification(delta(session_a)).await;
    let frame = recv_frame(&mut rx).await?;
    assert_eq!(
        frame.get("method"),
        Some(&serde_json::json!("item/assistantMessage/delta"))
    );

    // Switch to B the way the TUI does: new-style subscription for B only.
    conn_page::<serde_json::Value>(
        &runtime,
        connection_id,
        2,
        "subscription/create",
        serde_json::json!({
            "selectors": [{ "kind": "session", "sessionId": session_b.to_string() }],
            "includeSnapshot": false,
        }),
    )
    .await;

    // A's stream must stop arriving; B's must arrive.
    runtime.broadcast_notification(delta(session_a)).await;
    recv_none(&mut rx, 150).await;
    runtime.broadcast_notification(delta(session_b)).await;
    let frame = recv_frame(&mut rx).await?;
    assert_eq!(
        frame.get("method"),
        Some(&serde_json::json!("item/assistantMessage/delta"))
    );

    Ok(())
}

#[tokio::test]
async fn live_context_occupancy_skips_zero_anchor() -> Result<()> {
    // Regression (live r25b protocol trace): every provider leg starts with a
    // message_start whose usage is all zeros. That placeholder was scaled to
    // the `.max(1)` floor and broadcast as a 1-token occupancy, so the client
    // context bar collapsed to 0% at the start of every leg — repeatedly
    // during multi-leg spawn/await turns. A zero anchor is not a measurement;
    // the last known occupancy must stand.
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let session_id = write_subscribed_rollout(&runtime).await;
    runtime
        .rollout_store
        .index_rollout_metadata(&runtime.deps.db)
        .expect("index rollouts");
    let (connection_id, mut rx) = connect_stdio(&runtime, true, 16).await;
    conn_page::<serde_json::Value>(
        &runtime,
        connection_id,
        1,
        "session/resume",
        serde_json::json!({ "sessionId": session_id.to_string() }),
    )
    .await;
    // Drain the resume's trailing notifications before the silence check.
    recv_none(&mut rx, 150).await;

    let raw = || devo_core::RawContextBreakdown {
        base: 100,
        skills: 0,
        tools_builtin: 0,
        tools_mcp: 0,
        conversation: 400,
    };

    // Leg-start placeholder: an all-zero usage anchor must not broadcast.
    runtime
        .publish_live_context_occupancy(session_id, None, raw(), 0)
        .await;
    recv_none(&mut rx, 150).await;

    // A real anchor broadcasts exactly the provider-anchored total.
    runtime
        .publish_live_context_occupancy(session_id, None, raw(), 500)
        .await;
    let frame = recv_frame(&mut rx).await?;
    assert_eq!(
        frame.get("method"),
        Some(&serde_json::json!("context/usageUpdated"))
    );
    let occupancy = frame
        .pointer("/params/occupancy")
        .cloned()
        .unwrap_or_default();
    assert_eq!(occupancy["totalTokens"].as_u64(), Some(500));

    Ok(())
}

#[tokio::test]
async fn subscription_snapshot_queue_sources() -> Result<()> {
    use devo_protocol::native::event::{SnapshotData, SubscriptionCreateResult};
    use devo_protocol::native::rpc_turn::SessionQueuePushResult;

    let data_root = TempDir::new()?;
    let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let runtime = build_runtime_with_provider(
        data_root.path(),
        Arc::new(GatedProvider {
            open: Arc::clone(&open),
            started: Default::default(),
        }),
    );
    let connection_id = initialized_connection(&runtime).await;
    let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
    let started: SessionQueuePushResult = json_result(
        &history_request(
            &runtime,
            connection_id,
            2,
            "session/queue/push",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "first" }],
                "idempotencyKey": "push-1",
            }),
        )
        .await,
        "rpc",
    );
    assert!(matches!(started, SessionQueuePushResult::Started { .. }));
    for (request, key, text) in [(3, "push-2", "second"), (4, "push-3", "third")] {
        let pushed: SessionQueuePushResult = json_result(
            &history_request(
                &runtime,
                connection_id,
                request,
                "session/queue/push",
                serde_json::json!({
                    "sessionId": session_id.to_string(),
                    "input": [{ "type": "text", "text": text }],
                    "idempotencyKey": key,
                }),
            )
            .await,
            "rpc",
        );
        assert!(matches!(pushed, SessionQueuePushResult::Queued { .. }));
    }
    let result: SubscriptionCreateResult = json_result(
        &history_request(
            &runtime,
            connection_id,
            5,
            "subscription/create",
            serde_json::json!({
                "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
                "includeSnapshot": true,
            }),
        )
        .await,
        "rpc",
    );
    let SnapshotData::Session { queue, .. } = &result.snapshots[0].data else {
        panic!("expected session snapshot");
    };
    assert_eq!(queue.len(), 2);
    assert_eq!(queue[0].preview, "second");
    open.store(true, std::sync::atomic::Ordering::SeqCst);

    let runtime = build_runtime(data_root.path());
    let session_id = write_subscribed_rollout(&runtime).await;
    let now = Utc::now();
    runtime.deps.db.upsert_session(
        crate::db::SessionIndexRow {
            session_id,
            cwd: std::path::PathBuf::from("/tmp/subscription-test"),
            additional_directories: Vec::new(),
            created_at: now,
            updated_at: now,
            last_activity_at: now,
            title: Some("subscribed session".into()),
            title_state: devo_protocol::SessionTitleState::Generating,
            parent_session_id: None,
            fork_from_id: None,
            fork_at_turn_id: None,
            agent_path: None,
            ephemeral: false,
            model: Some("test-model".into()),
            reasoning_effort_selection: None,
        },
        None,
    )?;
    for text in ["first queued", "second queued"] {
        runtime.deps.db.push_pending(
            &session_id,
            crate::db::QueueType::Turn,
            &devo_core::PendingInputItem::new(
                devo_core::PendingInputKind::UserText { text: text.into() },
                None,
                chrono::Utc::now(),
            ),
        )?;
    }
    let result: SubscriptionCreateResult = json_result(
        &history_request(
            &runtime,
            initialized_connection(&runtime).await,
            6,
            "subscription/create",
            serde_json::json!({
                "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
                "includeSnapshot": true,
            }),
        )
        .await,
        "rpc",
    );
    let SnapshotData::Session { queue, .. } = &result.snapshots[0].data else {
        panic!("expected session snapshot");
    };
    assert_eq!(
        queue
            .iter()
            .map(|entry| entry.preview.as_str())
            .collect::<Vec<_>>(),
        vec!["first queued", "second queued"]
    );
    Ok(())
}

struct GatedProvider {
    open: Arc<std::sync::atomic::AtomicBool>,
    started: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl ModelProviderSDK for GatedProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("gated provider does not support completion")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.started
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let open = Arc::clone(&self.open);
        // Tick like a real provider stream so the session actor keeps
        // servicing its mailbox (a perfectly silent stream would stall
        // every mailbox round-trip the way no production stream can).
        Ok(Box::pin(futures::stream::unfold(false, move |done| {
            let open = Arc::clone(&open);
            async move {
                if done {
                    return None;
                }
                let gate_open = async {
                    while !open.load(std::sync::atomic::Ordering::SeqCst) {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                };
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                        Some((
                            Ok(StreamEvent::TextDelta {
                                index: 0,
                                text: "tick".into(),
                            }),
                            false,
                        ))
                    }
                    _ = gate_open => {
                        Some((
                            Ok(StreamEvent::MessageDone {
                                response: ModelResponse {
                                    id: "gated-response".into(),
                                    content: vec![devo_protocol::ResponseContent::Text(
                                        "done".into(),
                                    )],
                                    stop_reason: Some(devo_protocol::StopReason::EndTurn),
                                    usage: devo_protocol::Usage::default(),
                                    metadata: devo_protocol::ResponseMetadata::default(),
                                },
                            }),
                            true,
                        ))
                    }
                }
            }
        })))
    }

    fn name(&self) -> &str {
        "gated-provider"
    }
}

async fn start_durable_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    data_root: &std::path::Path,
) -> Result<SessionId> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 100,
                "method": "session/new",
                "params": {
                    "cwd": data_root,
                    "idempotencyKey": format!("test-session-{}", Uuid::new_v4())
                }
            }),
        )
        .await
        .expect("session/new response");
    let native_session = serde_json::from_value::<
        crate::SuccessResponse<devo_protocol::native::rpc_session::SessionNewResult>,
    >(response.clone())
    .map_err(|error| anyhow::anyhow!("session/new response {response}: {error}"))?
    .result
    .session;
    Ok(SessionId::from(native_session.id.as_str()))
}

async fn start_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
    text: &str,
) -> Result<TurnId> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 101,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": text }],
                    "idempotencyKey": format!("test-turn-{}", Uuid::new_v4())
                }
            }),
        )
        .await
        .expect("turn/start response");
    let result: crate::SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult> =
        serde_json::from_value(response.clone())
            .map_err(|error| anyhow::anyhow!("turn/start response {response}: {error}"))?;
    Ok(TurnId::from(result.result.turn.id.as_str()))
}

#[tokio::test]
async fn rollback_preview_commit_restores_git_and_is_idempotent() -> Result<()> {
    use devo_protocol::native::rpc_session::{RestorePlan, SessionRollbackCommitResult};

    let data_root = TempDir::new()?;
    let repo = TempDir::new()?;
    let normalize_line_endings = |s: String| s.replace("\r\n", "\n");
    init_git_repo(repo.path());

    let runtime = build_runtime(data_root.path());
    let connection_id = initialized_connection(&runtime).await;
    let session_id = start_durable_session(&runtime, connection_id, repo.path()).await?;
    start_turn(&runtime, connection_id, session_id, "first").await?;
    wait_turn_idle(&runtime, session_id).await;
    assert!(runtime.runtime_active_turn_id(session_id).await.is_none());

    std::fs::write(repo.path().join("tracked.txt"), "before second\n")?;
    start_turn(&runtime, connection_id, session_id, "second").await?;
    wait_turn_idle(&runtime, session_id).await;
    assert!(runtime.runtime_active_turn_id(session_id).await.is_none());
    std::fs::write(repo.path().join("tracked.txt"), "after second\n")?;
    std::fs::write(repo.path().join("new.txt"), "new\n")?;
    let turns_before_preview = session_turns_json(&runtime, connection_id, session_id).await;

    let preview = history_request(
        &runtime,
        connection_id,
        200,
        "session/rollback/preview",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "userTurnIndex": 1,
            "mode": "beforeUserTurn",
        }),
    )
    .await;
    let plan: RestorePlan = json_result(&preview, "rpc");
    assert_eq!(
        plan.affected_files,
        vec![PathBuf::from("new.txt"), PathBuf::from("tracked.txt")]
    );
    assert_eq!(plan.dropped_turn_count, 1);
    assert_eq!(
        session_turns_json(&runtime, connection_id, session_id).await,
        turns_before_preview
    );
    let turn_ids_before: HashSet<String> = turns_before_preview
        .iter()
        .filter_map(|turn| turn["id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(turn_ids_before.len(), 2);
    let index_before = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["show", ":tracked.txt"])
        .output()?;
    assert!(index_before.status.success());

    let commit_params = serde_json::json!({
        "restorePlanId": plan.restore_plan_id.as_str(),
        "expectedWorkspaceVersion": plan.workspace_version,
    });
    let other_connection_id = initialized_connection(&runtime).await;
    let wrong_connection = history_request(
        &runtime,
        other_connection_id,
        201,
        "session/rollback/commit",
        commit_params.clone(),
    )
    .await;
    assert_eq!(
        wrong_connection["error"]["code"],
        serde_json::json!("RESTORE_PLAN_NOT_FOUND")
    );
    std::fs::write(repo.path().join("drift.txt"), "drift\n")?;
    let conflicted = history_request(
        &runtime,
        connection_id,
        202,
        "session/rollback/commit",
        commit_params.clone(),
    )
    .await;
    assert_eq!(
        conflicted["error"]["code"],
        serde_json::json!("WORKSPACE_VERSION_CONFLICT")
    );
    std::fs::remove_file(repo.path().join("drift.txt"))?;
    let title_update = history_request(
        &runtime,
        connection_id,
        206,
        "session/metadata/update",
        serde_json::json!({
            "sessionId": session_id,
            "expectedVersion": 0,
            "title": "Preserved rollback title",
        }),
    )
    .await;
    assert!(title_update.get("error").is_none(), "{title_update}");
    let queued_input = devo_protocol::PendingInputItem::new(
        devo_protocol::PendingInputKind::UserText {
            text: "preserve queued input".to_string(),
        },
        None,
        Utc::now(),
    );
    let queued_input_id = queued_input.id;
    runtime
        .session_turn_reservation_snapshot(session_id)
        .await
        .expect("turn reservation")
        .pending_turn_queue
        .lock()
        .expect("pending queue")
        .push_back(queued_input);
    let (committed, concurrent_retry) = tokio::join!(
        history_request(
            &runtime,
            connection_id,
            203,
            "session/rollback/commit",
            commit_params.clone(),
        ),
        history_request(
            &runtime,
            connection_id,
            204,
            "session/rollback/commit",
            commit_params.clone(),
        )
    );
    assert_eq!(concurrent_retry["result"], committed["result"]);
    let result: SessionRollbackCommitResult = json_result(&committed, "rpc");
    assert_eq!(
        result,
        SessionRollbackCommitResult {
            restored_turn_count: 1,
            restored_file_count: 2,
        }
    );
    assert_eq!(
        normalize_line_endings(std::fs::read_to_string(repo.path().join("tracked.txt"))?),
        "before second\n"
    );
    assert!(!repo.path().join("new.txt").exists());
    assert_eq!(
        std::process::Command::new("git")
            .current_dir(repo.path())
            .args(["show", ":tracked.txt"])
            .output()?
            .stdout,
        index_before.stdout
    );
    let turn_ids_after: HashSet<String> = session_turns_json(&runtime, connection_id, session_id)
        .await
        .iter()
        .filter_map(|turn| turn["id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(turn_ids_after.len(), 1);
    assert!(turn_ids_after.is_subset(&turn_ids_before));
    assert_eq!(
        runtime
            .session(session_id)
            .await
            .expect("session")
            .summary()
            .await
            .expect("summary")
            .title,
        Some("Preserved rollback title".to_string())
    );
    let pending_after = runtime
        .session_turn_reservation_snapshot(session_id)
        .await
        .expect("turn reservation")
        .pending_turn_queue
        .lock()
        .expect("pending queue")
        .front()
        .map(|item| item.id);
    assert_eq!(pending_after, Some(queued_input_id));

    let retried = history_request(
        &runtime,
        connection_id,
        205,
        "session/rollback/commit",
        commit_params.clone(),
    )
    .await;
    assert_eq!(retried["result"], committed["result"]);

    let session_handle = runtime.session(session_id).await.expect("session");
    let state_change_guard = session_handle.lock_state_change().await;
    let first = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let params = commit_params.clone();
        async move {
            runtime
                .handle_session_rollback_commit(connection_id, serde_json::json!(351), params)
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let second = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let params = commit_params.clone();
        async move {
            runtime
                .handle_session_rollback_commit(connection_id, serde_json::json!(352), params)
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    runtime.unregister_connection(connection_id).await;
    drop(state_change_guard);
    let first_response = tokio::time::timeout(Duration::from_secs(5), first).await??;
    let second_response = tokio::time::timeout(Duration::from_secs(5), second).await??;
    assert!(first_response.get("error").is_none(), "{first_response}");
    // A concurrent duplicate commit coalesces on the in-flight attempt
    // (RestorePlanStatus::InFlight) and replays its completed result:
    // duplicate commits are idempotent, not plan-not-found errors.
    assert!(second_response.get("error").is_none(), "{second_response}");
    assert_eq!(second_response["result"], first_response["result"]);
    Ok(())
}

#[tokio::test]
async fn state_change_gate_blocks_turn_start_and_compaction() -> Result<()> {
    let env = NativeEnv::connect().await?;
    let session_id = env.session().await?;
    let session_handle = env.runtime.session(session_id).await.expect("session");
    let state_change_guard = session_handle.lock_state_change().await;
    let runtime_for_turn = Arc::clone(&env.runtime);
    let connection_id = env.connection_id;
    let turn_start = tokio::spawn(async move {
        runtime_for_turn
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 300,
                    "method": "turn/start",
                    "params": {
                        "sessionId": session_id,
                        "input": [{ "type": "text", "text": "wait" }],
                        "idempotencyKey": "turn-start-state-change-gate"
                    }
                }),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!turn_start.is_finished());
    drop(state_change_guard);
    let response = turn_start.await?.expect("turn/start response");
    assert!(response.get("error").is_none(), "turn/start: {response}");

    let session_handle = env.runtime.session(session_id).await.expect("session");
    let state_change_guard = session_handle.lock_state_change().await;
    let now = Utc::now();
    let summary = session_handle.summary().await.expect("summary");
    let turn = crate::turn::RuntimeTurn {
        native: devo_protocol::native::turn::Turn {
            id: NativeTurnId::new(),
            session_id: summary.native.id,
            sequence: 1,
            kind: devo_protocol::native::turn::TurnKind::Compaction,
            status: devo_protocol::native::turn::TurnStatus::InProgress,
            model: devo_protocol::native::model::ModelBinding {
                provider: "unknown".into(),
                model: "test-model".to_string(),
                variant: None,
                reasoning_effort: None,
            },
            collaboration_mode: None,
            started_at: now,
            completed_at: None,
            error: None,
            usage: None,
        },
        extras: crate::turn::RuntimeTurnExtras {
            request_thinking: None,
            stop_reason: None,
            failure_reason: None,
        },
    };
    let compaction = tokio::spawn(Arc::clone(&env.runtime).run_session_compaction(
        session_id,
        session_handle,
        turn,
        crate::runtime::handlers::compaction::CompactionRunOptions::default(),
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!compaction.is_finished());
    drop(state_change_guard);
    tokio::time::timeout(Duration::from_secs(5), compaction).await??;
    Ok(())
}

async fn start_hanging_compaction(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
    request_id: u64,
) -> Result<devo_protocol::native::ids::TurnId> {
    start_turn(runtime, connection_id, session_id, "seed history").await?;
    wait_turn_idle(runtime, session_id).await;
    let compact_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "session/compact/start",
                "params": { "sessionId": session_id }
            }),
        )
        .await
        .expect("session/compact response");
    let compact_result: devo_protocol::native::rpc_turn::TurnStartResult = serde_json::from_value(
        compact_response
            .get("result")
            .cloned()
            .expect("compact result"),
    )?;
    let turn_id = compact_result.turn.id;
    for _ in 0..50 {
        if runtime.runtime_active_turn_id(session_id).await == Some(turn_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        runtime.runtime_active_turn_id(session_id).await,
        Some(turn_id)
    );
    Ok(turn_id)
}

struct TurnOkCompactHangProvider;

#[async_trait]
impl ModelProviderSDK for TurnOkCompactHangProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        std::future::pending::<()>().await;
        unreachable!("hanging compaction completion should be canceled")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(futures::stream::iter(vec![Ok(
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "turn-ok".into(),
                    content: vec![devo_protocol::ResponseContent::Text("ok".into())],
                    stop_reason: Some(devo_protocol::StopReason::EndTurn),
                    usage: devo_protocol::Usage::default(),
                    metadata: devo_protocol::ResponseMetadata::default(),
                },
            },
        )])))
    }

    fn name(&self) -> &str {
        "turn-ok-compact-hang"
    }
}

/// Completion (compaction summarizer) blocks until `open`; turn streams
/// resolve immediately. Lets a test queue a follow-up while a compaction is
/// genuinely mid-model-call, then release it to succeed.
struct GatedCompletionProvider {
    open: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl ModelProviderSDK for GatedCompletionProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        let open = Arc::clone(&self.open);
        tokio::select! {
            () = async {
                while !open.load(std::sync::atomic::Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            } => Ok(ModelResponse {
                id: "compaction-summary".into(),
                content: vec![devo_protocol::ResponseContent::Text(
                    "<compaction_summary>compacted</compaction_summary>".into(),
                )],
                stop_reason: Some(devo_protocol::StopReason::EndTurn),
                usage: devo_protocol::Usage::default(),
                metadata: devo_protocol::ResponseMetadata::default(),
            }),
        }
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(futures::stream::iter(vec![Ok(
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "turn-ok".into(),
                    content: vec![devo_protocol::ResponseContent::Text("ok".into())],
                    stop_reason: Some(devo_protocol::StopReason::EndTurn),
                    usage: devo_protocol::Usage::default(),
                    metadata: devo_protocol::ResponseMetadata::default(),
                },
            },
        )])))
    }

    fn name(&self) -> &str {
        "gated-completion"
    }
}

#[tokio::test]
async fn manual_compaction_success_drains_queued_followup() -> Result<()> {
    // A follow-up queued while a manual compaction runs must start once the
    // compaction SUCCEEDS. The replaced-history branch finalizes inline (not
    // via finalize_manual_compaction_turn), so it needs its own queue drain —
    // otherwise the message strands and its presence blocks every later turn.
    let data_root = TempDir::new()?;
    let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let runtime = build_runtime_with_provider(
        data_root.path(),
        Arc::new(GatedCompletionProvider {
            open: Arc::clone(&open),
        }),
    );
    let connection_id = initialized_connection(&runtime).await;
    let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
    // Two turns: the first is compacted, the second becomes the preserved
    // suffix (one turn alone is entirely preserved and compaction skips).
    for text in ["first turn", "second turn"] {
        start_turn(&runtime, connection_id, session_id, text).await?;
        wait_turn_idle(&runtime, session_id).await;
    }

    let compact = history_request(
        &runtime,
        connection_id,
        200,
        "session/compact/start",
        serde_json::json!({ "sessionId": session_id }),
    )
    .await;
    assert!(compact.get("error").is_none(), "compact: {compact}");

    let push = history_request(
        &runtime,
        connection_id,
        201,
        "session/queue/push",
        serde_json::json!({
            "sessionId": session_id,
            "input": [{ "type": "text", "text": "queued while compacting" }],
            "idempotencyKey": "push-compact-success",
        }),
    )
    .await;
    let result: devo_protocol::native::rpc_turn::SessionQueuePushResult =
        json_result(&push, "push");
    assert!(matches!(
        result,
        devo_protocol::native::rpc_turn::SessionQueuePushResult::Queued { .. }
    ));

    open.store(true, std::sync::atomic::Ordering::SeqCst);

    let session_handle = runtime.session(session_id).await.expect("session");
    let mut drained = false;
    for _ in 0..300 {
        let pending_empty = session_handle
            .pending_queue_snapshot()
            .await
            .is_none_or(|snapshot| snapshot.pending_count == 0);
        let followup_completed = session_turns_json(&runtime, connection_id, session_id)
            .await
            .iter()
            .any(|turn| {
                turn["kind"] == serde_json::json!("regular")
                    && turn["status"] == serde_json::json!("completed")
                    && turn["sequence"].as_u64() == Some(4)
            });
        if pending_empty && followup_completed {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        drained,
        "queued follow-up must run to completion after a successful compaction"
    );
    Ok(())
}

#[tokio::test]
async fn manual_compaction_interrupt_orphan_and_queue_push() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime =
        build_runtime_with_provider(data_root.path(), Arc::new(TurnOkCompactHangProvider));
    let connection_id = initialized_connection(&runtime).await;
    let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
    let _turn_id = start_hanging_compaction(&runtime, connection_id, session_id, 200).await?;

    let session_handle = runtime.session(session_id).await.expect("session");
    let gate = tokio::time::timeout(
        Duration::from_millis(100),
        session_handle.lock_state_change(),
    )
    .await
    .expect("compaction model call must not hold state_change_gate");
    drop(gate);
    let push = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        async move {
            runtime
                .handle_incoming(
                    connection_id,
                    serde_json::json!({
                        "id": 300,
                        "method": "session/queue/push",
                        "params": {
                            "sessionId": session_id.to_string(),
                            "input": [{ "type": "text", "text": "queued while compacting" }],
                            "idempotencyKey": "push-wedge-2"
                        }
                    }),
                )
                .await
        }
    });
    let response = tokio::time::timeout(Duration::from_secs(5), push)
        .await
        .context("busy push must respond immediately during compaction")??
        .expect("push response");
    let result: devo_protocol::native::rpc_turn::SessionQueuePushResult =
        json_result(&response, "rpc");
    assert!(matches!(
        result,
        devo_protocol::native::rpc_turn::SessionQueuePushResult::Queued { .. }
    ));

    assert!(
        runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 201,
                    "method": "session/interrupt",
                    "params": {
                        "scope": { "scope": "session", "sessionId": session_id }
                    }
                }),
            )
            .await
            .expect("session/interrupt response")
            .get("error")
            .is_none()
    );
    wait_turn_idle(&runtime, session_id).await;
    assert!(runtime.runtime_active_turn_id(session_id).await.is_none());
    // The queued follow-up pushed above must be admitted after the interrupt:
    // a manual compaction turn never runs the turn_exec post-turn pipeline,
    // so finalize_manual_compaction_turn has to drain the pending queue once
    // the session is idle again — otherwise the message strands and its
    // presence makes TryBeginActiveTurn reject every later turn/compaction.
    // The queued turn itself runs in milliseconds, so watch the queue drain
    // (and a newer regular turn in the reservation) rather than the transient
    // active-turn registry.
    let session_handle = runtime.session(session_id).await.expect("session");
    let compaction_turn_id = _turn_id;
    let mut queued_turn_admitted = false;
    for _ in 0..200 {
        let pending_empty = session_handle
            .pending_queue_snapshot()
            .await
            .is_none_or(|snapshot| snapshot.pending_count == 0);
        let newer_turn = session_handle
            .turn_reservation_snapshot()
            .await
            .and_then(|snapshot| snapshot.latest_turn)
            .is_some_and(|turn| {
                turn.native.id != compaction_turn_id
                    && turn.native.kind == devo_protocol::native::turn::TurnKind::Regular
            });
        if pending_empty && newer_turn {
            queued_turn_admitted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        queued_turn_admitted,
        "queued follow-up must be admitted after compaction interrupt"
    );
    wait_turn_idle(&runtime, session_id).await;

    assert!(
        runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 203,
                    "method": "session/compact/start",
                    "params": { "sessionId": session_id }
                }),
            )
            .await
            .expect("second session/compact response")
            .get("error")
            .is_none()
    );
    // The second compaction hangs in the model call; cancel it before the
    // orphan-recovery scenario below needs the session idle again.
    assert!(
        runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 204,
                    "method": "session/interrupt",
                    "params": {
                        "scope": { "scope": "session", "sessionId": session_id }
                    }
                }),
            )
            .await
            .expect("second session/interrupt response")
            .get("error")
            .is_none()
    );
    wait_turn_idle(&runtime, session_id).await;

    let turn_id = start_hanging_compaction(&runtime, connection_id, session_id, 210).await?;
    let session_handle = runtime.session(session_id).await.expect("session");
    // Simulate an orphaned manual compaction: the task hangs in the model
    // call while the session still books it as the active turn (recovery
    // reads `active_turn`, so clearing it first would defeat the scenario).
    let recovered = runtime
        .recover_orphaned_manual_compaction_interrupt(&session_handle, session_id, turn_id)
        .await
        .expect("orphaned compaction should recover");
    assert_eq!(recovered.status, TurnStatus::Interrupted);
    assert!(runtime.runtime_active_turn_id(session_id).await.is_none());
    let gate = tokio::time::timeout(Duration::from_secs(5), session_handle.lock_state_change())
        .await
        .context("timed out waiting for compaction to release state_change_gate")?;
    drop(gate);
    assert!(
        runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 211,
                    "method": "session/compact/start",
                    "params": { "sessionId": session_id }
                }),
            )
            .await
            .expect("third session/compact response")
            .get("error")
            .is_none()
    );
    Ok(())
}

struct TitleCompletionStreamGatedProvider {
    stream_open: Arc<std::sync::atomic::AtomicBool>,
    stream_started: Arc<std::sync::atomic::AtomicBool>,
    completion_open: Arc<std::sync::atomic::AtomicBool>,
    completion_requested: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl ModelProviderSDK for TitleCompletionStreamGatedProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        self.completion_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        while !self
            .completion_open
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(ModelResponse {
            id: "title-response".into(),
            content: vec![devo_protocol::ResponseContent::Text(
                "Generated session title".into(),
            )],
            stop_reason: Some(devo_protocol::StopReason::EndTurn),
            usage: devo_protocol::Usage::default(),
            metadata: devo_protocol::ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.stream_started
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let open = Arc::clone(&self.stream_open);
        Ok(Box::pin(futures::stream::unfold(false, move |done| {
            let open = Arc::clone(&open);
            async move {
                if done {
                    return None;
                }
                let gate_open = async {
                    while !open.load(std::sync::atomic::Ordering::SeqCst) {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                };
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                        Some((
                            Ok(StreamEvent::TextDelta {
                                index: 0,
                                text: "tick".into(),
                            }),
                            false,
                        ))
                    }
                    _ = gate_open => {
                        Some((
                            Ok(StreamEvent::MessageDone {
                                response: ModelResponse {
                                    id: "gated-response".into(),
                                    content: vec![devo_protocol::ResponseContent::Text(
                                        "done".into(),
                                    )],
                                    stop_reason: Some(devo_protocol::StopReason::EndTurn),
                                    usage: devo_protocol::Usage::default(),
                                    metadata: devo_protocol::ResponseMetadata::default(),
                                },
                            }),
                            true,
                        ))
                    }
                }
            }
        })))
    }

    fn name(&self) -> &str {
        "title-completion-stream-gated"
    }
}

async fn title_gated_runtime() -> Result<(
    Arc<ServerRuntime>,
    u64,
    SessionId,
    Arc<std::sync::atomic::AtomicBool>,
    Arc<std::sync::atomic::AtomicBool>,
)> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(TitleCompletionStreamGatedProvider {
        stream_open: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        stream_started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        completion_open: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        completion_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    let stream_open = Arc::clone(&provider.stream_open);
    let stream_started = Arc::clone(&provider.stream_started);
    let runtime = build_runtime_with_provider(data_root.path(), provider);
    let connection_id = initialized_connection(&runtime).await;
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 100,
                "method": "session/new",
                "params": {
                    "cwd": data_root.path(),
                    "idempotencyKey": format!("title-gate-{}", Uuid::new_v4())
                }
            }),
        )
        .await
        .expect("session/new response");
    let session_id = serde_json::from_value::<
        crate::SuccessResponse<devo_protocol::native::rpc_session::SessionNewResult>,
    >(response)?
    .result
    .session
    .id;
    start_turn(&runtime, connection_id, session_id, "first prompt").await?;
    for _ in 0..500 {
        if stream_started.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(stream_started.load(std::sync::atomic::Ordering::SeqCst));
    Ok((
        runtime,
        connection_id,
        session_id,
        stream_open,
        stream_started,
    ))
}

#[tokio::test]
async fn title_generation_gate_keeps_admission_responsive() -> Result<()> {
    let (runtime, connection_id, session_id, stream_open, _stream_started) =
        title_gated_runtime().await?;

    let push = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        async move {
            runtime
                .handle_incoming(
                    connection_id,
                    serde_json::json!({
                        "id": 300,
                        "method": "session/queue/push",
                        "params": {
                            "sessionId": session_id.to_string(),
                            "input": [{ "type": "text", "text": "second prompt" }],
                            "idempotencyKey": "push-wedge-1"
                        }
                    }),
                )
                .await
        }
    });
    let response = tokio::time::timeout(Duration::from_secs(5), push)
        .await
        .context("busy push must respond immediately during an active turn")??
        .expect("push response");
    let result: devo_protocol::native::rpc_turn::SessionQueuePushResult =
        json_result(&response, "rpc");
    assert!(matches!(
        result,
        devo_protocol::native::rpc_turn::SessionQueuePushResult::Queued { .. }
    ));

    let turn_start = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        async move {
            runtime
                .handle_incoming(
                    connection_id,
                    serde_json::json!({
                        "id": 301,
                        "method": "turn/start",
                        "params": {
                            "sessionId": session_id.to_string(),
                            "input": [{ "type": "text", "text": "second prompt" }],
                            "idempotencyKey": "native-wedge-1"
                        }
                    }),
                )
                .await
        }
    });
    let response = tokio::time::timeout(Duration::from_secs(2), turn_start)
        .await
        .context("native turn/start must respond during an active turn")??
        .expect("turn/start response");
    assert_eq!(
        response["error"]["code"].as_str(),
        Some("TurnAlreadyRunning")
    );

    let metadata_update = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        async move {
            runtime
                .handle_incoming(
                    connection_id,
                    serde_json::json!({
                        "id": 302,
                        "method": "session/metadata/update",
                        "params": {
                            "sessionId": session_id.to_string(),
                            "expectedVersion": 0,
                            "model": { "provider": "", "model": "test-model" }
                        }
                    }),
                )
                .await
        }
    });
    let response = tokio::time::timeout(Duration::from_secs(2), metadata_update)
        .await
        .context("session/metadata/update must respond during an active turn")??
        .expect("metadata update response");
    assert!(response.get("error").is_none());

    stream_open.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

#[tokio::test]
async fn rollback_in_non_git_workspace_is_history_only() -> Result<()> {
    use devo_protocol::native::rpc_session::{RestorePlan, SessionRollbackCommitResult};

    let data_root = TempDir::new()?;
    let workspace = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let connection_id = initialized_connection(&runtime).await;
    let session_id = start_durable_session(&runtime, connection_id, workspace.path()).await?;
    for text in ["first", "second"] {
        start_turn(&runtime, connection_id, session_id, text).await?;
        wait_turn_idle(&runtime, session_id).await;
    }

    let preview = history_request(
        &runtime,
        connection_id,
        400,
        "session/rollback/preview",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "userTurnIndex": 1,
            "mode": "beforeUserTurn",
        }),
    )
    .await;
    let plan: RestorePlan = json_result(&preview, "rpc");
    assert_eq!(plan.affected_files, Vec::<PathBuf>::new());
    assert_eq!(plan.workspace_version, "history-only");

    start_turn(&runtime, connection_id, session_id, "third").await?;
    wait_turn_idle(&runtime, session_id).await;
    assert!(runtime.runtime_active_turn_id(session_id).await.is_none());
    let stale_commit = history_request(
        &runtime,
        connection_id,
        401,
        "session/rollback/commit",
        serde_json::json!({
            "restorePlanId": plan.restore_plan_id.as_str(),
            "expectedWorkspaceVersion": plan.workspace_version,
        }),
    )
    .await;
    assert_eq!(
        stale_commit["error"]["code"],
        serde_json::json!("WORKSPACE_VERSION_CONFLICT")
    );

    let preview = history_request(
        &runtime,
        connection_id,
        402,
        "session/rollback/preview",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "userTurnIndex": 1,
            "mode": "beforeUserTurn",
        }),
    )
    .await;
    let plan: RestorePlan = json_result(&preview, "rpc");
    let committed = history_request(
        &runtime,
        connection_id,
        403,
        "session/rollback/commit",
        serde_json::json!({
            "restorePlanId": plan.restore_plan_id.as_str(),
            "expectedWorkspaceVersion": plan.workspace_version,
        }),
    )
    .await;
    assert_eq!(
        serde_json::from_value::<SessionRollbackCommitResult>(committed["result"].clone())?,
        SessionRollbackCommitResult {
            restored_turn_count: 2,
            restored_file_count: 0,
        }
    );
    let turn_ids: HashSet<String> = session_turns_json(&runtime, connection_id, session_id)
        .await
        .iter()
        .filter_map(|turn| turn["id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(turn_ids.len(), 1);

    let disconnecting_connection_id = initialized_connection(&runtime).await;
    let disconnect_preview = history_request(
        &runtime,
        disconnecting_connection_id,
        404,
        "session/rollback/preview",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "userTurnIndex": 0,
            "mode": "beforeUserTurn",
        }),
    )
    .await;
    let disconnect_plan: RestorePlan =
        serde_json::from_value(disconnect_preview["result"].clone())?;
    runtime
        .unregister_connection(disconnecting_connection_id)
        .await;
    let disconnected_commit = runtime
        .handle_session_rollback_commit(
            disconnecting_connection_id,
            serde_json::json!(405),
            serde_json::json!({
                "restorePlanId": disconnect_plan.restore_plan_id.as_str(),
                "expectedWorkspaceVersion": disconnect_plan.workspace_version,
            }),
        )
        .await;
    assert_eq!(
        disconnected_commit["error"]["code"],
        serde_json::json!("RESTORE_PLAN_NOT_FOUND")
    );
    Ok(())
}

async fn queue_list(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
) -> Vec<devo_protocol::native::queue::QueueEntry> {
    let response = history_request(
        runtime,
        connection_id,
        1,
        "session/queue/list",
        serde_json::json!({ "sessionId": session_id.to_string() }),
    )
    .await;
    let result: devo_protocol::native::rpc_turn::SessionQueueListResult =
        json_result(&response, "rpc");
    result.entries
}

async fn session_turns_json(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
) -> Vec<serde_json::Value> {
    let response = history_request(
        runtime,
        connection_id,
        90,
        "session/turns/list",
        serde_json::json!({ "sessionId": session_id.to_string() }),
    )
    .await;
    response["result"]["data"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn queue_push_idle_starts_turn_and_busy_queues_then_update_remove() -> Result<()> {
    use devo_protocol::native::rpc_turn::{SessionQueuePushResult, SessionQueueUpdateResult};
    use devo_protocol::native::turn::TurnStatus as NativeTurnStatus;

    let data_root = TempDir::new()?;
    let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let runtime = build_runtime_with_provider(
        data_root.path(),
        Arc::new(GatedProvider {
            open: Arc::clone(&open),
            started: Default::default(),
        }),
    );
    let (outbound, mut notifications) = super::outbound::test_outbound_channel(64);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    runtime
        .handle_acp_initialize(
            connection_id,
            Some(serde_json::json!(1)),
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "terminal": false },
                "_meta": { "devo": { "protocol": "native" } },
            }),
        )
        .await;
    let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
    let created = history_request(
        &runtime,
        connection_id,
        2,
        "subscription/create",
        serde_json::json!({
            "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
            "includeSnapshot": false,
        }),
    )
    .await;
    assert!(created.get("error").is_none(), "subscribe: {created}");

    let pushed = history_request(
        &runtime,
        connection_id,
        3,
        "session/queue/push",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "input": [{ "type": "text", "text": "first" }],
            "idempotencyKey": "push-1",
        }),
    )
    .await;
    let pushed: SessionQueuePushResult = json_result(&pushed, "rpc");
    let SessionQueuePushResult::Started { turn } = pushed else {
        panic!("idle push must start a turn");
    };
    assert_eq!(turn.session_id.as_str(), session_id.to_string());
    assert_eq!(turn.status, NativeTurnStatus::InProgress);
    assert_eq!(turn.sequence, 1);

    let queued = history_request(
        &runtime,
        connection_id,
        4,
        "session/queue/push",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "input": [{ "type": "text", "text": "second" }],
            "idempotencyKey": "push-2",
        }),
    )
    .await;
    let queued: SessionQueuePushResult = json_result(&queued, "rpc");
    let SessionQueuePushResult::Queued { entry } = queued else {
        panic!("busy push must queue");
    };
    assert_eq!(entry.position, 1);
    assert_eq!(entry.preview, "second");
    assert!(
        matches!(&entry.input.as_slice(), [devo_protocol::native::item::UserInput::Text { text }] if text == "second")
    );

    let updated = history_request(
        &runtime,
        connection_id,
        5,
        "session/queue/update",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "queueItemId": entry.queue_item_id.as_str(),
            "input": [{ "type": "text", "text": "edited" }],
        }),
    )
    .await;
    let updated: SessionQueueUpdateResult = json_result(&updated, "rpc");
    assert!(
        matches!(&updated.entry.input.as_slice(), [devo_protocol::native::item::UserInput::Text { text }] if text == "edited")
    );

    let third = history_request(
        &runtime,
        connection_id,
        6,
        "session/queue/push",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "input": [{ "type": "text", "text": "third" }],
            "idempotencyKey": "push-3",
        }),
    )
    .await;
    let third: SessionQueuePushResult = json_result(&third, "rpc");
    let SessionQueuePushResult::Queued { entry: third_entry } = third else {
        panic!("busy push must queue");
    };
    let reordered = history_request(
        &runtime,
        connection_id,
        7,
        "session/queue/update",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "queueItemId": third_entry.queue_item_id.as_str(),
            "position": 1,
        }),
    )
    .await;
    let reordered: SessionQueueUpdateResult = json_result(&reordered, "rpc");
    assert_eq!(reordered.entry.position, 1);
    let entries = queue_list(&runtime, connection_id, session_id).await;
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.queue_item_id.as_str().to_owned())
            .collect::<Vec<_>>(),
        vec![
            third_entry.queue_item_id.as_str().to_owned(),
            entry.queue_item_id.as_str().to_owned()
        ]
    );
    // The reorder survives in SQLite too.
    let db_entries = runtime
        .deps
        .db
        .list_pending(&session_id, crate::db::QueueType::Turn)?;
    assert_eq!(db_entries.len(), 2);
    assert_eq!(
        db_entries[0].id.to_string(),
        third_entry.queue_item_id.as_str()
    );

    // Remove works; removing again reports the entry is gone.
    let removed = history_request(
        &runtime,
        connection_id,
        8,
        "session/queue/remove",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "queueItemId": third_entry.queue_item_id.as_str(),
        }),
    )
    .await;
    assert!(removed.get("error").is_none(), "remove: {removed}");
    let removed_again = history_request(
        &runtime,
        connection_id,
        9,
        "session/queue/remove",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "queueItemId": third_entry.queue_item_id.as_str(),
        }),
    )
    .await;
    assert_eq!(
        removed_again["error"]["code"],
        serde_json::json!("QueueItemNotFound")
    );

    // queue/updated notifications reached the new-style subscriber.
    let mut changes = Vec::new();
    while let Ok(Some(frame)) =
        tokio::time::timeout(Duration::from_millis(50), notifications.recv()).await
    {
        if frame["method"] == serde_json::json!("queue/updated") {
            changes.push(frame["params"]["change"].clone());
        }
    }
    assert!(
        changes.contains(&serde_json::json!("added"))
            && changes.contains(&serde_json::json!("updated"))
            && changes.contains(&serde_json::json!("removed")),
        "expected added/updated/removed notifications, got {changes:?}"
    );

    const PUSH_COUNT: usize = 8;
    let mut tasks = Vec::new();
    for index in 0..PUSH_COUNT {
        let runtime = Arc::clone(&runtime);
        tasks.push(tokio::spawn(async move {
            runtime
                .handle_incoming(
                    connection_id,
                    serde_json::json!({
                        "id": 500 + index as u64,
                        "method": "session/queue/push",
                        "params": {
                            "sessionId": session_id.to_string(),
                            "input": [{ "type": "text", "text": format!("queued {index}") }],
                            "idempotencyKey": format!("push-{index}"),
                        },
                    }),
                )
                .await
        }));
    }
    for (index, task) in tasks.into_iter().enumerate() {
        let response = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .with_context(|| format!("queue push {index} did not respond in time"))?
            .context("queue push task panicked")?
            .expect("queue push response");
        assert!(
            response.get("error").is_none(),
            "queue push {index} failed: {response}"
        );
    }
    // The "edited" entry from the update/remove phase is still queued (the
    // gated turn stays busy until `open` flips), so the list is that survivor
    // plus the eight parallel pushes.
    let entries_after_burst = queue_list(&runtime, connection_id, session_id).await;
    assert_eq!(
        entries_after_burst.len(),
        PUSH_COUNT + 1,
        "expected the surviving 'edited' entry plus {PUSH_COUNT} pushes, got {:?}",
        entries_after_burst
            .iter()
            .map(|e| e.preview.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(entries_after_burst[0].preview, "edited");
    open.store(true, std::sync::atomic::Ordering::SeqCst);

    Ok(())
}

#[tokio::test]
async fn turn_steer_injects_and_late_unconsumed_degrades_back_to_queue() -> Result<()> {
    use devo_protocol::native::rpc_turn::TurnSteerResult;

    let (env, open, started) = NativeEnv::gated().await?;
    let session_id = env.session().await?;
    let turn_id = start_turn(&env.runtime, env.connection_id, session_id, "go").await?;
    // Wait until the model call is actually in flight: a steer admitted
    // before the query loop's first pending-input drain would be consumed
    // into the prompt (the correct injection outcome), which is not the
    // degrade path this test exercises.
    tokio::time::timeout(Duration::from_secs(5), async {
        while !started.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;

    let steered = env
        .rpc(
            3,
            "turn/steer",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "expectedTurnId": turn_id.to_string(),
                "input": [{ "type": "text", "text": "steer me" }],
                "clientUserMessageId": "steer-direct-client-id",
                "idempotencyKey": "steer-direct",
            }),
        )
        .await;
    let steered: TurnSteerResult = json_result(&steered, "rpc");
    let TurnSteerResult::Injected { item_id } = steered else {
        panic!("expected Injected, got {steered:?}");
    };
    assert!(!item_id.as_str().is_empty());
    let items: devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope> =
        json_result(
            &env.rpc(
                30,
                "session/items/list",
                serde_json::json!({ "sessionId": session_id.to_string() }),
            )
            .await,
            "rpc",
        );
    assert!(items.data.iter().any(|item| {
        item.id == item_id
            && matches!(
                &item.item,
                devo_protocol::native::item::Item::UserMessage {
                    client_user_message_id: Some(id),
                    entry: devo_protocol::native::item::UserMessageEntry::Steer,
                    ..
                } if id == "steer-direct-client-id"
            )
    }));

    // Interrupt the turn before the next injection boundary: the
    // admitted steer is never consumed; it degrades back into the
    // session queue, and the now-idle session drains it into a new
    // turn — the message is never lost.
    let interrupted = env
        .rpc(
            4,
            "session/interrupt",
            serde_json::json!({
                "scope": {
                    "scope": "session",
                    "sessionId": session_id.to_string()
                },
            }),
        )
        .await;
    assert!(
        interrupted.get("error").is_none(),
        "interrupt: {interrupted}"
    );
    // Open the gate: turn 1 settles as interrupted; the follow-up turn
    // started by the queue drain runs to completion immediately.
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let turns = session_turns_json(&env.runtime, env.connection_id, session_id).await;
            let has_completed_followup = turns
                .iter()
                .any(|turn| turn["status"] == serde_json::json!("completed"));
            if has_completed_followup
                && queue_list(&env.runtime, env.connection_id, session_id)
                    .await
                    .is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    // Turn 1 is interrupted; the degraded steer drained into a second,
    // completed turn (two distinct turn ids, message processed).
    let turns = session_turns_json(&env.runtime, env.connection_id, session_id).await;
    let statuses: Vec<&str> = turns
        .iter()
        .filter_map(|turn| turn["status"].as_str())
        .collect();
    assert!(
        statuses.contains(&"interrupted") && statuses.contains(&"completed"),
        "expected interrupted + completed turns, got {statuses:?}"
    );
    let turn_ids: std::collections::HashSet<&str> = turns
        .iter()
        .filter_map(|turn| turn["id"].as_str())
        .collect();
    assert_eq!(turn_ids.len(), 2, "expected two distinct turns: {turns:?}");

    // Native semantics: steering after the turn ends degrades into the
    // session queue (message never lost). A fresh push on the now-idle
    // session starts a new turn.
    let late_steer = env
        .rpc(
            5,
            "turn/steer",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "expectedTurnId": turn_id.to_string(),
                "input": [{ "type": "text", "text": "too late" }],
                "idempotencyKey": "steer-late",
            }),
        )
        .await;
    let late: TurnSteerResult = json_result(&late_steer, "rpc");
    assert!(
        matches!(late, TurnSteerResult::DegradedToQueue { .. }),
        "steer after turn end must degrade to queue: {late:?}"
    );
    // The fresh push either starts immediately (fully idle) or queues
    // briefly behind the degraded steer's drain turn — the actor admits a
    // drained turn a beat before the registry records it, so "Started"
    // cannot be asserted race-free here. The contract that matters: the
    // message is never lost — it becomes its own turn and the queue empties.
    let turns_before = session_turns_json(&env.runtime, env.connection_id, session_id)
        .await
        .len();
    let pushed = env
        .rpc(
            6,
            "session/queue/push",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "start next" }],
                "idempotencyKey": "push-late",
            }),
        )
        .await;
    assert!(pushed.get("error").is_none(), "push: {pushed}");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let drained = queue_list(&env.runtime, env.connection_id, session_id)
                .await
                .is_empty();
            let grew = session_turns_json(&env.runtime, env.connection_id, session_id)
                .await
                .len()
                > turns_before;
            if drained && grew {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("fresh push never drained into its own turn")?;
    let items: devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope> =
        json_result(
            &env.rpc(
                31,
                "session/items/list",
                serde_json::json!({ "sessionId": session_id.to_string() }),
            )
            .await,
            "rpc",
        );
    assert!(items.data.iter().any(|item| matches!(
        &item.item,
        devo_protocol::native::item::Item::UserMessage {
            client_user_message_id: Some(id),
            entry: devo_protocol::native::item::UserMessageEntry::Queue,
            ..
        } if id == "turn/steer:steer-late"
    )));

    Ok(())
}

#[tokio::test]
async fn queue_persistence_restart_and_steer_restore() -> Result<()> {
    use devo_protocol::native::rpc_turn::{SessionQueuePushResult, TurnSteerResult};

    let env = NativeEnv::connect().await?;
    let session_id = env.session().await?;
    let queued_item = devo_core::PendingInputItem::new(
        devo_core::PendingInputKind::UserText {
            text: "queued text".into(),
        },
        None,
        chrono::Utc::now(),
    );
    let steer_item = devo_core::PendingInputItem::new(
        devo_core::PendingInputKind::UserText {
            text: "stale steer".into(),
        },
        None,
        chrono::Utc::now(),
    );
    env.runtime
        .deps
        .db
        .push_pending(&session_id, crate::db::QueueType::Turn, &queued_item)?;
    env.runtime
        .deps
        .db
        .push_pending(&session_id, crate::db::QueueType::Steer, &steer_item)?;
    let cwd = env.cwd().to_path_buf();
    drop(env.runtime);

    let rebuilt = build_runtime(&cwd);
    rebuilt.load_persisted_sessions().await?;
    let rebuilt_connection = initialized_connection(&rebuilt).await;
    let entries = queue_list(&rebuilt, rebuilt_connection, session_id).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].preview, "stale steer");
    assert!(
        rebuilt
            .deps
            .db
            .list_pending(&session_id, crate::db::QueueType::Steer)?
            .is_empty()
    );
    drop(rebuilt);
    let restarted = build_runtime(&cwd);
    restarted.load_persisted_sessions().await?;
    let restarted_connection = initialized_connection(&restarted).await;
    assert!(
        queue_list(&restarted, restarted_connection, session_id)
            .await
            .is_empty()
    );

    let (env, open, started) = NativeEnv::gated().await?;
    let session_id = env.session().await?;
    start_turn(&env.runtime, env.connection_id, session_id, "hold open").await?;
    wait_flag(&started, "turn started").await;
    let queued: SessionQueuePushResult = json_result(
        &env.rpc(
            3,
            "session/queue/push",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "after restart" }],
                "idempotencyKey": "push-after-restart",
                "clientUserMessageId": "queue-after-restart-message",
            }),
        )
        .await,
        "rpc",
    );
    assert!(matches!(queued, SessionQueuePushResult::Queued { .. }));
    let pending = env
        .runtime
        .deps
        .db
        .list_pending(&session_id, crate::db::QueueType::Turn)?;
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0]
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("clientUserMessageId"))
            .and_then(serde_json::Value::as_str),
        Some("queue-after-restart-message")
    );
    env.runtime.shutdown().await;
    let cwd = env.cwd().to_path_buf();
    drop(env.runtime);
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    let rebuilt = build_runtime_with_provider(
        &cwd,
        Arc::new(GatedProvider {
            open: Arc::clone(&open),
            started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }),
    );
    rebuilt.load_persisted_sessions().await?;
    let rebuilt_connection = initialized_connection(&rebuilt).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let entries = queue_list(&rebuilt, rebuilt_connection, session_id).await;
            let turns = session_turns_json(&rebuilt, rebuilt_connection, session_id).await;
            if entries.is_empty()
                && turns
                    .iter()
                    .any(|turn| turn["status"] == serde_json::json!("completed"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await?;
    let items: devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope> =
        conn_page(
            &rebuilt,
            rebuilt_connection,
            8,
            "session/items/list",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
    assert!(items.data.iter().any(|item| matches!(
        &item.item,
        devo_protocol::native::item::Item::UserMessage {
            client_user_message_id: Some(id),
            entry: devo_protocol::native::item::UserMessageEntry::Queue,
            ..
        } if id == "queue-after-restart-message"
    )));

    let (env, open, started) = NativeEnv::gated().await?;
    let session_id = env.session().await?;
    let turn_id = start_turn(&env.runtime, env.connection_id, session_id, "go").await?;
    wait_flag(&started, "turn started").await;
    let steered: TurnSteerResult = json_result(
        &env.rpc(
            3,
            "turn/steer",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "expectedTurnId": turn_id.to_string(),
                "input": [{ "type": "text", "text": "duplicate steer" }],
                "idempotencyKey": "steer-dedup",
            }),
        )
        .await,
        "rpc",
    );
    let TurnSteerResult::Injected { item_id } = steered else {
        panic!("expected Injected, got {steered:?}");
    };
    assert!(!item_id.as_str().is_empty());
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    wait_turn_idle(&env.runtime, session_id).await;
    env.runtime.deps.db.push_pending(
        &session_id,
        crate::db::QueueType::Steer,
        &devo_core::PendingInputItem::new(
            devo_core::PendingInputKind::UserText {
                text: "duplicate steer".into(),
            },
            None,
            chrono::Utc::now(),
        ),
    )?;
    let cwd = env.cwd().to_path_buf();
    drop(env.runtime);
    let rebuilt = build_runtime(&cwd);
    rebuilt.load_persisted_sessions().await?;
    let rebuilt_connection = initialized_connection(&rebuilt).await;
    assert!(
        queue_list(&rebuilt, rebuilt_connection, session_id)
            .await
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn mid_turn_read_rpcs_and_active_status() -> Result<()> {
    use devo_protocol::native::session::SessionStatus;

    let (env, open, started) = NativeEnv::gated().await?;
    let session_id = env.session().await?;
    let turn_started: devo_protocol::native::rpc_turn::TurnStartResult = json_result(
        &env.rpc(
            2,
            "turn/start",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "hold the turn" }],
                "idempotencyKey": "mid-turn-reads",
            }),
        )
        .await,
        "rpc",
    );
    wait_flag(&started, "turn should start streaming").await;
    let deadline = Duration::from_millis(500);
    for (id, method, params) in [
        (20, "session/list", serde_json::json!({})),
        (
            21,
            "session/items/list",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        ),
        (
            22,
            "workspace/changes/read",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "scopes": ["uncommitted"],
            }),
        ),
        (23, "runtime/ping", serde_json::json!({})),
    ] {
        let response = tokio::time::timeout(deadline, env.rpc(id, method, params))
            .await
            .unwrap_or_else(|_| panic!("{method} must return mid-turn"));
        assert!(
            response.get("result").is_some() || response.get("error").is_some(),
            "{method}: {response}"
        );
    }
    let listed: devo_protocol::native::rpc_session::SessionListResult = json_result(
        &env.rpc(24, "session/list", serde_json::json!({})).await,
        "rpc",
    );
    let listed_session = listed
        .data
        .iter()
        .find(|session| session.id.as_str() == session_id.to_string())
        .expect("listed session");
    assert_eq!(listed_session.status, SessionStatus::Active);
    assert_eq!(
        listed_session.active_turn_id.as_ref(),
        Some(&turn_started.turn.id)
    );
    let read: devo_protocol::native::rpc_session::SessionReadResult = json_result(
        &env.rpc(
            25,
            "session/read",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await,
        "rpc",
    );
    assert_eq!(read.session.status, SessionStatus::Active);
    assert_eq!(
        read.session.active_turn_id.as_ref(),
        Some(&turn_started.turn.id)
    );

    open.store(true, std::sync::atomic::Ordering::SeqCst);
    wait_turn_idle(&env.runtime, session_id).await;
    let listed_idle: devo_protocol::native::rpc_session::SessionListResult = json_result(
        &env.rpc(26, "session/list", serde_json::json!({})).await,
        "rpc",
    );
    let listed_idle_session = listed_idle
        .data
        .iter()
        .find(|session| session.id.as_str() == session_id.to_string())
        .expect("listed session after turn");
    assert_eq!(listed_idle_session.status, SessionStatus::Idle);
    assert_eq!(listed_idle_session.active_turn_id, None);
    Ok(())
}

#[tokio::test]
async fn native_metadata_update_branches() -> Result<()> {
    let env = NativeEnv::connect().await?;
    let session_id = env.session().await?;

    let response = env
        .rpc(
            7,
            "session/metadata/update",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "expectedVersion": 1,
                "settings": { "permissionProfile": "fullAccess" },
            }),
        )
        .await;
    let result: devo_protocol::native::rpc_session::SessionMetadataUpdateResult =
        json_result(&response, "rpc");
    assert_eq!(
        result.session.settings.permission_profile,
        devo_protocol::native::model::PermissionProfile::FullAccess
    );
    assert_eq!(result.session.version, 2);
    let rollout_path = env
        .runtime
        .rollout_store
        .find_rollout_by_session_id(&session_id)?
        .expect("rollout exists");
    let history = devo_core::read_canonical_history(&rollout_path)?;
    assert_eq!(
        history
            .session
            .expect("session")
            .settings
            .permission_profile,
        devo_protocol::native::model::PermissionProfile::FullAccess
    );

    let conflict = env
        .rpc(
            8,
            "session/metadata/update",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "expectedVersion": 1,
                "settings": { "permissionProfile": "autoReview" },
            }),
        )
        .await;
    assert_eq!(
        conflict["error"]["code"].as_str(),
        Some("WORKSPACE_VERSION_CONFLICT")
    );

    let renamed = env
        .rpc(
            9,
            "session/metadata/update",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "expectedVersion": 0,
                "title": "renamed session",
            }),
        )
        .await;
    let renamed: devo_protocol::native::rpc_session::SessionMetadataUpdateResult =
        json_result(&renamed, "rpc");
    assert_eq!(renamed.session.title.as_deref(), Some("renamed session"));
    let cleared = env
        .rpc(
            10,
            "session/metadata/update",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "expectedVersion": 0,
                "title": null,
            }),
        )
        .await;
    assert!(cleared.get("error").is_some());
    Ok(())
}

#[tokio::test]
async fn metadata_update_auto_refine_settings_reach_live_actor() -> Result<()> {
    // Regression: auto-refine / python-wait settings were persisted to the
    // rollout but never mirrored into the live session actor, so the
    // post-turn auto-refine interval check kept reading the actor's stale
    // settings (default interval 25) until a restart rehydrated the session.
    let env = NativeEnv::connect().await?;
    let session_id = env.session().await?;

    let response = env
        .rpc(
            7,
            "session/metadata/update",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "expectedVersion": 1,
                "settings": {
                    "autoRefineEnabled": true,
                    "autoRefineTurnInterval": 2,
                    "pythonCellFirstWaitMs": 250,
                },
            }),
        )
        .await;
    let result: devo_protocol::native::rpc_session::SessionMetadataUpdateResult =
        json_result(&response, "rpc");
    assert_eq!(result.session.settings.auto_refine_enabled, Some(true));
    assert_eq!(result.session.settings.auto_refine_turn_interval, Some(2));
    assert_eq!(result.session.settings.python_cell_first_wait_ms, Some(250));

    // The actor must hold the new values — this is the exact snapshot the
    // post-turn auto-refine check reads. Assert on the actor handle directly
    // so the history fallback in native_session_snapshot cannot mask it.
    let handle = env
        .runtime
        .session(session_id)
        .await
        .expect("session actor must be live for session/new");
    let actor_session = handle.native_session().await.expect("actor native session");
    assert_eq!(actor_session.settings.auto_refine_enabled, Some(true));
    assert_eq!(actor_session.settings.auto_refine_turn_interval, Some(2));
    assert_eq!(actor_session.settings.python_cell_first_wait_ms, Some(250));

    // And the snapshot the post-turn pipeline uses agrees.
    let snapshot = env
        .runtime
        .native_session_snapshot(session_id)
        .await
        .expect("session snapshot");
    assert_eq!(snapshot.settings.auto_refine_enabled, Some(true));
    assert_eq!(snapshot.settings.auto_refine_turn_interval, Some(2));
    Ok(())
}

#[tokio::test]
async fn native_metadata_update_during_active_turn() -> Result<()> {
    let (env, open, started) = NativeEnv::gated().await?;
    let session_id = env.session().await?;
    let _turn_id = start_turn(&env.runtime, env.connection_id, session_id, "hold the turn").await?;
    wait_flag(&started, "turn should start streaming").await;

    let response = env
        .rpc(
            7,
            "session/metadata/update",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "expectedVersion": 1,
                "settings": { "permissionProfile": "fullAccess" },
            }),
        )
        .await;
    let result: devo_protocol::native::rpc_session::SessionMetadataUpdateResult =
        json_result(&response, "rpc");
    assert!(result.applied_to_active_turn);
    let stream = env
        .runtime
        .active_stream_state(session_id)
        .await
        .expect("stream");
    let stream = stream.lock().await;
    let inline = stream.turn_inline.as_ref().expect("turn inline state");
    assert_eq!(
        inline.hook_context.config.permission_profile.preset,
        devo_safety::PermissionPreset::FullAccess
    );
    drop(stream);
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

#[tokio::test]
async fn native_turn_start_admits_busy_and_replays() -> Result<()> {
    let data_root = TempDir::new()?;
    let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let runtime = build_runtime_with_provider(
        data_root.path(),
        Arc::new(GatedProvider {
            open: Arc::clone(&open),
            started: Default::default(),
        }),
    );
    let connection_id = initialized_connection(&runtime).await;
    let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

    let params = serde_json::json!({
        "sessionId": session_id.to_string(),
        "input": [{ "type": "text", "text": "hello" }],
        "idempotencyKey": "turn-replay",
    });
    let started = tokio::time::timeout(
        Duration::from_secs(2),
        history_request(&runtime, connection_id, 7, "turn/start", params.clone()),
    )
    .await
    .context("native turn/start must return before the turn finishes")?;
    let first: devo_protocol::native::rpc_turn::TurnStartResult = json_result(&started, "rpc");
    assert_eq!(first.turn.session_id.as_str(), session_id.to_string());
    assert_eq!(
        first.turn.status,
        devo_protocol::native::turn::TurnStatus::InProgress
    );

    let busy = history_request(
        &runtime,
        connection_id,
        8,
        "turn/start",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "input": [{ "type": "text", "text": "second" }],
            "idempotencyKey": "turn-2",
        }),
    )
    .await;
    assert_eq!(busy["error"]["code"].as_str(), Some("TurnAlreadyRunning"));

    let replay = history_request(&runtime, connection_id, 9, "turn/start", params).await;
    let replay: devo_protocol::native::rpc_turn::TurnStartResult = json_result(&replay, "rpc");
    assert_eq!(replay.turn, first.turn);

    let env = NativeEnv::connect().await?;
    let other_session = env.session().await?;
    let rejected = env
        .rpc(
            10,
            "turn/start",
            serde_json::json!({
                "sessionId": other_session.to_string(),
                "input": [{ "type": "image", "uri": "https://example.com/x.png" }],
                "idempotencyKey": "turn-image",
            }),
        )
        .await;
    assert!(
        rejected["error"]["code"]
            .as_str()
            .is_some_and(|code| code.contains("InvalidParams") || code == "INVALID_PARAMS"),
        "unsupported input must be rejected: {rejected}"
    );

    open.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

#[tokio::test]
async fn native_session_compact_start_busy_rejects_and_idle_compacts() -> Result<()> {
    let data_root = TempDir::new()?;
    let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let runtime = build_runtime_with_provider(
        data_root.path(),
        Arc::new(GatedProvider {
            open: Arc::clone(&open),
            started: Default::default(),
        }),
    );
    let connection_id = initialized_connection(&runtime).await;
    let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
    let _turn_id = start_turn(&runtime, connection_id, session_id, "hold the turn").await?;

    let busy = history_request(
        &runtime,
        connection_id,
        7,
        "session/compact/start",
        serde_json::json!({ "sessionId": session_id.to_string() }),
    )
    .await;
    assert_eq!(
        busy["error"]["code"].as_str(),
        Some("TurnAlreadyRunning"),
        "active turn must reject compaction: {busy}"
    );

    // is idle afterwards and compaction may start.
    let interrupted = history_request(
        &runtime,
        connection_id,
        8,
        "session/interrupt",
        serde_json::json!({
            "scope": {
                "scope": "session",
                "sessionId": session_id.to_string()
            },
        }),
    )
    .await;
    assert!(
        interrupted.get("error").is_none(),
        "interrupt failed: {interrupted}"
    );
    let started = history_request(
        &runtime,
        connection_id,
        9,
        "session/compact/start",
        serde_json::json!({ "sessionId": session_id.to_string() }),
    )
    .await;
    let result: devo_protocol::native::rpc_turn::TurnStartResult = json_result(&started, "rpc");
    assert_eq!(
        result.turn.kind,
        devo_protocol::native::turn::TurnKind::Compaction
    );
    Ok(())
}

struct StreamOkCompletionFailProvider;

#[async_trait]
impl ModelProviderSDK for StreamOkCompletionFailProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("summarizer provider unavailable")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(futures::stream::iter(vec![Ok(
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "stream-ok".into(),
                    content: vec![devo_protocol::ResponseContent::Text("ok".into())],
                    stop_reason: Some(devo_protocol::StopReason::EndTurn),
                    usage: devo_protocol::Usage::default(),
                    metadata: devo_protocol::ResponseMetadata::default(),
                },
            },
        )])))
    }

    fn name(&self) -> &str {
        "stream-ok-completion-fail"
    }
}

#[tokio::test]
async fn auto_refine_interval_applies_at_turn_boundary() -> Result<()> {
    // Regression: the auto-refine interval trigger must fire end-to-end —
    // settings reaching the live actor, the turn counter crossing the
    // interval scheduling a pending refine, and the NEXT turn boundary
    // applying (and persisting) the refinement. LLM planning fails fast on
    // this provider, exercising the heuristic fallback.
    let data_root = TempDir::new()?;
    let runtime =
        build_runtime_with_provider(data_root.path(), Arc::new(StreamOkCompletionFailProvider));
    let connection_id = initialized_connection(&runtime).await;
    let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

    let update = history_request(
        &runtime,
        connection_id,
        300,
        "session/metadata/update",
        serde_json::json!({
            "sessionId": session_id,
            "expectedVersion": 1,
            "settings": { "autoRefineEnabled": true, "autoRefineTurnInterval": 2 },
        }),
    )
    .await;
    assert!(update.get("error").is_none(), "update: {update}");

    // Turn 1: count 1 (odd) — no schedule. Turn 2: count 2 — schedules the
    // pending refine. Turn 3's boundary applies it.
    for text in ["first", "second", "third"] {
        start_turn(&runtime, connection_id, session_id, text).await?;
        wait_turn_idle(&runtime, session_id).await;
    }

    let rollout_path = runtime
        .rollout_store
        .find_rollout_by_session_id(&session_id)?
        .expect("rollout exists");
    let mut saw_refinement = false;
    for _ in 0..50 {
        let journal = std::fs::read_to_string(&rollout_path)?;
        if journal.contains("\"refinement\"") {
            saw_refinement = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(
        saw_refinement,
        "auto-interval refinement must be journaled after the boundary turn"
    );
    Ok(())
}

#[tokio::test]
async fn manual_compaction_failure_broadcasts_single_terminal_with_error() -> Result<()> {
    // Regression: a failed manual compaction used to broadcast TurnCompleted
    // twice (duplicate "Error: Error" lines in the TUI) with turn.error never
    // populated (rendered as a bare "Error"). The failure path must emit one
    // terminal carrying the real reason.
    let data_root = TempDir::new()?;
    let runtime =
        build_runtime_with_provider(data_root.path(), Arc::new(StreamOkCompletionFailProvider));
    let (connection_id, mut notifications) = connect_stdio(&runtime, true, 32).await;
    let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
    // Compaction of an empty history skips before the model call; seed one
    // turn so the summarizer (and its failing completion) is actually reached.
    start_turn(&runtime, connection_id, session_id, "seed history").await?;
    wait_turn_idle(&runtime, session_id).await;
    let compact_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 7,
                "method": "session/compact/start",
                "params": { "sessionId": session_id }
            }),
        )
        .await
        .expect("session/compact/start response");
    let compact: devo_protocol::native::rpc_turn::TurnStartResult =
        json_result(&compact_response, "compact");
    let compaction_turn_id = compact.turn.id.as_str().to_owned();

    let mut terminal_frames = Vec::new();
    let mut compaction_failed_count = 0usize;
    let mut seen_methods = Vec::new();
    let deadline = Duration::from_secs(30);
    let collect = async {
        while terminal_frames.is_empty() {
            let frame =
                match tokio::time::timeout(Duration::from_secs(8), notifications.recv()).await {
                    Ok(Some(frame)) => frame,
                    Ok(None) => anyhow::bail!("notification channel closed; saw {seen_methods:?}"),
                    Err(_) => anyhow::bail!("8s notification silence; saw {seen_methods:?}"),
                };
            if let Some(method) = frame["method"].as_str() {
                seen_methods.push(method.to_owned());
            }
            if frame["method"] == serde_json::json!("turn/completed")
                && frame["params"]["turn"]["id"].as_str() == Some(compaction_turn_id.as_str())
            {
                terminal_frames.push(frame);
            } else if frame["method"] == serde_json::json!("context/compactionFailed") {
                compaction_failed_count += 1;
            }
        }
        anyhow::Ok(())
    };
    tokio::time::timeout(deadline, collect)
        .await
        .map_err(|_| anyhow::anyhow!("no terminal for compaction turn; saw {seen_methods:?}"))
        .context("compaction turn never reached a terminal broadcast")??;
    // Keep draining briefly: unrelated idle/status frames are fine, but no
    // second terminal for this turn may follow.
    let grace = async {
        while let Ok(Some(frame)) =
            tokio::time::timeout(Duration::from_millis(150), notifications.recv()).await
        {
            if frame["method"] == serde_json::json!("turn/completed")
                && frame["params"]["turn"]["id"].as_str() == Some(compaction_turn_id.as_str())
            {
                terminal_frames.push(frame);
            } else if frame["method"] == serde_json::json!("context/compactionFailed") {
                compaction_failed_count += 1;
            }
        }
    };
    grace.await;

    assert_eq!(
        terminal_frames.len(),
        1,
        "one TurnCompleted per failed compaction turn"
    );
    let turn = &terminal_frames[0]["params"]["turn"];
    assert_eq!(turn["status"], serde_json::json!("failed"));
    let message = turn["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        message.contains("summarizer provider unavailable"),
        "terminal turn must carry the compaction failure reason, got {message:?}"
    );
    assert_eq!(compaction_failed_count, 1);
    Ok(())
}

#[tokio::test]
async fn native_session_lifecycle_and_routing() -> Result<()> {
    let env = NativeEnv::native().await?;
    let params = serde_json::json!({
        "cwd": env.cwd(),
        "idempotencyKey": "session-new-1",
    });
    let created = history_request(
        &env.runtime,
        env.connection_id,
        7,
        "session/new",
        params.clone(),
    )
    .await;
    let created: devo_protocol::native::rpc_session::SessionNewResult =
        json_result(&created, "rpc");
    assert_eq!(created.session.cwd, env.cwd());
    let replay = history_request(&env.runtime, env.connection_id, 8, "session/new", params).await;
    let replay: devo_protocol::native::rpc_session::SessionNewResult = json_result(&replay, "rpc");
    assert_eq!(replay.session.id, created.session.id);
    let resumed: devo_protocol::native::rpc_session::SessionResumeResult = json_result(
        &env.rpc(
            9,
            "session/resume",
            serde_json::json!({ "sessionId": created.session.id.as_str() }),
        )
        .await,
        "rpc",
    );
    assert_eq!(resumed.session.id, created.session.id);

    let listed: devo_protocol::native::rpc_session::SessionListResult = json_result(
        &env.rpc(10, "session/list", serde_json::json!({})).await,
        "rpc",
    );
    assert!(
        listed
            .data
            .iter()
            .any(|session| session.id == created.session.id)
    );
    let read: devo_protocol::native::rpc_session::SessionReadResult = json_result(
        &env.rpc(
            11,
            "session/read",
            serde_json::json!({ "sessionId": created.session.id.as_str() }),
        )
        .await,
        "rpc",
    );
    assert_eq!(read.session, created.session);
    assert_eq!(
        env.rpc(
            12,
            "session/read",
            serde_json::json!({ "sessionId": SessionId::new().to_string() }),
        )
        .await["error"]["code"]
            .as_str(),
        Some("SessionNotFound")
    );
    let pong: devo_protocol::native::rpc_admin::RuntimePingResult = json_result(
        &env.rpc(13, "runtime/ping", serde_json::json!({})).await,
        "rpc",
    );
    assert!(pong.server_time_ms > 0);

    let delete_target = env
        .rpc(
            14,
            "session/new",
            serde_json::json!({
                "cwd": env.cwd(),
                "idempotencyKey": "session-list-delete",
            }),
        )
        .await;
    let delete_target: devo_protocol::native::rpc_session::SessionNewResult =
        json_result(&delete_target, "rpc");
    assert!(
        env.rpc(
            15,
            "session/delete",
            serde_json::json!({ "sessionId": delete_target.session.id.as_str() }),
        )
        .await
        .get("error")
        .is_none()
    );
    let listed_after: devo_protocol::native::rpc_session::SessionListResult = json_result(
        &env.rpc(16, "session/list", serde_json::json!({})).await,
        "rpc",
    );
    assert!(
        !listed_after
            .data
            .iter()
            .any(|session| session.id == delete_target.session.id)
    );

    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let acp_connection_id = initialized_with_protocol_meta(&runtime, false).await;
    let native_connection_id = initialized_with_protocol_meta(&runtime, true).await;
    let acp_response = history_request(
        &runtime,
        acp_connection_id,
        17,
        "session/new",
        serde_json::json!({ "cwd": data_root.path(), "mcpServers": [] }),
    )
    .await;
    assert!(
        acp_response["result"]["sessionId"].is_string()
            && acp_response["result"]["session"].is_null()
    );
    let native_response = history_request(
        &runtime,
        native_connection_id,
        18,
        "session/new",
        serde_json::json!({
            "cwd": data_root.path(),
            "idempotencyKey": "session-new-native",
        }),
    )
    .await;
    assert!(native_response["result"]["session"]["id"].is_string());
    Ok(())
}

#[tokio::test]
async fn native_task_process_io_read_and_list() -> Result<()> {
    let env = NativeEnv::connect().await?;
    let session_id = env.session().await?;

    let params = serde_json::json!({
        "kind": "process",
        "sessionId": session_id.to_string(),
        "command": "sleep 60",
        "idempotencyKey": "task-1",
    });
    let started: devo_protocol::native::rpc_turn::TaskStartResult = json_result(
        &history_request(
            &env.runtime,
            env.connection_id,
            7,
            "task/start",
            params.clone(),
        )
        .await,
        "rpc",
    );
    assert!(started.item_id.as_str().starts_with("item_"));
    let replay: devo_protocol::native::rpc_turn::TaskStartResult = json_result(
        &history_request(&env.runtime, env.connection_id, 8, "task/start", params).await,
        "rpc",
    );
    assert_eq!(replay.item_id, started.item_id);
    assert!(
        env.rpc(
            9,
            "session/interrupt",
            serde_json::json!({
                "scope": { "scope": "task", "itemId": started.item_id.as_str() }
            }),
        )
        .await
        .get("error")
        .is_none()
    );

    let cat: devo_protocol::native::rpc_turn::TaskStartResult = json_result(
        &env.rpc(
            10,
            "task/start",
            serde_json::json!({
                "kind": "process",
                "sessionId": session_id.to_string(),
                "command": "cat",
                "idempotencyKey": "task-io",
            }),
        )
        .await,
        "rpc",
    );
    assert!(
        env.rpc(
            11,
            "task/write_stdin",
            serde_json::json!({
                "itemId": cat.item_id.as_str(),
                "data": "hello task\n",
            }),
        )
        .await
        .get("error")
        .is_none()
    );
    assert!(
        env.rpc(
            12,
            "task/resize",
            serde_json::json!({ "itemId": cat.item_id.as_str(), "rows": 40, "cols": 120 }),
        )
        .await
        .get("error")
        .is_none()
    );
    let _ = env
        .rpc(
            13,
            "task/interrupt",
            serde_json::json!({ "itemId": cat.item_id.as_str() }),
        )
        .await;

    let echo: devo_protocol::native::rpc_turn::TaskStartResult = json_result(
        &env.rpc(
            14,
            "task/start",
            serde_json::json!({
                "kind": "process",
                "sessionId": session_id.to_string(),
                "command": "echo native-task-read",
                "idempotencyKey": "task-read",
            }),
        )
        .await,
        "rpc",
    );
    let mut terminal = None;
    for _ in 0..100 {
        let read: devo_protocol::native::rpc_turn::TaskReadResult = json_result(
            &env.rpc(
                15,
                "task/read",
                serde_json::json!({ "itemId": echo.item_id.as_str() }),
            )
            .await,
            "rpc",
        );
        if let devo_protocol::native::item::Item::BackgroundTask {
            exit_code: Some(exit_code),
            state,
            ..
        } = &read.item.item
        {
            terminal = Some((*exit_code, *state, read.output_tail.clone()));
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let (exit_code, state, output_tail) = terminal.expect("terminal snapshot");
    assert_eq!(exit_code, 0);
    assert_eq!(
        state,
        devo_protocol::native::item::SpawnedWorkState::Completed
    );
    assert!(
        output_tail
            .as_deref()
            .unwrap_or_default()
            .contains("native-task-read")
    );
    let listed: devo_protocol::native::rpc_turn::TaskListResult = json_result(
        &env.rpc(
            16,
            "task/list",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await,
        "rpc",
    );
    assert!(listed.tasks.iter().any(|task| task.id == echo.item_id));

    let agent: devo_protocol::native::rpc_turn::TaskStartResult = json_result(
        &env.rpc(
            17,
            "task/start",
            serde_json::json!({
                "kind": "agent",
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "hi" }],
                "idempotencyKey": "task-2",
            }),
        )
        .await,
        "rpc",
    );
    assert!(agent.item_id.as_str().starts_with("item_"));
    Ok(())
}

#[tokio::test]
async fn approval_fanout_mixes_native_and_acp_surfaces() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let session_id = SessionId::new();

    let (native_outbound, mut native_receiver) = super::outbound::test_outbound_channel(4);
    let native_connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, native_outbound)
        .await;
    runtime
        .handle_acp_initialize(
            native_connection_id,
            Some(serde_json::json!(1)),
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "terminal": false },
                "_meta": { "devo": { "protocol": "native" } },
            }),
        )
        .await;
    let (acp_outbound, mut acp_receiver) = super::outbound::test_outbound_channel(4);
    let acp_connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, acp_outbound)
        .await;
    runtime
        .handle_acp_initialize(
            acp_connection_id,
            Some(serde_json::json!(1)),
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "terminal": false },
            }),
        )
        .await;
    let native_session_id = session_id;
    {
        let mut connections = runtime.connections.lock().await;
        connections
            .get_mut(&acp_connection_id)
            .expect("acp connection")
            .event_selectors = vec![devo_protocol::native::event::StreamSelector::Session {
            session_id: native_session_id,
        }];
        connections
            .get_mut(&native_connection_id)
            .expect("native connection")
            .event_selectors = vec![devo_protocol::native::event::StreamSelector::Session {
            session_id: native_session_id,
        }];
    }

    let acp_params: devo_protocol::AcpRequestPermissionParams =
        serde_json::from_value(serde_json::json!({
            "sessionId": session_id.to_string(),
            "toolCall": { "toolCallId": "call-1" },
            "options": [],
        }))?;
    let runtime_for_request = Arc::clone(&runtime);
    let (request_ready_tx, request_ready_rx) = oneshot::channel();
    let request = tokio::spawn(async move {
        runtime_for_request
            .request_permission_from_controllers(
                session_id,
                Some(native_connection_id),
                super::control_requests::PermissionControllerRequest {
                    acp_params,
                    native_method: "approval/command/request".to_string(),
                    native_params: serde_json::json!({
                        "type": "approval",
                        "approvalId": "call-1",
                        "actionSummary": "run tests",
                        "justification": "",
                    }),
                    ready: request_ready_tx,
                },
                CancellationToken::new(),
            )
            .await
    });

    assert_eq!(request_ready_rx.await?, Ok(()));
    let native_request = native_receiver.recv().await.expect("native request");
    assert_eq!(
        native_request["method"].as_str(),
        Some("approval/command/request"),
        "native connection must get the native reverse request: {native_request}"
    );
    assert_eq!(
        native_request["params"]["approvalId"].as_str(),
        Some("call-1")
    );
    let acp_request = acp_receiver.recv().await.expect("ACP request");
    assert_eq!(
        acp_request["method"].as_str(),
        Some("session/request_permission"),
        "ACP connection must get the ACP permission envelope: {acp_request}"
    );

    runtime
        .resolve_client_response(
            native_connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": native_request["id"].clone(),
                "result": {
                    "requestId": "call-1",
                    "decision": {
                        "decision": "approved",
                        "scope": "session",
                        "decidedAt": "2026-08-09T00:00:00Z",
                    },
                },
            }),
        )
        .await;
    let (decision, scope) = request
        .await?
        .map_err(|error| anyhow::anyhow!("permission fan-out failed: {error}"))?;
    assert_eq!(decision, devo_protocol::ApprovalDecisionValue::Approve);
    assert_eq!(scope, devo_protocol::ApprovalScopeValue::Session);
    Ok(())
}

#[tokio::test]
async fn native_user_input_request_resolves_pending_question() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path());
    let (outbound, mut receiver) = super::outbound::test_outbound_channel(8);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    runtime
        .handle_acp_initialize(
            connection_id,
            Some(serde_json::json!(1)),
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "terminal": false },
                "_meta": { "devo": { "protocol": "native" } },
            }),
        )
        .await;
    let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
    let native_session_id = runtime
        .session_summary_snapshot(session_id)
        .await
        .expect("summary")
        .native
        .id;
    {
        let mut connections = runtime.connections.lock().await;
        connections
            .get_mut(&connection_id)
            .expect("native connection")
            .event_selectors = vec![devo_protocol::native::event::StreamSelector::Session {
            session_id: native_session_id,
        }];
    }

    let turn_id = TurnId::new();
    let runtime_for_tool = Arc::clone(&runtime);
    let tool_call = tokio::spawn(async move {
        runtime_for_tool
            .request_user_input_for_tool(
                session_id,
                turn_id,
                "question-call-1".to_string(),
                devo_protocol::RequestUserInputArgs {
                    questions: vec![
                        serde_json::from_value(serde_json::json!({
                            "id": "q1",
                            "header": "Pick one",
                            "question": "Which color?",
                            "isOther": false,
                            "isSecret": false,
                        }))
                        .expect("question"),
                    ],
                },
            )
            .await
    });

    // Session setup emits its own notifications first; scan forward to
    // the reverse request frame.
    let request = {
        let mut request = None;
        for _ in 0..8 {
            let frame = receiver.recv().await.expect("userInput/request frame");
            if frame["method"].as_str() == Some("userInput/request") {
                request = Some(frame);
                break;
            }
        }
        request.expect("userInput/request must be fanned out")
    };
    assert_eq!(request["method"].as_str(), Some("userInput/request"));
    assert_eq!(
        request["params"]["requestId"].as_str(),
        Some("question-call-1")
    );

    runtime
        .resolve_client_response(
            connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": request["id"].clone(),
                "result": {
                    "requestId": "question-call-1",
                    "answers": { "q1": { "answers": ["blue"] } },
                },
            }),
        )
        .await;
    let response = tool_call
        .await?
        .map_err(|error| anyhow::anyhow!("user input tool call failed: {error}"))?;
    assert_eq!(
        response.answers.get("q1"),
        Some(&devo_protocol::RequestUserInputAnswer {
            answers: vec!["blue".to_string()],
        })
    );
    Ok(())
}

#[tokio::test]
async fn native_search_and_workspace_reads() -> Result<()> {
    let env = NativeEnv::native().await?;
    let session_id = start_durable_session(&env.runtime, env.connection_id, env.cwd()).await?;

    let started = env
        .rpc(
            7,
            "search/start",
            serde_json::json!({
                "cwd": env.cwd(),
                "query": "nothing-matches-this-query",
            }),
        )
        .await;
    let result: devo_protocol::native::rpc_search::SearchStartResult = json_result(&started, "rpc");
    assert_eq!(result.snapshot.query, "nothing-matches-this-query");
    assert!(
        started["result"]["snapshot"]["searchId"].is_string()
            && started["result"]["snapshot"]["fileSearchComplete"].is_boolean()
    );

    let read = env
        .rpc(
            8,
            "workspace/changes/read",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "scopes": ["uncommitted"],
            }),
        )
        .await;
    let result: devo_protocol::native::rpc_workspace::WorkspaceChangesReadResult =
        serde_json::from_value(read["result"].clone())?;
    assert_eq!(result.views.len(), 1);
    assert_eq!(
        result.views[0].scope,
        devo_protocol::WorkspaceChangeScope::Uncommitted
    );
    assert!(
        read["result"]["views"][0]["workspaceRoot"].is_string()
            && read["result"]["views"][0]["changeSetStatus"].is_string()
    );
    Ok(())
}

#[tokio::test]
async fn native_model_catalog_and_preferences() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime_with_provider_and_catalog(
        data_root.path(),
        Arc::new(NoopProvider::failing()),
        Arc::new(PresetModelCatalog::load().expect("embedded model catalog")),
    );
    let connection_id = initialized_with_protocol_meta(&runtime, true).await;
    let listed: devo_protocol::native::rpc_admin::ModelListResult = json_result(
        &history_request(
            &runtime,
            connection_id,
            7,
            "model/list",
            serde_json::json!({}),
        )
        .await,
        "rpc",
    );
    assert!(!listed.models.is_empty());
    let model = &listed.models[0];
    assert!(!model.slug.is_empty() && model.context_window > 0);

    let read: devo_protocol::native::rpc_admin::ModelPreferencesReadResult =
        serde_json::from_value(
            history_request(
                &runtime,
                connection_id,
                8,
                "model/preferences/read",
                serde_json::json!({ "cwd": data_root.path() }),
            )
            .await["result"]
                .clone(),
        )?;
    assert!(!read.preferences.available_models.is_empty());
    assert!(!read.preferences.available_efforts.is_empty());
    let written: devo_protocol::native::rpc_admin::ModelPreferencesWriteResult =
        serde_json::from_value(
            history_request(
                &runtime,
                connection_id,
                9,
                "model/preferences/write",
                serde_json::json!({
                    "cwd": data_root.path(),
                    "patch": { "reasoningEffort": "high" },
                }),
            )
            .await["result"]
                .clone(),
        )?;
    assert_eq!(
        written.preferences.reasoning_effort.as_deref(),
        Some("high")
    );
    let rejected = history_request(
        &runtime,
        connection_id,
        10,
        "model/preferences/write",
        serde_json::json!({
            "cwd": data_root.path(),
            "patch": { "model": "no-such-model" },
        }),
    )
    .await;
    assert!(rejected.get("error").is_some());
    Ok(())
}

#[tokio::test]
async fn native_admin_directory_lists() -> Result<()> {
    let data_root = TempDir::new()?;
    let catalog = Arc::new(PresetModelCatalog::load_from_provider_config(
        &devo_core::ProviderConfigFile::default(),
    )?);
    let runtime = build_runtime_with_provider_and_catalog(
        data_root.path(),
        Arc::new(NoopProvider::failing()),
        catalog,
    );
    let connection_id = initialized_with_protocol_meta(&runtime, true).await;
    let listed: devo_protocol::native::rpc_admin::ProviderListResult = json_result(
        &history_request(
            &runtime,
            connection_id,
            7,
            "provider/list",
            serde_json::json!({}),
        )
        .await,
        "rpc",
    );
    assert!(
        listed
            .providers
            .iter()
            .any(|provider| provider.id == "deepseek")
    );
    assert!(listed.template_provider_ids.contains(&"zhipu".to_string()));

    let env = NativeEnv::native().await?;
    let listed: devo_protocol::native::rpc_admin::SkillListResult = json_result(
        &env.rpc(8, "skill/list", serde_json::json!({ "cwd": env.cwd() }))
            .await,
        "rpc",
    );
    assert!(listed.skills.iter().all(|skill| !skill.id.is_empty()));
    let noop: devo_protocol::native::rpc_admin::SkillSetEnabledResult = json_result(
        &env.rpc(
            9,
            "skill/set_enabled",
            serde_json::json!({
                "path": env.cwd().join("no-such-skill/SKILL.md"),
                "enabled": true,
                "cwd": env.cwd(),
            }),
        )
        .await,
        "rpc",
    );
    assert_eq!(noop.skills.len(), listed.skills.len());

    let listed: devo_protocol::native::rpc_admin::ProviderListResult = json_result(
        &env.rpc(10, "provider/list", serde_json::json!({})).await,
        "rpc",
    );
    assert!(listed.providers.is_empty());
    assert!(
        env.rpc(
            11,
            "provider/model/remove",
            serde_json::json!({
                "providerId": "test-provider", "modelId": "test-model",
            })
        )
        .await
        .get("error")
        .is_some()
    );
    assert!(
        env.rpc(
            12,
            "provider/upsert",
            serde_json::json!({
                "provider": {
                    "name": "test-provider",
                    "baseUrl": "https://example.com/v1",
                    "wireApis": ["openai_chat_completions"],
                    "enabled": true,
                    "models": { "test-model": { "name": "Test model" } },
                },
                "defaultModel": "test-provider/test-model",
            })
        )
        .await
        .get("error")
        .is_none()
    );
    let listed = env.rpc(13, "provider/list", serde_json::json!({})).await;
    assert_eq!(
        listed["result"]["connectedProviderIds"],
        serde_json::json!(["test-provider"])
    );
    assert!(
        env.rpc(
            14,
            "provider/model/remove",
            serde_json::json!({
                "providerId": "test-provider", "modelId": "test-model",
            })
        )
        .await
        .get("error")
        .is_none()
    );
    assert!(
        env.rpc(
            15,
            "provider/disconnect",
            serde_json::json!({ "providerId": "test-provider" })
        )
        .await
        .get("error")
        .is_none()
    );
    Ok(())
}

/// A credential-only overlay must inherit the bundled protocol and URL, and
/// credential/set must refresh routers held by already-live sessions.
#[tokio::test]
async fn native_oauth_credential_set_keeps_builtin_protocol_and_refreshes_live_routers()
-> Result<()> {
    use devo_protocol::ProviderWireApi;
    use devo_provider::ProviderRoute;

    let data_root = TempDir::new()?;
    let runtime = build_runtime_with_provider_and_catalog(
        data_root.path(),
        Arc::new(NoopProvider::failing()),
        Arc::new(PresetModelCatalog::load()?),
    );
    let connection_id = initialized_with_protocol_meta(&runtime, true).await;
    let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
    let session_context = runtime
        .session(session_id)
        .await
        .expect("live session")
        .runtime_context()
        .await
        .expect("workspace context");
    let process_context = Arc::clone(&runtime.deps.process_context);
    let process_before = process_context.provider_router();
    let session_before = session_context.provider_router();

    for (index, provider) in ["anthropic", "openai-codex"].into_iter().enumerate() {
        let response = history_request(
            &runtime,
            connection_id,
            900 + index as u64,
            "credential/set",
            serde_json::json!({
                "provider": provider,
                "kind": "oauth",
                "access": "local-test-token",
                "expiresAt": Utc::now().timestamp() + 86_400,
            }),
        )
        .await;
        assert!(
            response.get("error").is_none(),
            "credential/set failed: {response}"
        );
    }

    let listed: devo_protocol::native::rpc_admin::ProviderListResult = json_result(
        &history_request(
            &runtime,
            connection_id,
            902,
            "provider/list",
            serde_json::json!({}),
        )
        .await,
        "provider/list",
    );
    for (provider_id, base_url, wire_api) in [
        (
            "anthropic",
            "https://api.anthropic.com",
            ProviderWireApi::AnthropicMessages,
        ),
        (
            "openai-codex",
            "https://chatgpt.com/backend-api/codex",
            ProviderWireApi::OpenAIResponses,
        ),
    ] {
        let entry = listed
            .providers
            .iter()
            .find(|entry| entry.id == provider_id)
            .expect("bundled provider");
        assert_eq!(
            (
                entry.base_url.as_deref(),
                entry.wire_apis.as_slice(),
                entry.credential.as_deref()
            ),
            (Some(base_url), &[wire_api][..], Some(provider_id)),
        );
        assert!(
            !entry.models.is_empty(),
            "bundled models must remain visible"
        );
    }
    assert_eq!(
        listed.connected_provider_ids,
        vec!["anthropic", "openai-codex"]
    );
    assert!(
        !Arc::ptr_eq(&process_before, &process_context.provider_router()),
        "the process router must be replaced after credential/set"
    );
    assert!(
        !Arc::ptr_eq(&session_before, &session_context.provider_router()),
        "a live session router must be replaced after credential/set"
    );
    for context in [&process_context, &session_context] {
        let configured = context
            .config_store
            .lock()
            .expect("config store")
            .effective_config()
            .provider_catalog_config();
        assert_eq!(
            configured
                .providers
                .get("openai-codex")
                .and_then(|entry| entry.credential.as_deref()),
            Some("openai-codex"),
        );
        let router = context.reload_provider_router().await?;
        assert!(router.has_route(&ProviderRoute::connection(
            "anthropic",
            ProviderWireApi::AnthropicMessages
        )));
        assert!(router.has_route(&ProviderRoute::connection(
            "openai-codex",
            ProviderWireApi::OpenAIResponses
        )));
        assert!(!router.has_route(&ProviderRoute::connection(
            "anthropic",
            ProviderWireApi::OpenAIChatCompletions
        )));
    }

    // An explicit user override beats the builtin; an explicit custom-provider
    // override beats the user connection without dropping its OAuth binding.
    let provider_path = data_root.path().join(devo_core::PROVIDER_CONFIG_FILE_NAME);
    let mut user = devo_core::read_provider_catalog_config(&provider_path)?;
    user.providers
        .get_mut("anthropic")
        .expect("user connection")
        .base_url = Some("https://user.example/messages".to_string());
    devo_core::write_provider_catalog_config(&provider_path, &user)?;
    runtime.deps.config_store.lock().expect("store").reload()?;
    let listed: devo_protocol::native::rpc_admin::ProviderListResult = json_result(
        &history_request(
            &runtime,
            connection_id,
            903,
            "provider/list",
            serde_json::json!({}),
        )
        .await,
        "provider/list user override",
    );
    let anth = listed
        .providers
        .iter()
        .find(|entry| entry.id == "anthropic")
        .expect("anthropic");
    assert_eq!(
        (anth.base_url.as_deref(), anth.wire_apis.as_slice()),
        (
            Some("https://user.example/messages"),
            &[ProviderWireApi::AnthropicMessages][..]
        ),
    );

    devo_core::write_provider_catalog_config(
        &data_root
            .path()
            .join(devo_core::CUSTOM_PROVIDER_CONFIG_FILE_NAME),
        &devo_core::ProviderConfigFile {
            providers: std::collections::BTreeMap::from([(
                "anthropic".to_string(),
                devo_core::ProviderConfigEntry {
                    base_url: Some("https://custom.example/messages".to_string()),
                    wire_api: Some(ProviderWireApi::OpenAIResponses),
                    ..devo_core::ProviderConfigEntry::default()
                },
            )]),
            ..devo_core::ProviderConfigFile::default()
        },
    )?;
    runtime.deps.config_store.lock().expect("store").reload()?;
    let listed: devo_protocol::native::rpc_admin::ProviderListResult = json_result(
        &history_request(
            &runtime,
            connection_id,
            904,
            "provider/list",
            serde_json::json!({}),
        )
        .await,
        "provider/list custom override",
    );
    let anth = listed
        .providers
        .iter()
        .find(|entry| entry.id == "anthropic")
        .expect("anthropic");
    assert_eq!(
        (
            anth.base_url.as_deref(),
            anth.wire_apis.as_slice(),
            anth.credential.as_deref()
        ),
        (
            Some("https://custom.example/messages"),
            &[ProviderWireApi::OpenAIResponses][..],
            Some("anthropic")
        ),
    );

    let credentials: devo_protocol::native::rpc_admin::CredentialListResult = json_result(
        &history_request(
            &runtime,
            connection_id,
            905,
            "credential/list",
            serde_json::json!({}),
        )
        .await,
        "credential/list",
    );
    assert_eq!(
        credentials
            .credentials
            .iter()
            .map(|credential| credential.id.as_str())
            .collect::<Vec<_>>(),
        vec!["anthropic", "openai-codex"],
    );
    assert!(
        credentials
            .credentials
            .iter()
            .all(|credential| credential.masked != "local-test-token")
    );

    let process_before_delete = process_context.provider_router();
    let session_before_delete = session_context.provider_router();
    let deleted = history_request(
        &runtime,
        connection_id,
        906,
        "credential/delete",
        serde_json::json!({ "credentialId": "anthropic" }),
    )
    .await;
    assert!(
        deleted.get("error").is_none(),
        "credential/delete failed: {deleted}"
    );
    assert!(!Arc::ptr_eq(
        &process_before_delete,
        &process_context.provider_router()
    ));
    assert!(!Arc::ptr_eq(
        &session_before_delete,
        &session_context.provider_router()
    ));
    let credentials: devo_protocol::native::rpc_admin::CredentialListResult = json_result(
        &history_request(
            &runtime,
            connection_id,
            907,
            "credential/list",
            serde_json::json!({}),
        )
        .await,
        "credential/list after delete",
    );
    assert_eq!(
        credentials
            .credentials
            .iter()
            .map(|credential| credential.id.as_str())
            .collect::<Vec<_>>(),
        vec!["openai-codex"],
    );
    assert!(
        !devo_core::read_user_auth_config(
            &data_root.path().join(devo_core::AUTH_CONFIG_FILE_NAME)
        )?
        .credentials
        .contains_key("anthropic")
    );
    Ok(())
}

#[tokio::test]
async fn native_session_message_edit_branches() -> Result<()> {
    let env = NativeEnv::native().await?;
    let session_id = start_durable_session(&env.runtime, env.connection_id, env.cwd()).await?;
    env.rpc(
        7,
        "turn/start",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "input": [{ "type": "text", "text": "original message" }],
            "idempotencyKey": "edit-target-turn",
        }),
    )
    .await;
    wait_turn_idle(&env.runtime, session_id).await;
    let items: devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope> =
        json_result(
            &env.rpc(
                8,
                "session/items/list",
                serde_json::json!({ "sessionId": session_id.to_string() }),
            )
            .await,
            "rpc",
        );
    let user_item = items
        .data
        .iter()
        .find(|item| {
            matches!(
                &item.item,
                devo_protocol::native::item::Item::UserMessage { .. }
            )
        })
        .expect("user message item");
    let edited: devo_protocol::native::rpc_session::SessionMessageEditResult = json_result(
        &env.rpc(
            9,
            "session/message/edit",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "itemId": user_item.id.as_str(),
                "expectedRevision": user_item.revision,
                "content": [{ "type": "text", "text": "edited message" }],
                "idempotencyKey": "edit-1",
            }),
        )
        .await,
        "rpc",
    );
    assert_eq!(edited.item.revision, user_item.revision + 1);
    assert_eq!(
        edited.edit_state,
        devo_protocol::native::rpc_session::MessageEditState::Accepted
    );
    assert!(edited.replacement_turn_id.is_some());
    assert_eq!(
        env.rpc(
            10,
            "session/message/edit",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "itemId": user_item.id.as_str(),
                "expectedRevision": 999,
                "content": [{ "type": "text", "text": "stale edit" }],
                "idempotencyKey": "edit-2",
            }),
        )
        .await["error"]["code"]
            .as_str(),
        Some("WORKSPACE_VERSION_CONFLICT")
    );

    let data_root = TempDir::new()?;
    let started = Arc::new(tokio::sync::Notify::new());
    let runtime = build_runtime_with_provider(
        data_root.path(),
        Arc::new(HangTurnStreamProvider {
            started: Arc::clone(&started),
        }),
    );
    let connection_id = initialized_with_protocol_meta(&runtime, true).await;
    let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
    assert!(
        history_request(
            &runtime,
            connection_id,
            7,
            "turn/start",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "original message" }],
                "idempotencyKey": "edit-active-turn",
            }),
        )
        .await
        .get("error")
        .is_none()
    );
    tokio::time::timeout(Duration::from_secs(5), started.notified()).await?;
    let items: devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope> =
        json_result(
            &history_request(
                &runtime,
                connection_id,
                8,
                "session/items/list",
                serde_json::json!({ "sessionId": session_id.to_string() }),
            )
            .await,
            "rpc",
        );
    let user_item = items
        .data
        .into_iter()
        .find(|item| {
            matches!(
                &item.item,
                devo_protocol::native::item::Item::UserMessage { .. }
            )
        })
        .expect("user message item");
    let edited: devo_protocol::native::rpc_session::SessionMessageEditResult = json_result(
        &history_request(
            &runtime,
            connection_id,
            9,
            "session/message/edit",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "itemId": user_item.id.as_str(),
                "expectedRevision": 0,
                "content": [{ "type": "text", "text": "edited while running" }],
                "workspaceRestore": "skip",
                "idempotencyKey": "edit-while-active",
            }),
        )
        .await,
        "rpc",
    );
    assert_eq!(
        edited.edit_state,
        devo_protocol::native::rpc_session::MessageEditState::Accepted
    );
    assert!(edited.replacement_turn_id.is_some());
    Ok(())
}

struct HangTurnStreamProvider {
    started: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl ModelProviderSDK for HangTurnStreamProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title".into(),
            content: vec![devo_protocol::ResponseContent::Text("title".into())],
            stop_reason: Some(devo_protocol::StopReason::EndTurn),
            usage: devo_protocol::Usage::default(),
            metadata: devo_protocol::ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        self.started.notify_one();
        Ok(Box::pin(futures::stream::once(async {
            std::future::pending::<()>().await;
            unreachable!("hanging turn stream should be canceled by interrupt")
        })))
    }

    fn name(&self) -> &str {
        "hang-turn-stream"
    }
}

#[tokio::test]
async fn native_session_goal_and_fork_branches() -> Result<()> {
    let env = NativeEnv::connect().await?;
    let session_id = env.session().await?;
    let params = serde_json::json!({
        "sessionId": session_id.to_string(),
        "objective": "ship the protocol unification",
        "ifExists": "replace",
        "idempotencyKey": "goal-set-1",
    });
    let created: devo_protocol::native::rpc_session::SessionGoalSetResult =
        json_result(&env.rpc(7, "session/goal/set", params.clone()).await, "rpc");
    assert_eq!(created.goal.objective, "ship the protocol unification");
    let replay: devo_protocol::native::rpc_session::SessionGoalSetResult = json_result(
        &history_request(
            &env.runtime,
            env.connection_id,
            8,
            "session/goal/set",
            params,
        )
        .await,
        "rpc",
    );
    assert_eq!(replay.goal, created.goal);
    let read: devo_protocol::native::rpc_session::SessionGoalReadResult = json_result(
        &env.rpc(
            9,
            "session/goal/read",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await,
        "rpc",
    );
    // The live goal may have advanced past `created` while this test ran:
    // an Active goal schedules a continuation turn whose completion accounts
    // usage into the store (updated_at bump). Identity and content must
    // match; only the accounting-written fields may move.
    let read_goal = read.goal.expect("goal survives idempotent replay");
    assert_eq!(read_goal.id, created.goal.id);
    assert_eq!(read_goal.objective, created.goal.objective);
    assert_eq!(read_goal.status, created.goal.status);
    assert!(read_goal.updated_at >= created.goal.updated_at);
    assert!(
        env.rpc(
            10,
            "session/goal/set",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "objective": "a different objective",
                "ifExists": "reject",
                "idempotencyKey": "goal-set-2",
            }),
        )
        .await
        .get("error")
        .is_some()
    );

    let update_params = serde_json::json!({
        "sessionId": session_id.to_string(),
        "expectedGoalId": created.goal.id.as_str(),
        "patch": {
            "objective": "edited objective",
            "status": "paused",
            "tokenBudget": 5000,
        },
        "idempotencyKey": "goal-update-1",
    });
    let updated: devo_protocol::native::rpc_session::SessionGoalUpdateResult = json_result(
        &env.rpc(11, "session/goal/update", update_params.clone())
            .await,
        "rpc",
    );
    assert_eq!(updated.goal.id, created.goal.id);
    assert_eq!(updated.goal.objective, "edited objective");
    assert!(
        env.rpc(
            12,
            "session/goal/update",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "patch": { "status": "budgetLimited" },
                "idempotencyKey": "goal-update-2",
            }),
        )
        .await
        .get("error")
        .is_some()
    );
    let replay: devo_protocol::native::rpc_session::SessionGoalUpdateResult = json_result(
        &env.rpc(13, "session/goal/update", update_params).await,
        "rpc",
    );
    assert_eq!(replay.goal, updated.goal);
    let no_goal_session =
        start_durable_session(&env.runtime, env.connection_id, &env.cwd().join("empty")).await?;
    assert_eq!(
        env.rpc(
            14,
            "session/goal/update",
            serde_json::json!({
                "sessionId": no_goal_session.to_string(),
                "patch": { "objective": "x" },
                "idempotencyKey": "goal-update-3",
            }),
        )
        .await["error"]["code"]
            .as_str(),
        Some("GoalNotFound")
    );

    let forked: devo_protocol::native::rpc_session::SessionForkResult = json_result(
        &env.rpc(
            15,
            "session/fork",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await,
        "rpc",
    );
    assert_ne!(forked.session.id.as_str(), session_id.to_string());
    assert_eq!(
        forked.session.fork_from_id.as_ref().map(|id| id.as_str()),
        Some(session_id.to_string().as_str())
    );
    assert!(
        env.rpc(
            16,
            "session/fork",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "atTurnId": "turn_00000000-0000-0000-0000-000000000000",
            }),
        )
        .await
        .get("error")
        .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn native_task_start_agent_spawns_child_session() -> Result<()> {
    let env = NativeEnv::connect().await?;
    let session_id = env.session().await?;

    let params = serde_json::json!({
        "kind": "agent",
        "sessionId": session_id.to_string(),
        "input": [{ "type": "text", "text": "quick question" }],
        "forkTurns": "all",
        "maxTurns": 1,
        "toolPolicy": "deny_all",
        "ephemeral": true,
        "idempotencyKey": "agent-task-1",
    });
    let started = history_request(
        &env.runtime,
        env.connection_id,
        7,
        "task/start",
        params.clone(),
    )
    .await;
    let started: devo_protocol::native::rpc_turn::TaskStartResult = json_result(&started, "rpc");
    assert!(
        started.item_id.as_str().starts_with("item_"),
        "agent item id must be item_-prefixed: {}",
        started.item_id.as_str()
    );

    let replay = history_request(&env.runtime, env.connection_id, 8, "task/start", params).await;
    let replay: devo_protocol::native::rpc_turn::TaskStartResult = json_result(&replay, "rpc");
    assert_eq!(replay.item_id, started.item_id);

    // The spawned child is visible through native agent/list.
    let listed = env
        .rpc(
            9,
            "agent/list",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
    let listed: devo_protocol::native::rpc_turn::AgentListResult = json_result(&listed, "rpc");
    assert_eq!(listed.agents.len(), 1, "spawned agent must be listed");

    let _durable = env
        .rpc(
            10,
            "task/start",
            serde_json::json!({
                "kind": "agent",
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "durable child" }],
                "forkTurns": "none",
                "maxTurns": 1,
                "toolPolicy": "deny_all",
                "ephemeral": false,
                "idempotencyKey": "agent-task-durable",
            }),
        )
        .await;
    let listed = env
        .rpc(
            11,
            "agent/list",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
    let listed: devo_protocol::native::rpc_turn::AgentListResult = json_result(&listed, "rpc");
    let child_session_id = listed
        .agents
        .iter()
        .rev()
        .find_map(|envelope| match &envelope.item {
            devo_protocol::native::item::Item::SubAgent {
                agent_session_id, ..
            } => {
                let id = agent_session_id;
                env.runtime
                    .deps_db()
                    .get_session_index(id)
                    .ok()
                    .flatten()
                    .and_then(|row| row.rollout_path.map(|_| id))
            }
            _ => None,
        })
        .expect("durable child session id");
    let child_index = env
        .runtime
        .deps_db()
        .get_session_index(child_session_id)
        .expect("db")
        .expect("child indexed");
    let child_rollout = child_index
        .rollout_path
        .expect("durable child must have rollout_path");
    let rel = child_rollout
        .strip_prefix(env.cwd())
        .expect("child under data root");
    let parts: Vec<_> = rel
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert_eq!(parts[0], "session-artifacts");
    assert_eq!(parts[1], session_id.to_string());
    assert!(
        parts[2].starts_with("sub-") && parts[2].len() == 12,
        "expected sub-xxxxxxxx, got {}",
        parts[2]
    );
    assert_eq!(parts[3], format!("{child_session_id}.jsonl"));
    Ok(())
}

#[tokio::test]
async fn child_agent_message_to_idle_parent_starts_wake_turn() -> Result<()> {
    // Regression: child→parent agent messages must start a wake turn on an
    // IDLE parent (kernel_host_agent_message::deliver_family_agent_message).
    // Children's plain final text never wakes the parent — only an explicit
    // agent_message.send does; that path had no coverage at all.
    let env = NativeEnv::connect().await?;
    let parent_session_id = env.session().await?;

    let started = env
        .rpc(
            7,
            "task/start",
            serde_json::json!({
                "kind": "agent",
                "sessionId": parent_session_id.to_string(),
                "input": [{ "type": "text", "text": "quick question" }],
                "forkTurns": "all",
                "maxTurns": 1,
                "toolPolicy": "deny_all",
                "ephemeral": true,
                "idempotencyKey": "wake-child-1",
            }),
        )
        .await;
    let started: devo_protocol::native::rpc_turn::TaskStartResult = json_result(&started, "rpc");
    assert!(started.item_id.as_str().starts_with("item_"));

    let listed = env
        .rpc(
            8,
            "agent/list",
            serde_json::json!({ "sessionId": parent_session_id.to_string() }),
        )
        .await;
    let listed: devo_protocol::native::rpc_turn::AgentListResult = json_result(&listed, "rpc");
    let child_session_id = listed
        .agents
        .iter()
        .find_map(|envelope| match &envelope.item {
            devo_protocol::native::item::Item::SubAgent {
                agent_session_id, ..
            } => Some(*agent_session_id),
            _ => None,
        })
        .expect("spawned child session id");
    // The failing provider errors the child turn quickly; wait for the parent
    // to be idle again so the wake path exercises the idle branch.
    wait_turn_idle(&env.runtime, parent_session_id).await;

    let turns_before = session_turns_json(&env.runtime, env.connection_id, parent_session_id).await;
    let receipt = env
        .runtime
        .host_agent_message_send(
            child_session_id.to_string().as_str(),
            &serde_json::json!({
                "message": "wake probe from child",
                "receiver_role": "parent",
            }),
        )
        .await;
    assert_eq!(receipt["status"], serde_json::json!("ok"));
    assert_eq!(receipt["delivered"], serde_json::json!(true));
    assert_eq!(
        receipt["parentSessionId"],
        serde_json::json!(parent_session_id.to_string())
    );

    // The wake turn must materialize: the injected user message carries the
    // canonical "Agent message received" envelope, and the turn count grows.
    let rollout_path = env
        .runtime
        .rollout_store
        .find_rollout_by_session_id(&parent_session_id)?
        .expect("parent rollout exists");
    let mut wake_seen = false;
    for _ in 0..100 {
        let journal = std::fs::read_to_string(&rollout_path)?;
        if journal.contains("Agent message received") {
            wake_seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        wake_seen,
        "child agent_message.send must inject a wake turn on the idle parent"
    );
    wait_turn_idle(&env.runtime, parent_session_id).await;
    let turns_after = session_turns_json(&env.runtime, env.connection_id, parent_session_id).await;
    assert!(
        turns_after.len() > turns_before.len(),
        "wake turn must appear in the parent's turn list"
    );
    Ok(())
}

#[tokio::test]
async fn resume_hydrates_child_sessions_for_approval_recovery() -> Result<()> {
    // Regression: children load lazily (only when a parent tool awaits them),
    // so after a server restart a child turn parked on an approval checkpoint
    // never re-registered its waiting-approval lane and the approval was never
    // reissued — the parent flow waiting on the child stayed stuck forever.
    // Resuming the root must hydrate its children.
    let (env, open, started) = NativeEnv::gated().await?;
    let parent_session_id = env.session().await?;

    let response = env
        .rpc(
            1,
            "turn/start",
            serde_json::json!({
                "sessionId": parent_session_id.to_string(),
                "input": [{ "type": "text", "text": "parent blocks in provider" }],
                "idempotencyKey": "hydrate-children-parent-1",
            }),
        )
        .await;
    assert!(
        response.get("error").is_none(),
        "turn/start failed: {response}"
    );
    wait_flag(&started, "parent turn should reach the provider").await;

    let started_child = env
        .rpc(
            2,
            "task/start",
            serde_json::json!({
                "kind": "agent",
                "sessionId": parent_session_id.to_string(),
                "input": [{ "type": "text", "text": "child work" }],
                "forkTurns": "all",
                "maxTurns": 1,
                "toolPolicy": "deny_all",
                "idempotencyKey": "hydrate-children-child-1",
            }),
        )
        .await;
    let started_child: devo_protocol::native::rpc_turn::TaskStartResult =
        json_result(&started_child, "task/start");
    assert!(started_child.item_id.as_str().starts_with("item_"));

    let listed = env
        .rpc(
            3,
            "agent/list",
            serde_json::json!({ "sessionId": parent_session_id.to_string() }),
        )
        .await;
    let listed: devo_protocol::native::rpc_turn::AgentListResult =
        json_result(&listed, "agent/list");
    let child_session_id = listed
        .agents
        .iter()
        .find_map(|envelope| match &envelope.item {
            devo_protocol::native::item::Item::SubAgent {
                agent_session_id, ..
            } => Some(*agent_session_id),
            _ => None,
        })
        .expect("spawned child session id");
    let mut child_resident = false;
    for _ in 0..200 {
        if env.runtime.session(child_session_id).await.is_some() {
            child_resident = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        child_resident,
        "child session should be resident after spawn"
    );
    let index = env
        .runtime
        .deps
        .db
        .get_session_index(&child_session_id)
        .expect("child db index read")
        .expect("child session should be durably indexed");
    assert_eq!(index.session.parent_session_id, Some(parent_session_id));

    // Let both turns finish, then simulate a fresh process that has only the
    // parent resident: evict the child handle from the runtime map.
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut drained = false;
    for _ in 0..400 {
        let parent_active = env
            .runtime
            .runtime_active_turn_id(parent_session_id)
            .await
            .is_some();
        let child_active = env
            .runtime
            .runtime_active_turn_id(child_session_id)
            .await
            .is_some();
        if !parent_active && !child_active {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        drained,
        "parent and child turns should finish after the gate opens"
    );

    env.runtime.sessions.lock().await.remove(&child_session_id);
    assert!(
        env.runtime.session(child_session_id).await.is_none(),
        "child should be evicted before the recovery call"
    );

    env.runtime
        .hydrate_child_sessions_for_approval_recovery(parent_session_id)
        .await;

    assert!(
        env.runtime.session(child_session_id).await.is_some(),
        "resume must re-hydrate the child so its waiting-approval lane can register"
    );

    Ok(())
}

#[tokio::test]
async fn hydrated_child_regains_agent_registry_parent_linkage() -> Result<()> {
    // Regression: AgentRegistry linkage is spawn-time in-memory state. After a
    // restart (simulated here by dropping the parent registry and the child
    // actor), a lazily re-hydrated child had no parent linkage —
    // agent_message.send(receiver_role="parent") failed with "no parent for
    // this session", the child's report was lost, and a parent awaiting the
    // child hung forever. Hydration must rebuild the registry entry from
    // durable state with the SAME identity the spawn created.
    let env = NativeEnv::connect().await?;
    let parent_session_id = env.session().await?;

    let started_child = env
        .rpc(
            2,
            "task/start",
            serde_json::json!({
                "kind": "agent",
                "sessionId": parent_session_id.to_string(),
                "input": [{ "type": "text", "text": "child work" }],
                "forkTurns": "all",
                "maxTurns": 1,
                "toolPolicy": "deny_all",
                "idempotencyKey": "registry-restore-child-1",
            }),
        )
        .await;
    let started_child: devo_protocol::native::rpc_turn::TaskStartResult =
        json_result(&started_child, "task/start");
    assert!(started_child.item_id.as_str().starts_with("item_"));

    let listed = env
        .rpc(
            3,
            "agent/list",
            serde_json::json!({ "sessionId": parent_session_id.to_string() }),
        )
        .await;
    let listed: devo_protocol::native::rpc_turn::AgentListResult =
        json_result(&listed, "agent/list");
    let child_session_id = listed
        .agents
        .iter()
        .find_map(|envelope| match &envelope.item {
            devo_protocol::native::item::Item::SubAgent {
                agent_session_id, ..
            } => Some(*agent_session_id),
            _ => None,
        })
        .expect("spawned child session id");
    let mut child_resident = false;
    for _ in 0..200 {
        if env.runtime.session(child_session_id).await.is_some() {
            child_resident = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        child_resident,
        "child session should be resident after spawn"
    );
    wait_turn_idle(&env.runtime, child_session_id).await;

    let spawn_linkage = env
        .runtime
        .child_parent_and_path(child_session_id)
        .await
        .expect("spawn-time registry linkage");
    assert_eq!(spawn_linkage.0, parent_session_id);

    // Simulate a fresh process: the in-memory registry and the child actor
    // are gone; only durable state (rollout + db index) remains.
    env.runtime
        .agent_registries
        .lock()
        .await
        .remove(&parent_session_id);
    env.runtime.sessions.lock().await.remove(&child_session_id);
    assert!(
        env.runtime
            .child_parent_and_path(child_session_id)
            .await
            .is_none(),
        "registry drop must remove the linkage"
    );

    // Lazy re-hydration (the same path resume, Agents View, and approval
    // recovery all take) must rebuild the linkage with the spawn identity.
    env.runtime
        .get_or_load_parent_session(child_session_id)
        .await
        .expect("child re-hydrates");

    let restored = env
        .runtime
        .child_parent_and_path(child_session_id)
        .await
        .expect("re-hydrated child must resolve its parent again");
    assert_eq!(restored, spawn_linkage);

    // The rebuilt entry must also answer name-based lookups the RLM surface
    // uses (find_child matches on agent_path / nickname).
    let resolved = env
        .runtime
        .resolve_child_agent(parent_session_id, &restored.1)
        .await
        .expect("find_child resolves the restored agent by path");
    assert_eq!(resolved.session_id, child_session_id);
    assert_eq!(resolved.parent_session_id, parent_session_id);

    Ok(())
}

#[tokio::test]
async fn interrupting_parent_clears_child_active_turn_registry() -> Result<()> {
    // Regression: `interrupt_child_runtime_work` aborted the child's turn
    // task immediately after cancelling it, racing the task's own unwind.
    // When the abort won, the leaked ActiveTurnRegistry entry kept
    // projecting the interrupted child as active — session/read reported
    // status "active"/"working" with the interrupted turn's id forever, and
    // the TUI subagents panel stayed at "N running". The cascade must let
    // the child unwind (or clean the registry up itself).
    let (env, _open, started) = NativeEnv::gated().await?;
    let parent_session_id = env.session().await?;

    // Parent turn blocks in the gated provider.
    let response = env
        .rpc(
            1,
            "turn/start",
            serde_json::json!({
                "sessionId": parent_session_id.to_string(),
                "input": [{ "type": "text", "text": "parent blocks in provider" }],
                "idempotencyKey": "cascade-parent-1",
            }),
        )
        .await;
    assert!(
        response.get("error").is_none(),
        "turn/start failed: {response}"
    );
    wait_flag(&started, "parent turn should reach the provider").await;
    assert!(
        env.runtime
            .runtime_active_turn_id(parent_session_id)
            .await
            .is_some(),
        "parent turn should be active"
    );

    // Spawn a child; its turn also blocks in the gated provider.
    let started_child = env
        .rpc(
            2,
            "task/start",
            serde_json::json!({
                "kind": "agent",
                "sessionId": parent_session_id.to_string(),
                "input": [{ "type": "text", "text": "child blocks in provider" }],
                "forkTurns": "all",
                "maxTurns": 1,
                "toolPolicy": "deny_all",
                "ephemeral": true,
                "idempotencyKey": "cascade-child-1",
            }),
        )
        .await;
    let started_child: devo_protocol::native::rpc_turn::TaskStartResult =
        json_result(&started_child, "task/start");
    assert!(started_child.item_id.as_str().starts_with("item_"));

    let listed = env
        .rpc(
            3,
            "agent/list",
            serde_json::json!({ "sessionId": parent_session_id.to_string() }),
        )
        .await;
    let listed: devo_protocol::native::rpc_turn::AgentListResult =
        json_result(&listed, "agent/list");
    let child_session_id = listed
        .agents
        .iter()
        .find_map(|envelope| match &envelope.item {
            devo_protocol::native::item::Item::SubAgent {
                agent_session_id, ..
            } => Some(*agent_session_id),
            _ => None,
        })
        .expect("spawned child session id");
    let mut child_active = false;
    for _ in 0..200 {
        if env
            .runtime
            .runtime_active_turn_id(child_session_id)
            .await
            .is_some()
        {
            child_active = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(child_active, "child turn should be active");

    // Interrupt the parent: the cascade must interrupt the child too.
    let response = env
        .rpc(
            4,
            "session/interrupt",
            serde_json::json!({
                "scope": { "scope": "session", "sessionId": parent_session_id.to_string() }
            }),
        )
        .await;
    assert!(
        response.get("error").is_none(),
        "interrupt failed: {response}"
    );

    // Both the parent AND the interrupted child must leave the active-turn
    // registry — a leaked entry here is what session/read projected as
    // "working" long after the interruption.
    assert!(
        env.runtime
            .runtime_active_turn_id(parent_session_id)
            .await
            .is_none(),
        "parent turn must leave the active-turn registry"
    );
    assert!(
        env.runtime
            .runtime_active_turn_id(child_session_id)
            .await
            .is_none(),
        "child turn registry entry must not leak after the cascade interrupt"
    );
    Ok(())
}

#[tokio::test]
async fn schedule_dispatch_hydrates_evicted_session_and_delivers() -> Result<()> {
    // Regression: the schedule wake loop runs detached from any client, so
    // the target session's actor may not be resident (LRU-evicted or never
    // loaded). Delivery used to fail with SessionNotFound forever while
    // claim_due kept advancing the job; it must hydrate on demand instead.
    let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let env = NativeEnv::with_provider(Arc::new(GatedProvider {
        open: Arc::clone(&open),
        started: Arc::clone(&started),
    }))
    .await?;
    let session_id = env.session().await?;

    // Evict the actor so delivery has to rehydrate from the rollout.
    if let Some(handle) = env.runtime.remove_session_actor(session_id).await {
        handle.shutdown().await;
    }
    assert!(
        env.runtime.session(session_id).await.is_none(),
        "precondition: session actor must be gone before dispatch"
    );

    let upsert: devo_protocol::native::rpc_schedule::SessionScheduleUpsertResult = json_result(
        &env.rpc(
            2,
            "session/schedule/upsert",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "kind": "heartbeat",
                "intervalMs": 1_u64,
                "instruction": "schedule hydration probe",
                "deliveryMode": "followUp",
            }),
        )
        .await,
        "session/schedule/upsert",
    );
    assert_eq!(upsert.job.run_count, 0);
    // next_run = creation + 1ms; let it become due.
    tokio::time::sleep(Duration::from_millis(10)).await;
    env.runtime.dispatch_due_schedule_jobs().await?;

    wait_flag(&started, "hydrated session must start its heartbeat turn").await;
    assert!(
        env.runtime
            .runtime_active_turn_id(session_id)
            .await
            .is_some(),
        "schedule delivery must start a turn on the rehydrated session"
    );
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    wait_turn_idle(&env.runtime, session_id).await;
    Ok(())
}

#[tokio::test]
async fn schedule_dispatch_retires_job_for_missing_session() -> Result<()> {
    // Regression: a schedule job whose session no longer exists used to
    // retry forever (SessionNotFound every interval, run_count climbing).
    // The wake loop must retire it.
    let env = NativeEnv::connect().await?;
    let missing = SessionId::new();
    let upsert: devo_protocol::native::rpc_schedule::SessionScheduleUpsertResult = json_result(
        &env.rpc(
            3,
            "session/schedule/upsert",
            serde_json::json!({
                "sessionId": missing.to_string(),
                "kind": "heartbeat",
                "intervalMs": 1_u64,
                "instruction": "doomed heartbeat",
                "deliveryMode": "followUp",
            }),
        )
        .await,
        "session/schedule/upsert",
    );
    tokio::time::sleep(Duration::from_millis(10)).await;
    env.runtime.dispatch_due_schedule_jobs().await?;
    let remaining = env.runtime.schedule_store.list(None, None)?;
    assert!(
        remaining.iter().all(|job| job.job_id != upsert.job.job_id),
        "job for missing session must be retired; store now: {remaining:?}"
    );
    Ok(())
}

#[tokio::test]
async fn schedule_dispatch_skips_fire_when_identical_prompt_already_queued() -> Result<()> {
    // Regression: a heartbeat interval shorter than the turn duration piles
    // identical fires onto the pending queue without bound (each drained
    // fire is itself a turn). A fire whose prompt is already queued is a
    // no-op nudge and must be skipped.
    let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let env = NativeEnv::with_provider(Arc::new(GatedProvider {
        open: Arc::clone(&open),
        started: Arc::clone(&started),
    }))
    .await?;
    let session_id = env.session().await?;

    // Seed the pending queue with the exact heartbeat text, as an earlier
    // fire queued behind a busy turn would have left it.
    let reservation = env
        .runtime
        .session_turn_reservation_snapshot(session_id)
        .await
        .expect("session actor snapshot");
    reservation
        .pending_turn_queue
        .lock()
        .expect("pending turn queue mutex should not be poisoned")
        .push_back(devo_core::PendingInputItem::new(
            devo_core::PendingInputKind::UserText {
                text: "dedup probe heartbeat".to_string(),
            },
            None,
            chrono::Utc::now(),
        ));

    env.rpc(
        4,
        "session/schedule/upsert",
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "kind": "heartbeat",
            "intervalMs": 1_u64,
            "instruction": "dedup probe heartbeat",
            "deliveryMode": "followUp",
        }),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    env.runtime.dispatch_due_schedule_jobs().await?;

    let queue_after = env
        .runtime
        .session_turn_reservation_snapshot(session_id)
        .await
        .expect("session actor snapshot after dispatch");
    let queue_len = queue_after
        .pending_turn_queue
        .lock()
        .expect("pending turn queue mutex should not be poisoned")
        .len();
    assert_eq!(
        queue_len, 1,
        "duplicate fire must not enqueue a second entry"
    );
    assert!(
        env.runtime
            .runtime_active_turn_id(session_id)
            .await
            .is_none(),
        "skipped fire must not start a turn either"
    );
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}
