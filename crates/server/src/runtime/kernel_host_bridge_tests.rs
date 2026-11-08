use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use pretty_assertions::assert_eq;
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
    observed_statuses: Vec<McpServerStatus>,
    refresh_state: McpStartupState,
}

impl FakeMcpManager {
    fn new() -> Self {
        Self {
            refreshed: Mutex::new(Vec::new()),
            discoveries: AtomicUsize::new(0),
            invocations: AtomicUsize::new(0),
            observed_statuses: Vec::new(),
            refresh_state: McpStartupState::Ready,
        }
    }
}

#[async_trait]
impl McpManager for FakeMcpManager {
    async fn statuses(&self) -> Result<Vec<McpServerStatus>, McpError> {
        Ok(self.observed_statuses.clone())
    }

    async fn discover_tools(&self) -> Result<Vec<McpToolInfo>, McpError> {
        self.discoveries.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    }

    async fn refresh(&self, server_id: &McpServerId) -> Result<McpServerStatus, McpError> {
        self.refreshed.lock().unwrap().push(server_id.clone());
        Ok(McpServerStatus {
            server_id: server_id.clone(),
            startup_state: self.refresh_state.clone(),
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

#[tokio::test]
async fn mcp_status_dispatch_is_read_only_and_does_not_expose_config_or_schemas() {
    let mut manager = FakeMcpManager::new();
    for (server, state) in [
        ("z-disabled", McpStartupState::Disabled),
        ("a-ready", McpStartupState::Ready),
        ("b-auth", McpStartupState::AuthRequired),
    ] {
        manager.observed_statuses.push(McpServerStatus {
            server_id: McpServerId(server.to_string()),
            startup_state: state,
            auth_state: McpAuthState::NotRequired,
            tools: Vec::new(),
            resources: Vec::new(),
            resource_templates: Vec::new(),
            last_refreshed_at: None,
        });
    }
    let manager = Arc::new(manager);
    let bridge = test_bridge(Arc::clone(&manager));
    let reply = crate::runtime::kernel_host::dispatch_host_request_with_bridge(
        &bridge,
        json!({ "type": "mcp.list_servers" }),
    )
    .await;
    assert_eq!(
        reply,
        json!({ "status": "ok", "result": { "servers": [
            { "server": "a-ready", "status": "ready", "authStatus": "not_required", "toolCount": 0 },
            { "server": "b-auth", "status": "auth_required", "authStatus": "not_required", "toolCount": 0 },
            { "server": "z-disabled", "status": "disabled", "authStatus": "not_required", "toolCount": 0 }
        ] } })
    );
    assert!(manager.refreshed.lock().unwrap().is_empty());
    assert_eq!(manager.discoveries.load(Ordering::SeqCst), 0);
    assert_eq!(manager.invocations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn mcp_discovery_does_not_report_non_ready_servers_as_connected() {
    for state in [
        McpStartupState::Disabled,
        McpStartupState::NotStarted,
        McpStartupState::Starting,
        McpStartupState::Failed,
        McpStartupState::AuthRequired,
        McpStartupState::Degraded,
        McpStartupState::Stopped,
    ] {
        let mut manager = FakeMcpManager::new();
        manager.refresh_state = state;
        let bridge = test_bridge(Arc::new(manager));
        let reply = handle_mcp_list_tools(&bridge, &json!({ "server": "docs" })).await;
        assert_eq!(
            reply,
            json!({
                "status": "error",
                "error": "MCP server 'docs' is not ready; inspect mcp.list_servers before retrying"
            })
        );
    }
}

#[tokio::test]
async fn legacy_auth_actions_fail_explicitly_instead_of_acknowledging_noops() {
    let manager = Arc::new(FakeMcpManager::new());
    let bridge = test_bridge(Arc::clone(&manager));
    for action in ["mcp.refresh", "mcp.config"] {
        let reply = crate::runtime::kernel_host::dispatch_host_request_with_bridge(
            &bridge,
            json!({ "type": action, "server": "linear" }),
        )
        .await;
        assert_eq!(reply["status"], "error");
        assert!(reply.get("result").is_none());
        assert!(reply["error"].as_str().unwrap().contains("Rust host"));
    }
    assert!(manager.refreshed.lock().unwrap().is_empty());
    assert_eq!(manager.invocations.load(Ordering::SeqCst), 0);
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

/// Exercises the real Python REPL transport, Rust dispatcher, and manager/Ask
/// boundary. The MCP manager and expired grant are synthetic; no live OAuth.
#[tokio::test]
#[ignore = "requires Python 3.11+; run explicitly for kernel/host integration changes"]
async fn mcp_real_kernel_uses_native_dispatch_and_reports_legacy_refresh_failure() {
    use devo_kernel::{KernelSession, KernelSessionConfig};

    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("auth.json"),
        r#"{"mcp:linear":{"type":"oauth","endpoint":"https://mcp.linear.app/mcp","access":"synthetic-expired","expires":0}}"#,
    )
    .unwrap();
    let crates_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let mut config = KernelSessionConfig::default();
    config.cwd = home.path().to_path_buf();
    config.python_path_entries = vec![
        crates_dir.join("kernel/rlm-runtime/src"),
        crates_dir.join("skills/assets/bundled/linear/src"),
    ];
    config.extra_env.push((
        "DEVO_CODING_AGENT_DIR".into(),
        home.path().to_string_lossy().into_owned(),
    ));
    let kernel = KernelSession::spawn(config).await.unwrap();
    let manager = Arc::new(FakeMcpManager::new());
    let mut bridge = test_bridge(Arc::clone(&manager));
    bridge.permission =
        PermissionChecker::new(|_| Box::pin(async { Err("denied by synthetic host".to_string()) }));
    kernel
        .set_host_handler(Some(super::host_handler_for_bridge(Arc::new(bridge))))
        .await;
    let output = kernel
        .execute(
            r#"
from rlm import mcp
import linear
assert await mcp.list_servers() == []
assert await mcp.list_tools("docs") == [{
    "name": "search:raw/name", "description": "Search docs",
    "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}},
}]
try:
    await mcp.call_tool("docs", "create", {"name": "demo"})
except PermissionError:
    pass
else:
    raise AssertionError("host denial was ignored")
try:
    await linear.linear._resolve_token()
except RuntimeError as error:
    assert "Rust host" in str(error)
    from rlm.mcp_base import NotEnabled
    assert not isinstance(error, NotEnabled)
else:
    raise AssertionError("native host falsely refreshed legacy credentials")
print("MCP_NATIVE_WIRE_OK")
"#,
        )
        .await
        .unwrap();
    assert_eq!(output.status, "ok", "{output:?}");
    assert_eq!(output.stdout, "MCP_NATIVE_WIRE_OK\n");
    assert_eq!(manager.invocations.load(Ordering::SeqCst), 0);
    assert_eq!(
        *manager.refreshed.lock().unwrap(),
        vec![McpServerId("docs".to_string())]
    );
}
