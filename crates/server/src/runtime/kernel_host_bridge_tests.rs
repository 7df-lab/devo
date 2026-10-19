use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use serde_json::Value;
use serde_json::json;

use devo_core::McpAuthState;
use devo_core::McpError;
use devo_core::McpManager;
use devo_core::McpResourceDescriptor;
use devo_core::McpResourceTemplateDescriptor;
use devo_core::McpServerId;
use devo_core::McpServerStatus;
use devo_core::McpStartupState;
use devo_core::McpToolDescriptor;
use devo_core::McpToolInfo;
use devo_core::tools::{PermissionChecker, PermissionGrant};

use super::{HostBridge, handle_fs_read, handle_mcp_call, handle_mcp_list_tools};

struct FakeMcpManager {
    refreshed: Mutex<Vec<McpServerId>>,
    discoveries: AtomicUsize,
    invocations: AtomicUsize,
}

impl FakeMcpManager {
    fn new() -> Self {
        Self {
            refreshed: Mutex::new(Vec::new()),
            discoveries: AtomicUsize::new(0),
            invocations: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl McpManager for FakeMcpManager {
    async fn statuses(&self) -> Result<Vec<McpServerStatus>, McpError> {
        Ok(Vec::new())
    }

    async fn discover_tools(&self) -> Result<Vec<McpToolInfo>, McpError> {
        self.discoveries.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    }

    async fn refresh(&self, server_id: &McpServerId) -> Result<McpServerStatus, McpError> {
        self.refreshed.lock().unwrap().push(server_id.clone());
        Ok(McpServerStatus {
            server_id: server_id.clone(),
            startup_state: McpStartupState::Ready,
            auth_state: McpAuthState::Authenticated,
            tools: vec![
                McpToolDescriptor {
                    server_id: server_id.clone(),
                    name: "search:raw/name".to_string(),
                    description: "Search docs".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": { "query": { "type": "string" } }
                    }),
                },
                McpToolDescriptor {
                    server_id: McpServerId("Docs".to_string()),
                    name: "wrong-case".to_string(),
                    description: "Must be filtered".to_string(),
                    input_schema: json!({"type": "object"}),
                },
            ],
            resources: Vec::<McpResourceDescriptor>::new(),
            resource_templates: Vec::<McpResourceTemplateDescriptor>::new(),
            last_refreshed_at: None,
        })
    }

    async fn set_enabled(
        &self,
        server_id: &McpServerId,
        _enabled: bool,
    ) -> Result<McpServerStatus, McpError> {
        Err(McpError::McpServerUnavailable {
            server_id: server_id.clone(),
        })
    }

    async fn invoke_tool(
        &self,
        server_id: &McpServerId,
        tool_name: &str,
        _input: Value,
    ) -> Result<Value, McpError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        Err(McpError::McpToolInvocationFailed {
            server_id: server_id.clone(),
            tool_name: tool_name.to_string(),
            message: "unused fake method".to_string(),
        })
    }

    async fn read_resource(&self, server_id: &McpServerId, uri: &str) -> Result<Value, McpError> {
        Err(McpError::McpResourceReadFailed {
            server_id: server_id.clone(),
            uri: uri.to_string(),
            message: "unused fake method".to_string(),
        })
    }
}

fn test_bridge(manager: Arc<FakeMcpManager>) -> HostBridge {
    let mut bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
    bridge.mcp_manager = Some(manager);
    bridge
}

