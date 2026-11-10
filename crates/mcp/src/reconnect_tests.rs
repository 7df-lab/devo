use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

#[derive(Default)]
struct FakeState {
    starts: AtomicU64,
    lists: AtomicU64,
    calls: AtomicU64,
    block: AtomicBool,
    fail: AtomicBool,
    entered: Notify,
    release: Notify,
}

struct FakeServer {
    url: String,
    state: Arc<FakeState>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for FakeServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl FakeServer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let state = Arc::new(FakeState::default());
        let shared = Arc::clone(&state);
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let state = Arc::clone(&shared);
                tokio::spawn(async move {
                    let mut data = Vec::new();
                    let header_end = loop {
                        let mut buf = [0; 4096];
                        let n = stream.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        data.extend_from_slice(&buf[..n]);
                        if let Some(end) = data.windows(4).position(|s| s == b"\r\n\r\n") {
                            break end + 4;
                        }
                    };
                    let headers = String::from_utf8_lossy(&data[..header_end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    while data.len() < header_end + length {
                        let mut buf = [0; 4096];
                        let n = stream.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        data.extend_from_slice(&buf[..n]);
                    }
                    let request: Value =
                        serde_json::from_slice(&data[header_end..]).unwrap_or(Value::Null);
                    let result = match request["method"].as_str() {
                        Some("initialize") => {
                            state.starts.fetch_add(1, Ordering::SeqCst);
                            json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1"}})
                        }
                        Some("tools/list") => {
                            let number = state.lists.fetch_add(1, Ordering::SeqCst) + 1;
                            if state.block.load(Ordering::SeqCst) {
                                state.entered.notify_one();
                                state.release.notified().await;
                            }
                            json!({"tools":[{"name":format!("tool_{number}"),"description":"fake", "inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true},"_meta":{"ui":{"visibility":["model"]}}}]})
                        }
                        Some("tools/call") => {
                            state.calls.fetch_add(1, Ordering::SeqCst);
                            json!({"content":[]})
                        }
                        _ => Value::Null,
                    };
                    let (status, body) = if request["id"].is_null() {
                        ("202 Accepted", String::new())
                    } else {
                        let body = if state.fail.load(Ordering::SeqCst)
                            && request["method"] == "tools/list"
                        {
                            json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32603,"message":"fake failure"}})
                        } else {
                            json!({"jsonrpc":"2.0","id":request["id"],"result":result})
                        };
                        ("200 OK", body.to_string())
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        Self { url, state, task }
    }

    fn record(&self, id: &str) -> McpServerRecord {
        McpServerRecord {
            id: McpServerId(id.into()),
            display_name: id.into(),
            enabled: true,
            transport: McpTransportConfig::StreamableHttp {
                url: self.url.clone(),
                auth: None,
                http_headers: BTreeMap::new(),
                env_http_headers: BTreeMap::new(),
            },
            startup_policy: McpStartupPolicy::Eager,
            trust_policy: Default::default(),
            allowed_capabilities: vec![McpCapability::Tools],
            roots_policy: Default::default(),
            output_limits: Default::default(),
            auth_ref: None,
        }
    }
}

#[tokio::test]
async fn reconnect_replaces_transport_and_metadata_without_calls_or_sibling_refresh() {
    let server = FakeServer::start().await;
    let sibling = FakeServer::start().await;
    let manager = RmcpMcpManager::new(
        McpConfig {
            servers: vec![server.record("target"), sibling.record("sibling")],
            ..Default::default()
        },
        OAuthCredentialsStoreMode::File,
    );
    let id = McpServerId("target".into());
    manager.discover_tools().await.unwrap();
    let old_client = manager.clients.read().await[&id].clone();
    let sibling_tools = manager
        .cached_tools(&McpServerId("sibling".into()))
        .await
        .unwrap();
    let status = manager.reconnect(&id).await.unwrap();
    assert_eq!(
        (
            status.startup_state,
            server.state.starts.load(Ordering::SeqCst),
            server.state.lists.load(Ordering::SeqCst),
            server.state.calls.load(Ordering::SeqCst),
            sibling.state.starts.load(Ordering::SeqCst),
            sibling.state.lists.load(Ordering::SeqCst)
        ),
        (McpStartupState::Ready, 2, 2, 0, 1, 1)
    );
    assert!(!Arc::ptr_eq(
        &old_client,
        &manager.clients.read().await[&id]
    ));
    let tools = manager.cached_tools(&id).await.unwrap();
    assert_eq!(
        (
            tools[0].raw_tool_name.as_str(),
            tools[0].read_only_hint,
            tools[0].meta.clone()
        ),
        ("tool_2", true, Some(json!({"ui":{"visibility":["model"]}})))
    );
    assert_eq!(
        manager
            .cached_tools(&McpServerId("sibling".into()))
            .await
            .unwrap(),
        sibling_tools
    );
    server.state.fail.store(true, Ordering::SeqCst);
    assert!(manager.reconnect(&id).await.is_err());
    assert!(manager.cached_tools(&id).await.unwrap().is_empty());
    assert!(!manager.clients.read().await.contains_key(&id));
}

#[tokio::test]
async fn concurrent_reconnect_rejected_and_disable_wins_with_responsive_status() {
    let server = FakeServer::start().await;
    let manager = Arc::new(RmcpMcpManager::new(
        McpConfig {
            servers: vec![server.record("target")],
            ..Default::default()
        },
        OAuthCredentialsStoreMode::File,
    ));
    let id = McpServerId("target".into());
    manager.refresh(&id).await.unwrap();
    server.state.block.store(true, Ordering::SeqCst);
    let pending = {
        let manager = Arc::clone(&manager);
        let id = id.clone();
        tokio::spawn(async move { manager.reconnect(&id).await })
    };
    tokio::time::timeout(Duration::from_secs(3), server.state.entered.notified())
        .await
        .unwrap();
    assert!(manager.reconnect(&id).await.is_err());
    let refresh = manager.refresh(&id);
    tokio::pin!(refresh);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut refresh)
            .await
            .is_err()
    );
    let statuses = tokio::time::timeout(Duration::from_millis(100), manager.statuses())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        statuses,
        vec![empty_status(id.clone(), McpStartupState::Starting)]
    );
    manager.set_enabled(&id, /*enabled*/ false).await.unwrap();
    server.state.release.notify_one();
    assert!(pending.await.unwrap().is_err());
    assert!(refresh.await.is_err());
    assert_eq!(
        manager.statuses().await.unwrap(),
        vec![empty_status(id.clone(), McpStartupState::Disabled)]
    );
    assert!(manager.cached_tools(&id).await.unwrap().is_empty());
    assert!(manager.clients.read().await.is_empty());
    assert_eq!(server.state.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_registry_discovery_waits_for_complete_tool_metadata() {
    let server = FakeServer::start().await;
    let manager: Arc<dyn McpManager> = Arc::new(RmcpMcpManager::new(
        McpConfig {
            servers: vec![server.record("target")],
            ..Default::default()
        },
        OAuthCredentialsStoreMode::File,
    ));
    server.state.block.store(true, Ordering::SeqCst);
    let first = {
        let manager = Arc::clone(&manager);
        tokio::spawn(async move {
            devo_core::tools::handlers::build_registry_from_plan_with_mcp(
                &devo_core::tools::ToolPlanConfig::default(),
                manager,
            )
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(3), server.state.entered.notified())
        .await
        .unwrap();
    let plan = devo_core::tools::ToolPlanConfig::default();
    let second =
        devo_core::tools::handlers::build_registry_from_plan_with_mcp(&plan, Arc::clone(&manager));
    tokio::pin!(second);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut second)
            .await
            .is_err(),
        "A contending registry must not publish empty discovery"
    );
    server.state.release.notify_one();
    let first = first.await.unwrap();
    let second = tokio::time::timeout(Duration::from_secs(3), second)
        .await
        .unwrap();
    assert!(first.get("mcp__target__tool_1").is_some());
    assert_eq!(
        serde_json::to_value(first.tool_definitions()).unwrap(),
        serde_json::to_value(second.tool_definitions()).unwrap()
    );
    assert_eq!(
        (
            server.state.starts.load(Ordering::SeqCst),
            server.state.lists.load(Ordering::SeqCst),
            server.state.calls.load(Ordering::SeqCst),
        ),
        (1, 1, 0)
    );
}

#[tokio::test]
async fn cancelled_discovery_wakes_waiters_and_allows_explicit_retry() {
    let server = FakeServer::start().await;
    let manager = Arc::new(RmcpMcpManager::new(
        McpConfig {
            servers: vec![server.record("target")],
            ..Default::default()
        },
        OAuthCredentialsStoreMode::File,
    ));
    let id = McpServerId("target".into());
    server.state.block.store(true, Ordering::SeqCst);
    let pending = {
        let manager = Arc::clone(&manager);
        let id = id.clone();
        tokio::spawn(async move { manager.reconnect(&id).await })
    };
    tokio::time::timeout(Duration::from_secs(3), server.state.entered.notified())
        .await
        .unwrap();
    let refresh = manager.refresh(&id);
    tokio::pin!(refresh);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut refresh)
            .await
            .is_err()
    );
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    assert_eq!(
        manager.statuses().await.unwrap(),
        vec![empty_status(id.clone(), McpStartupState::Stopped)]
    );
    server.state.block.store(false, Ordering::SeqCst);
    server.state.release.notify_one();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), refresh)
            .await
            .unwrap()
            .unwrap()
            .startup_state,
        McpStartupState::Ready
    );
    assert_eq!(
        manager.reconnect(&id).await.unwrap().startup_state,
        McpStartupState::Ready
    );
    assert_eq!(server.state.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn configure_enable_is_network_free_and_obsolete_discovery_cannot_restore_tools() {
    let server = FakeServer::start().await;
    let manager = Arc::new(RmcpMcpManager::new(
        McpConfig {
            servers: vec![server.record("target")],
            ..Default::default()
        },
        OAuthCredentialsStoreMode::File,
    ));
    let id = McpServerId("target".into());
    manager
        .configure_enabled(&id, /*enabled*/ false)
        .await
        .unwrap();
    let configured = manager
        .configure_enabled(&id, /*enabled*/ true)
        .await
        .unwrap();
    assert_eq!(
        configured,
        empty_status(id.clone(), McpStartupState::NotStarted)
    );
    assert_eq!(server.state.starts.load(Ordering::SeqCst), 0);
    server.state.block.store(true, Ordering::SeqCst);
    let pending = {
        let manager = Arc::clone(&manager);
        let id = id.clone();
        tokio::spawn(async move { manager.refresh(&id).await })
    };
    tokio::time::timeout(Duration::from_secs(3), server.state.entered.notified())
        .await
        .unwrap();
    assert_eq!(
        manager
            .configure_enabled(&id, /*enabled*/ true)
            .await
            .unwrap(),
        empty_status(id.clone(), McpStartupState::Starting)
    );
    manager
        .configure_enabled(&id, /*enabled*/ false)
        .await
        .unwrap();
    manager
        .configure_enabled(&id, /*enabled*/ true)
        .await
        .unwrap();
    server.state.block.store(false, Ordering::SeqCst);
    server.state.release.notify_one();
    assert!(pending.await.unwrap().is_err());
    assert_eq!(
        manager.statuses().await.unwrap(),
        vec![empty_status(id.clone(), McpStartupState::NotStarted)]
    );
    assert!(manager.cached_tools(&id).await.unwrap().is_empty());
    assert!(manager.clients.read().await.is_empty());
    assert_eq!(
        manager.refresh(&id).await.unwrap().startup_state,
        McpStartupState::Ready
    );
}

#[tokio::test]
async fn explicitly_connected_manual_tools_survive_later_registry_discovery() {
    let server = FakeServer::start().await;
    let mut record = server.record("manual");
    record.startup_policy = McpStartupPolicy::Manual;
    let manager = Arc::new(RmcpMcpManager::new(
        McpConfig {
            servers: vec![record],
            auto_start: false,
        },
        OAuthCredentialsStoreMode::File,
    ));
    assert!(manager.discover_tools().await.unwrap().is_empty());
    let id = McpServerId("manual".into());
    server.state.block.store(true, Ordering::SeqCst);
    let pending = {
        let manager = Arc::clone(&manager);
        let id = id.clone();
        tokio::spawn(async move { manager.reconnect(&id).await })
    };
    tokio::time::timeout(Duration::from_secs(3), server.state.entered.notified())
        .await
        .unwrap();
    let discovery = manager.discover_tools();
    tokio::pin!(discovery);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut discovery)
            .await
            .is_err()
    );
    server.state.release.notify_one();
    pending.await.unwrap().unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), discovery)
            .await
            .unwrap()
            .unwrap(),
        manager.cached_tools(&id).await.unwrap()
    );
    assert_eq!(
        manager.discover_tools().await.unwrap(),
        manager.cached_tools(&id).await.unwrap()
    );
    assert_eq!(
        (
            server.state.starts.load(Ordering::SeqCst),
            server.state.lists.load(Ordering::SeqCst),
            server.state.calls.load(Ordering::SeqCst)
        ),
        (1, 1, 0)
    );
}
