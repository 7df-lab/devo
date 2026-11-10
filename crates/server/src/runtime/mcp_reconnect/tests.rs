use super::*;
use async_trait::async_trait;
use devo_core::tools::{ToolPlanConfig, ToolRegistry, handlers};
use devo_core::{
    AppConfigStore, McpAuthState, McpError, McpServerRecord, McpServerStatus, McpToolDescriptor,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{Mutex, Notify};

struct TestManager {
    states: Mutex<Vec<McpServerStatus>>,
    reconnects: AtomicUsize,
    block: AtomicBool,
    fail: AtomicBool,
    block_refresh: AtomicBool,
    refresh_entered: Notify,
    entered: Notify,
    release: Notify,
}

fn status(server: &str, state: McpStartupState, tool: &str) -> McpServerStatus {
    McpServerStatus {
        server_id: McpServerId(server.into()),
        startup_state: state,
        auth_state: McpAuthState::NotRequired,
        tools: if tool.is_empty() {
            Vec::new()
        } else {
            vec![McpToolDescriptor {
                server_id: McpServerId(server.into()),
                name: tool.into(),
                description: "test tool".into(),
                input_schema: json!({"type":"object"}),
            }]
        },
        resources: Vec::new(),
        resource_templates: Vec::new(),
        last_refreshed_at: None,
    }
}

impl TestManager {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            states: Mutex::new(vec![
                status("docs", McpStartupState::Ready, "old"),
                status("sibling", McpStartupState::Ready, "keep"),
            ]),
            reconnects: AtomicUsize::new(0),
            block: AtomicBool::new(false),
            fail: AtomicBool::new(false),
            block_refresh: AtomicBool::new(false),
            refresh_entered: Notify::new(),
            entered: Notify::new(),
            release: Notify::new(),
        })
    }
}

#[async_trait]
impl McpManager for TestManager {
    async fn statuses(&self) -> Result<Vec<McpServerStatus>, McpError> {
        Ok(self.states.lock().await.clone())
    }
    async fn cached_tools(&self, id: &McpServerId) -> Result<Vec<McpToolInfo>, McpError> {
        Ok(self
            .states
            .lock()
            .await
            .iter()
            .filter(|s| &s.server_id == id && s.startup_state == McpStartupState::Ready)
            .flat_map(|s| &s.tools)
            .map(|t| {
                McpToolInfo::new(
                    id.clone(),
                    id.0.clone(),
                    t.name.clone(),
                    Some(t.description.clone()),
                    t.input_schema.clone(),
                    /*read_only_hint*/ true,
                    /*supports_parallel_tool_calls*/ false,
                )
            })
            .collect())
    }
    async fn discover_tools(&self) -> Result<Vec<McpToolInfo>, McpError> {
        let mut tools = self.cached_tools(&McpServerId("docs".into())).await?;
        tools.extend(self.cached_tools(&McpServerId("sibling".into())).await?);
        Ok(tools)
    }
    async fn refresh(&self, id: &McpServerId) -> Result<McpServerStatus, McpError> {
        if self.block_refresh.load(Ordering::SeqCst) {
            self.refresh_entered.notify_one();
            self.release.notified().await;
        }
        self.states
            .lock()
            .await
            .iter()
            .find(|s| &s.server_id == id)
            .cloned()
            .ok_or(McpError::McpServerUnavailable {
                server_id: id.clone(),
            })
    }
    async fn reconnect(&self, id: &McpServerId) -> Result<McpServerStatus, McpError> {
        self.reconnects.fetch_add(1, Ordering::SeqCst);
        {
            let mut states = self.states.lock().await;
            *states.iter_mut().find(|s| &s.server_id == id).unwrap() =
                status(&id.0, McpStartupState::Starting, "");
        }
        self.entered.notify_one();
        if self.block.load(Ordering::SeqCst) {
            self.release.notified().await;
        }
        let mut states = self.states.lock().await;
        let current = states.iter_mut().find(|s| &s.server_id == id).unwrap();
        if current.startup_state == McpStartupState::Disabled {
            return Err(McpError::McpServerUnavailable {
                server_id: id.clone(),
            });
        }
        if self.fail.load(Ordering::SeqCst) {
            *current = status(&id.0, McpStartupState::Failed, "");
            return Err(McpError::McpProtocolError {
                server_id: id.clone(),
                message: "synthetic-secret-provider-body".into(),
            });
        }
        *current = status(&id.0, McpStartupState::Ready, "new");
        Ok(current.clone())
    }
    async fn configure_enabled(
        &self,
        id: &McpServerId,
        enabled: bool,
    ) -> Result<McpServerStatus, McpError> {
        let mut states = self.states.lock().await;
        let current = states.iter_mut().find(|s| &s.server_id == id).unwrap();
        *current = status(
            &id.0,
            if enabled {
                McpStartupState::Ready
            } else {
                McpStartupState::Disabled
            },
            "",
        );
        Ok(current.clone())
    }
    async fn set_enabled(
        &self,
        id: &McpServerId,
        enabled: bool,
    ) -> Result<McpServerStatus, McpError> {
        self.configure_enabled(id, enabled).await
    }
    async fn invoke_tool(&self, _: &McpServerId, _: &str, _: Value) -> Result<Value, McpError> {
        panic!("Reconnect must never invoke a tool")
    }
    async fn read_resource(&self, _: &McpServerId, _: &str) -> Result<Value, McpError> {
        panic!("Reconnect must never read a resource")
    }
}

