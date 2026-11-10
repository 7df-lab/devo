//! Native wire admission and failure behavior, using a synthetic missing server.
use std::sync::Arc;

use devo_core::{
    AppConfigStore, McpServerId, McpServerRecord, McpStartupPolicy, McpTransportConfig,
};
use devo_server::{ClientTransportKind, TestRuntime};
use pretty_assertions::assert_eq;
use serde_json::json;

#[tokio::test]
async fn native_reconnect_wire_contract_is_strict_and_failure_remains_readable() {
    let home = tempfile::tempdir().unwrap();
    let mut store = AppConfigStore::load(home.path().to_path_buf(), None).unwrap();
    store
        .upsert_mcp_server(McpServerRecord {
            id: McpServerId("missing".into()),
            display_name: "Missing synthetic server".into(),
            transport: McpTransportConfig::Stdio {
                command: vec!["__devo_nonexistent_mcp_test_binary__".into()],
                cwd: None,
                env: Default::default(),
                env_vars: Vec::new(),
            },
            startup_policy: McpStartupPolicy::Manual,
            enabled: true,
            trust_policy: Default::default(),
            allowed_capabilities: Vec::new(),
            roots_policy: Default::default(),
            output_limits: Default::default(),
            auth_ref: None,
        })
        .unwrap();
    let manager = Arc::new(devo_mcp::manager::RmcpMcpManager::new(
        store.effective_config().mcp_runtime.clone(),
        Default::default(),
    ));
    let runtime = TestRuntime::noop()
        .mcp(manager)
        .config_store(Arc::new(std::sync::Mutex::new(store)))
        .runtime(home.path());
    let (outbound, _notifications) = devo_server::test_outbound_channel(128);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    let initialized = runtime.handle_incoming(connection, json!({"id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}}}})).await.unwrap();
    assert!(initialized.get("error").is_none());
    let rejected = runtime.handle_incoming(connection,json!({"id":2,"method":"mcp/reconnect","params":{"name":"missing","access":"synthetic-secret"}})).await.unwrap();
    assert!(rejected.get("error").is_some());
    assert!(!rejected.to_string().contains("synthetic-secret"));
    for id in [3, 4] {
        let result = runtime
            .handle_incoming(
                connection,
                json!({"id":id,"method":"mcp/reconnect","params":{"name":"missing"}}),
            )
            .await
            .unwrap();
        assert!(result.get("error").is_some());
        assert!(
            !result
                .to_string()
                .contains("__devo_nonexistent_mcp_test_binary__")
        );
    }
    let listed = runtime
        .handle_incoming(connection, json!({"id":5,"method":"mcp/list","params":{}}))
        .await
        .unwrap();
    assert_eq!(
        listed["result"],
        json!({"servers":[{"name":"missing","status":"failed","toolCount":0}]})
    );
}