#[tokio::test]
async fn list_tools_refreshes_only_the_requested_server_and_returns_raw_schema() {
    let manager = Arc::new(FakeMcpManager::new());
    let bridge = test_bridge(manager.clone());

    let reply = handle_mcp_list_tools(&bridge, &json!({"server": "docs"})).await;

    assert_eq!(
        reply,
        json!({
            "status": "ok",
            "result": {
                "tools": [{
                    "server": "docs",
                    "name": "search:raw/name",
                    "description": "Search docs",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "query": { "type": "string" } }
                    }
                }]
            }
        })
    );
    assert_eq!(
        *manager.refreshed.lock().unwrap(),
        vec![McpServerId("docs".to_string())]
    );
    assert_eq!(manager.discoveries.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn mcp_call_denial_prevents_manager_invocation() {
    let manager = Arc::new(FakeMcpManager::new());
    let mut bridge = test_bridge(Arc::clone(&manager));
    bridge.permission = PermissionChecker::new(|_| {
        Box::pin(async { Err("permission denied by test".to_string()) })
    });

    let reply = handle_mcp_call(
        &bridge,
        &json!({ "server": "docs", "tool": "search", "arguments": {} }),
    )
    .await;

    assert_eq!(reply.get("status").and_then(Value::as_str), Some("error"));
    assert_eq!(manager.invocations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn list_tools_requires_server_before_refreshing() {
    let manager = Arc::new(FakeMcpManager::new());
    let bridge = test_bridge(manager.clone());

    let reply = handle_mcp_list_tools(&bridge, &json!({})).await;

    assert_eq!(reply.get("status").and_then(Value::as_str), Some("error"));
    assert!(manager.refreshed.lock().unwrap().is_empty());
    assert_eq!(manager.discoveries.load(Ordering::SeqCst), 0);
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn mediated_read_pins_the_file_during_approval_for_both_read_modes() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use tokio::sync::{Notify, oneshot};

    for once in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let requested = dir.path().join("approved.txt");
        let moved = dir.path().join("moved.txt");
        let secret = dir.path().join("private.txt");
        std::fs::write(&requested, "approved content").unwrap();
        std::fs::write(&secret, "SECRET CONTENT MUST NOT LEAK").unwrap();

        let (started_tx, started_rx) = oneshot::channel();
        let started = Mutex::new(Some(started_tx));
        let gate = Arc::new(Notify::new());
        let requested_for_check = requested.canonicalize().unwrap();
        let approval_gate = Arc::clone(&gate);
        let mut bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
        bridge.permission = PermissionChecker::new(move |req| {
            assert_eq!(req.path.as_deref(), Some(requested_for_check.as_path()));
            started.lock().unwrap().take().unwrap().send(()).unwrap();
            let gate = Arc::clone(&approval_gate);
            Box::pin(async move {
                gate.notified().await;
                Ok(PermissionGrant::default())
            })
        });
        let params = json!({"path": requested, "once": once});
        let read = tokio::spawn(async move { handle_fs_read(&bridge, &params).await });

        // The approval checker has observed the original path, but has not
        // released the read. Replace that path with a symlink to a secret.
        started_rx.await.unwrap();
        std::fs::rename(&requested, &moved).unwrap();
        #[cfg(unix)]
        symlink(&secret, &requested).unwrap();
        // Hard links require no elevated symlink privilege on Windows, while
        // still replacing the approved name with a different file identity.
        #[cfg(windows)]
        std::fs::hard_link(&secret, &requested).unwrap();
        gate.notify_one();
        let reply = read.await.unwrap();
        assert_eq!(reply["status"], "ok", "once={once}: {reply}");
        assert_eq!(
            reply["result"]["content"], "approved content",
            "once={once}"
        );
        assert_eq!(
            reply["result"]["once"],
            if once { json!(true) } else { Value::Null }
        );
    }
}

#[tokio::test]
async fn both_mediated_read_modes_reject_oversized_files() {
    let dir = tempfile::tempdir().unwrap();
    let large = dir.path().join("large.txt");
    std::fs::write(&large, vec![b'a'; 1024 * 1024 + 1]).unwrap();
    let bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
    for once in [false, true] {
        let reply = handle_fs_read(&bridge, &json!({"path": large, "once": once})).await;
        assert_eq!(reply["status"], "error", "once={once}: {reply}");
        assert!(
            reply["error"]
                .as_str()
                .unwrap()
                .contains("mediated read limit")
        );
    }
}