fn record() -> McpServerRecord {
    McpServerRecord {
        id: McpServerId("docs".into()),
        display_name: "Docs".into(),
        transport: devo_core::McpTransportConfig::Stdio {
            command: vec!["__unused__".into()],
            cwd: None,
            env: Default::default(),
            env_vars: Vec::new(),
        },
        startup_policy: devo_core::McpStartupPolicy::Manual,
        enabled: true,
        trust_policy: Default::default(),
        allowed_capabilities: Vec::new(),
        roots_policy: Default::default(),
        output_limits: Default::default(),
        auth_ref: None,
    }
}

async fn setup() -> (tempfile::TempDir, Arc<ServerRuntime>, Arc<TestManager>) {
    let temp = tempfile::tempdir().unwrap();
    let mut config = AppConfigStore::load(temp.path().to_path_buf(), None).unwrap();
    config.upsert_mcp_server(record()).unwrap();
    let manager = TestManager::new();
    let registry =
        handlers::build_registry_from_plan_with_mcp(&ToolPlanConfig::default(), manager.clone())
            .await;
    let runtime = crate::TestRuntime::noop()
        .with_test_model()
        .mcp(manager.clone())
        .registry(Arc::new(registry))
        .config_store(Arc::new(std::sync::Mutex::new(config)))
        .runtime(temp.path());
    (temp, runtime, manager)
}

