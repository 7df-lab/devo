#[path = "support/acp_session_setup.rs"]
mod acp_session_setup;

use anyhow::Result;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use acp_session_setup::AcpStdioClientInfo;
use acp_session_setup::assert_stdio_auth_required;
use acp_session_setup::shutdown_stdio_acp_process;
use acp_session_setup::spawn_stdio_acp_process;
use acp_session_setup::stdio_initialize;
use acp_session_setup::stdio_request_until;
use acp_session_setup::write_stdio_test_config;

#[tokio::test]
async fn stdio_acp_initialize_negotiates_capabilities_and_allows_session_setup() -> Result<()> {
    let home_dir = TempDir::new()?;
    write_stdio_test_config(&home_dir, &["stdio://"], "")?;
    let test_cwd = home_dir.path().to_string_lossy().into_owned();
    let mut process = spawn_stdio_acp_process(home_dir.path()).await?;

    let initialize_response = stdio_initialize(
        &mut process,
        0,
        AcpStdioClientInfo {
            name: "acp-initialization-e2e",
            title: "ACP Initialization E2E",
        },
        json!({
            "fs": { "readTextFile": true, "writeTextFile": true },
            "terminal": true
        }),
    )
    .await?;
    assert_eq!(initialize_response["jsonrpc"], json!("2.0"));
    assert_eq!(initialize_response["id"], json!(0));
    assert_eq!(initialize_response["result"]["protocolVersion"], json!(1));
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["loadSession"],
        json!(true)
    );
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["promptCapabilities"],
        json!({ "image": false, "audio": false, "embeddedContext": true })
    );
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["mcpCapabilities"],
        json!({ "http": true, "sse": true })
    );
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["sessionCapabilities"],
        json!({
            "list": {},
            "delete": {},
            "additionalDirectories": {},
            "resume": {},
            "close": {}
        })
    );
    let auth_methods = &initialize_response["result"]["authMethods"];
    assert!(auth_methods.is_null() || auth_methods == &json!([]));
    assert_eq!(
        initialize_response["result"]["agentInfo"]["name"],
        json!("devo-server")
    );
    assert_eq!(
        initialize_response["result"]["agentInfo"]["title"],
        json!("Devo")
    );
    assert!(
        initialize_response["result"]["agentInfo"]["version"]
            .as_str()
            .is_some_and(|version| !version.is_empty())
    );

    let session_new_response = stdio_request_until(
        &mut process,
        1,
        "session/new",
        json!({ "cwd": test_cwd, "mcpServers": [] }),
        "ACP session/new response",
    )
    .await?;
    assert_eq!(session_new_response["jsonrpc"], json!("2.0"));
    assert_eq!(session_new_response["id"], json!(1));
    assert!(
        session_new_response["result"]["sessionId"]
            .as_str()
            .is_some_and(|session_id| !session_id.is_empty())
    );

    shutdown_stdio_acp_process(process).await;
    Ok(())
}

#[tokio::test]
async fn stdio_acp_auth_gates_acp_methods() -> Result<()> {
    let home_dir = TempDir::new()?;
    write_stdio_test_config(
        &home_dir,
        &["stdio://"],
        r#"
[server.auth]
enabled = true
method_id = "agent-login"
name = "Agent login"
description = "Use the test login flow"
logout = true
"#,
    )?;
    let test_cwd = home_dir.path().to_string_lossy().into_owned();
    let mut process = spawn_stdio_acp_process(home_dir.path()).await?;

    let initialize_response = stdio_initialize(
        &mut process,
        0,
        AcpStdioClientInfo {
            name: "acp-auth-e2e",
            title: "ACP Auth E2E",
        },
        json!({}),
    )
    .await?;
    assert_eq!(
        initialize_response["result"]["authMethods"],
        json!([{
            "id": "agent-login",
            "name": "Agent login",
            "description": "Use the test login flow"
        }])
    );
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["auth"],
        json!({ "logout": {} })
    );

    assert_stdio_auth_required(
        &stdio_request_until(
            &mut process,
            1,
            "session/new",
            json!({ "cwd": test_cwd, "mcpServers": [] }),
            "unauthenticated ACP session/new response",
        )
        .await?,
    );

    assert_eq!(
        stdio_request_until(
            &mut process,
            3,
            "authenticate",
            json!({ "methodId": "agent-login" }),
            "authenticate response",
        )
        .await?,
        json!({ "jsonrpc": "2.0", "id": 3, "result": {} })
    );

    let session_new_response = stdio_request_until(
        &mut process,
        4,
        "session/new",
        json!({ "cwd": test_cwd, "mcpServers": [] }),
        "authenticated ACP session/new response",
    )
    .await?;
    assert!(
        session_new_response["result"]["sessionId"]
            .as_str()
            .is_some_and(|session_id| !session_id.is_empty())
    );

    let session_list_response = stdio_request_until(
        &mut process,
        5,
        "session/list",
        json!({}),
        "authenticated ACP session/list response",
    )
    .await?;
    assert!(
        session_list_response["result"]["sessions"]
            .as_array()
            .is_some_and(|sessions| !sessions.is_empty())
    );

    assert_eq!(
        stdio_request_until(&mut process, 6, "logout", json!({}), "logout response").await?,
        json!({ "jsonrpc": "2.0", "id": 6, "result": {} })
    );
    assert_stdio_auth_required(
        &stdio_request_until(
            &mut process,
            7,
            "session/list",
            json!({}),
            "relocked ACP session/list response",
        )
        .await?,
    );

    shutdown_stdio_acp_process(process).await;
    Ok(())
}