async fn reconnect(runtime: &ServerRuntime, params: Value) -> Value {
    runtime.handle_mcp_reconnect(json!(1), params).await
}
fn names(registry: &ToolRegistry) -> Vec<String> {
    let mut names = registry
        .tool_definitions()
        .into_iter()
        .map(|t| t.name)
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[tokio::test]
async fn native_reconnect_rejects_credentials_unknown_disabled_and_starting() {
    let (_temp, runtime, manager) = setup().await;
    for params in [
        json!({"name":"docs","access":"synthetic-secret"}),
        json!({"name":" "}),
        json!({"name":"unknown"}),
    ] {
        let response = reconnect(&runtime, params).await;
        assert!(response.get("error").is_some());
        assert!(!response.to_string().contains("synthetic-secret"));
    }
    for state in [McpStartupState::Disabled, McpStartupState::Starting] {
        manager.states.lock().await[0] = status("docs", state, "");
        assert!(
            reconnect(&runtime, json!({"name":"docs"}))
                .await
                .get("error")
                .is_some()
        );
    }
    assert_eq!(manager.reconnects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_reconnect_replaces_only_selected_tools_without_calls() {
    let (_temp, runtime, manager) = setup().await;
    let mut expected = names(&runtime.deps.process_context.tool_registry());
    expected.retain(|name| name != "mcp__docs__old");
    expected.push("mcp__docs__new".into());
    expected.sort();
    assert!(
        reconnect(&runtime, json!({"name":"docs"}))
            .await
            .get("error")
            .is_none()
    );
    assert_eq!(
        names(&runtime.deps.process_context.tool_registry()),
        expected
    );
    assert_eq!(manager.reconnects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn native_reconnect_failure_purges_old_tools_and_redacts_provider_body() {
    let (_temp, runtime, manager) = setup().await;
    manager.fail.store(true, Ordering::SeqCst);
    let response = reconnect(&runtime, json!({"name":"docs"})).await;
    assert!(response.get("error").is_some());
    assert!(
        !response
            .to_string()
            .contains("synthetic-secret-provider-body")
    );
    let registry = runtime.deps.process_context.tool_registry();
    assert!(registry.spec("mcp__docs__old").is_none());
    assert!(registry.spec("mcp__sibling__keep").is_some());
    manager.fail.store(false, Ordering::SeqCst);
    assert!(
        reconnect(&runtime, json!({"name":"docs"}))
            .await
            .get("error")
            .is_none()
    );
}

#[tokio::test]
async fn native_reconnect_busy_keeps_status_responsive_and_disable_wins() {
    let (_temp, runtime, manager) = setup().await;
    manager.block.store(true, Ordering::SeqCst);
    let spawned = Arc::clone(&runtime);
    let task = tokio::spawn(async move { reconnect(&spawned, json!({"name":"docs"})).await });
    manager.entered.notified().await;
    assert!(
        runtime
            .deps
            .process_context
            .tool_registry()
            .spec("mcp__docs__old")
            .is_none()
    );
    assert!(
        reconnect(&runtime, json!({"name":"docs"}))
            .await
            .get("error")
            .is_some()
    );
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        runtime.handle_mcp_list(json!(2), json!({})),
    )
    .await
    .unwrap();
    assert_eq!(
        response["result"]["servers"][0],
        json!({"name":"docs","status":"starting","toolCount":0})
    );
    let disabled = runtime
        .handle_mcp_set_enabled(json!(3), json!({"name":"docs","enabled":false}))
        .await;
    assert!(disabled.get("error").is_none(), "{disabled}");
    manager.release.notify_one();
    assert!(task.await.unwrap().get("error").is_some());
    assert_eq!(
        manager.states.lock().await[0],
        status("docs", McpStartupState::Disabled, "")
    );
    assert!(
        runtime
            .deps
            .process_context
            .tool_registry()
            .spec("mcp__docs__new")
            .is_none()
    );
}

#[tokio::test]
async fn native_reconnect_preserves_shared_manager_workspace_tool_plan() {
    let (temp, runtime, _manager) = setup().await;
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(workspace.join(".devo")).unwrap();
    std::fs::write(
        workspace.join(".devo/config.toml"),
        "[tools]\nexecution_surface = \"discrete\"\n",
    )
    .unwrap();
    let context = runtime
        .deps
        .context_for_workspace(&workspace)
        .await
        .unwrap();
    assert!(Arc::ptr_eq(
        &context.mcp_manager,
        &runtime.deps.process_context.mcp_manager
    ));
    // An independently restricted workspace registry must not become the global plan.
    let registry = context.tool_registry().restricted_to_specs(&[
        "shell_command",
        "mcp__docs__old",
        "mcp__sibling__keep",
    ]);
    context.replace_tool_registry(Arc::new(registry));
    assert!(
        reconnect(&runtime, json!({"name":"docs"}))
            .await
            .get("error")
            .is_none()
    );
    assert_eq!(
        names(&context.tool_registry()),
        vec!["mcp__docs__new", "mcp__sibling__keep", "shell_command"]
    );
}

#[tokio::test]
async fn native_reconnect_does_not_touch_same_named_workspace_override() {
    let (temp, runtime, _manager) = setup().await;
    let workspace = temp.path().join("override");
    std::fs::create_dir_all(workspace.join(".devo")).unwrap();
    std::fs::write(
        workspace.join(".devo/config.toml"),
        "[mcp_servers.docs]\ncommand = \"__workspace_override__\"\nenabled = false\n",
    )
    .unwrap();
    let context = runtime
        .deps
        .context_for_workspace(&workspace)
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(
        &context.mcp_manager,
        &runtime.deps.process_context.mcp_manager
    ));
    let before = context.mcp_manager.statuses().await.unwrap();
    let registry = context.tool_registry();
    assert!(
        reconnect(&runtime, json!({"name":"docs"}))
            .await
            .get("error")
            .is_none()
    );
    assert_eq!(context.mcp_manager.statuses().await.unwrap(), before);
    assert!(Arc::ptr_eq(&registry, &context.tool_registry()));
}

async fn session(
    runtime: &Arc<ServerRuntime>,
    cwd: &std::path::Path,
) -> super::super::session_actor::SessionHandle {
    let (outbound, _notifications) = crate::test_outbound_channel(128);
    let connection = runtime
        .register_connection(crate::ClientTransportKind::Stdio, outbound)
        .await;
    let response = runtime.handle_incoming(connection, json!({"id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}}}})).await.unwrap();
    assert!(response.get("error").is_none(), "{response}");
    let response = runtime.handle_incoming(connection, json!({"id":2,"method":"session/new","params":{"cwd":cwd,"idempotencyKey":uuid::Uuid::new_v4().to_string()}})).await.unwrap();
    assert!(response.get("error").is_none(), "{response}");
    let id = devo_protocol::native::ids::SessionId::from(
        response["result"]["session"]["id"].as_str().unwrap(),
    );
    runtime.session(id).await.unwrap()
}

#[tokio::test]
async fn native_reconnect_preserves_explicit_actor_registry() {
    let (temp, runtime, _manager) = setup().await;
    let handle = session(&runtime, temp.path()).await;
    let actor_registry = runtime
        .deps
        .process_context
        .tool_registry()
        .restricted_to_specs(&["shell_command", "mcp__docs__old"]);
    assert!(
        handle
            .set_tool_registry(Some(Arc::new(actor_registry)))
            .await
    );
    assert!(
        reconnect(&runtime, json!({"name":"docs"}))
            .await
            .get("error")
            .is_none()
    );
    let snapshot = handle.spawn_snapshot().await.unwrap();
    assert_eq!(
        names(snapshot.parent_tool_registry.as_ref().unwrap()),
        vec!["mcp__docs__new", "shell_command"]
    );
    assert!(
        runtime
            .deps
            .process_context
            .tool_registry()
            .spec("mcp__sibling__keep")
            .is_some()
    );
}

#[tokio::test]
async fn native_publication_preserves_independently_owned_explicit_session_registry() {
    // ACP installs its manager in the explicit registry while the runtime context
    // retains the process manager. Cover both populated and empty ACP discovery.
    for tool in ["session_only", ""] {
        let (temp, runtime, manager) = setup().await;
        let handle = session(&runtime, temp.path()).await;
        assert!(Arc::ptr_eq(
            &handle.runtime_context().await.unwrap().mcp_manager,
            &runtime.deps.process_context.mcp_manager
        ));
        let session_manager = TestManager::new();
        *session_manager.states.lock().await = vec![status("docs", McpStartupState::Ready, tool)];
        let registry = Arc::new(
            handlers::build_registry_from_plan_with_mcp(
                &ToolPlanConfig::default(),
                session_manager.clone(),
            )
            .await,
        );
        assert!(handle.set_tool_registry(Some(Arc::clone(&registry))).await);
        for fail in [false, true] {
            manager.fail.store(fail, Ordering::SeqCst);
            let response = reconnect(&runtime, json!({"name":"docs"})).await;
            assert_eq!(response.get("error").is_some(), fail);
            let after = handle
                .spawn_snapshot()
                .await
                .unwrap()
                .parent_tool_registry
                .unwrap();
            assert_eq!(
                serde_json::to_value(after.tool_definitions()).unwrap(),
                serde_json::to_value(registry.tool_definitions()).unwrap()
            );
            for (name, handler) in registry.all_handlers() {
                assert!(Arc::ptr_eq(after.get(name).unwrap(), handler));
            }
        }
        assert!(
            runtime
                .handle_mcp_set_enabled(json!(2), json!({"name":"docs","enabled":false}))
                .await
                .get("error")
                .is_none()
        );
        let after = handle
            .spawn_snapshot()
            .await
            .unwrap()
            .parent_tool_registry
            .unwrap();
        assert_eq!(
            serde_json::to_value(after.tool_definitions()).unwrap(),
            serde_json::to_value(registry.tool_definitions()).unwrap()
        );
        assert_eq!(session_manager.reconnects.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn native_reconnect_late_publication_reads_disabled_state() {
    let (_temp, runtime, manager) = setup().await;
    manager.block.store(true, Ordering::SeqCst);
    let spawned = Arc::clone(&runtime);
    let task = tokio::spawn(async move { reconnect(&spawned, json!({"name":"docs"})).await });
    manager.entered.notified().await;
    let gate = runtime.mcp_publication_gate.lock().await;
    manager
        .configure_enabled(&McpServerId("docs".into()), /*enabled*/ false)
        .await
        .unwrap();
    manager.release.notify_one();
    drop(gate);
    assert!(task.await.unwrap().get("error").is_some());
    assert!(
        runtime
            .deps
            .process_context
            .tool_registry()
            .spec("mcp__docs__new")
            .is_none()
    );
    assert!(
        runtime
            .deps
            .process_context
            .tool_registry()
            .spec("mcp__sibling__keep")
            .is_some()
    );
}

#[tokio::test]
async fn native_enablement_repeated_toggles_reach_inherited_workspace_manager() {
    let (temp, runtime, _manager) = setup().await;
    let workspace = temp.path().join("inherited");
    std::fs::create_dir_all(workspace.join(".devo")).unwrap();
    std::fs::write(
        workspace.join(".devo/config.toml"),
        "[mcp_servers.extra]\ncommand = \"__unused__\"\nenabled = false\n",
    )
    .unwrap();
    let handle = session(&runtime, &workspace).await;
    let context = handle.runtime_context().await.unwrap();
    assert!(!Arc::ptr_eq(
        &context.mcp_manager,
        &runtime.deps.process_context.mcp_manager
    ));
    for enabled in [false, true, false, true] {
        let response = runtime
            .handle_mcp_set_enabled(json!(1), json!({"name":"docs","enabled":enabled}))
            .await;
        assert!(response.get("error").is_none(), "{response}");
        let states = context.mcp_manager.statuses().await.unwrap();
        let state = states.iter().find(|s| s.server_id.0 == "docs").unwrap();
        assert_eq!(
            state.startup_state.clone(),
            if enabled {
                McpStartupState::Failed
            } else {
                McpStartupState::Disabled
            }
        );
        assert_eq!(
            states
                .iter()
                .find(|s| s.server_id.0 == "extra")
                .unwrap()
                .startup_state,
            McpStartupState::Disabled
        );
    }
}

#[tokio::test]
async fn native_enablement_later_disable_wins_before_second_manager_discovery() {
    let (temp, runtime, manager) = setup().await;
    let workspace = temp.path().join("inherited");
    std::fs::create_dir_all(workspace.join(".devo")).unwrap();
    std::fs::write(
        workspace.join(".devo/config.toml"),
        "[mcp_servers.extra]\ncommand = \"__unused__\"\nenabled = false\n",
    )
    .unwrap();
    let handle = session(&runtime, &workspace).await;
    let context = handle.runtime_context().await.unwrap();
    manager.block_refresh.store(true, Ordering::SeqCst);
    let spawned = Arc::clone(&runtime);
    let enable = tokio::spawn(async move {
        spawned
            .handle_mcp_set_enabled(json!(1), json!({"name":"docs","enabled":true}))
            .await
    });
    manager.refresh_entered.notified().await;
    let disable = runtime
        .handle_mcp_set_enabled(json!(2), json!({"name":"docs","enabled":false}))
        .await;
    assert!(disable.get("error").is_none());
    manager.release.notify_one();
    assert!(enable.await.unwrap().get("error").is_none());
    assert_eq!(
        context
            .mcp_manager
            .statuses()
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.server_id.0 == "docs")
            .unwrap()
            .startup_state,
        McpStartupState::Disabled
    );
}

#[tokio::test]
async fn native_equal_workspace_override_has_independent_connection_ownership() {
    let (temp, runtime, _manager) = setup().await;
    let workspace = temp.path().join("equal-override");
    std::fs::create_dir_all(workspace.join(".devo")).unwrap();
    // Even an identical explicit value belongs to this workspace.
    std::fs::write(
        workspace.join(".devo/config.toml"),
        "[mcp_servers.docs]\nenabled = true\n",
    )
    .unwrap();
    let handle = session(&runtime, &workspace).await;
    let context = handle.runtime_context().await.unwrap();
    assert!(!Arc::ptr_eq(
        &context.mcp_manager,
        &runtime.deps.process_context.mcp_manager
    ));
    let before = context.mcp_manager.statuses().await.unwrap();
    assert!(
        reconnect(&runtime, json!({"name":"docs"}))
            .await
            .get("error")
            .is_none()
    );
    let disable = runtime
        .handle_mcp_set_enabled(json!(1), json!({"name":"docs","enabled":false}))
        .await;
    assert!(disable.get("error").is_none());
    assert_eq!(context.mcp_manager.statuses().await.unwrap(), before);
}
