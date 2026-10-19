#[path = "support/acp_runtime_harness.rs"]
mod acp_runtime_harness;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use devo_protocol::AcpAuthMethod;
use devo_protocol::AcpLoadSessionResult;
use devo_protocol::AcpLogoutCapabilities;
use devo_protocol::AcpPromptResult;
use devo_protocol::AcpResumeSessionResult;
use devo_protocol::AcpSessionUpdate;
use devo_protocol::AcpStopReason;
use devo_protocol::SessionId;
use devo_protocol::native::session::Session;
use devo_server::AcpErrorResponse;
use devo_server::AcpInitializeResult;
use devo_server::AcpSuccessResponse;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;

use acp_runtime_harness::AcpTestClient;
use acp_runtime_harness::SingleReplyProvider;
use acp_runtime_harness::acp_request;
use acp_runtime_harness::assert_acp_error_message;
use acp_runtime_harness::assert_acp_slash_command_advertisement;
use acp_runtime_harness::assert_auth_required;
use acp_runtime_harness::assert_no_replayed_history;
use acp_runtime_harness::assert_removed_session_method;
use acp_runtime_harness::build_acp_test_runtime;
use acp_runtime_harness::connect_acp;
use acp_runtime_harness::create_acp_session_id;
use acp_runtime_harness::decode_native_session_meta;
use acp_runtime_harness::handle_acp;
use acp_runtime_harness::list_acp_sessions;
use acp_runtime_harness::path_value;
use acp_runtime_harness::send_session_prompt;
use acp_runtime_harness::session_load;
use acp_runtime_harness::session_resume;
use acp_runtime_harness::stdio_mcp_server_value;
use acp_runtime_harness::wait_for_agent_text_update;
use acp_runtime_harness::wait_for_available_commands_update;
use acp_runtime_harness::wait_for_prompt_update_and_response;
use acp_runtime_harness::wait_for_replayed_history;
use acp_runtime_harness::wait_for_response;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;

#[tokio::test]
async fn acp_session_list_filters_and_paginates_with_cursor() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, _notifications_rx) = initialize_acp_connection(&runtime).await?;
    let primary_cwd = data_root.path().join("primary");
    let other_cwd = data_root.path().join("other");
    std::fs::create_dir_all(&primary_cwd)?;
    std::fs::create_dir_all(&other_cwd)?;

    let mut expected_ids = HashSet::new();
    for index in 0..51 {
        expected_ids
            .insert(create_acp_session(&runtime, connection_id, &primary_cwd, 100 + index).await?);
    }
    let other_id = create_acp_session(&runtime, connection_id, &other_cwd, 200).await?;

    let first_page = list_acp_sessions(&runtime, connection_id, 300, Some(&primary_cwd), None)
        .await
        .context("first session/list page")?;
    assert_eq!(first_page.sessions.len(), 50);
    assert!(first_page.next_cursor.is_some());
    assert!(
        first_page
            .sessions
            .iter()
            .all(|session| session.cwd == primary_cwd)
    );

    let second_page = list_acp_sessions(
        &runtime,
        connection_id,
        301,
        Some(&primary_cwd),
        first_page.next_cursor.clone(),
    )
    .await
    .context("second session/list page")?;
    assert_eq!(second_page.sessions.len(), 1);
    assert_eq!(second_page.next_cursor, None);

    let actual_ids = first_page
        .sessions
        .into_iter()
        .chain(second_page.sessions)
        .map(|session| session.session_id)
        .collect::<HashSet<_>>();
    assert_eq!(actual_ids, expected_ids);
    assert!(!actual_ids.contains(&other_id));

    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 302,
                "method": "session/list",
                "params": {
                    "cursor": "not-a-valid-cursor"
                }
            }),
        )
        .await
        .context("invalid cursor response")?;
    let error: AcpErrorResponse = serde_json::from_value(response)?;
    assert_eq!(error.error.code, -32602);
    assert_eq!(error.error.message, "session/list cursor is invalid");
    Ok(())
}

#[tokio::test]
async fn acp_session_list_orders_by_last_activity_not_metadata_update() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, _notifications_rx) = initialize_acp_connection(&runtime).await?;
    let cwd = data_root.path().join("project");
    std::fs::create_dir_all(&cwd)?;

    let first_id = create_acp_session(&runtime, connection_id, &cwd, 10).await?;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let second_id = create_acp_session(&runtime, connection_id, &cwd, 11).await?;

    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 12,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": first_id,
                    "expectedVersion": 0,
                    "title": "Metadata-only rename"
                }
            }),
        )
        .await
        .context("session/metadata/update response")?;

    let listed = list_acp_sessions(&runtime, connection_id, 13, Some(&cwd), None).await?;

    assert_eq!(
        listed
            .sessions
            .iter()
            .take(2)
            .map(|session| session.session_id)
            .collect::<Vec<_>>(),
        vec![second_id, first_id]
    );
    Ok(())
}

#[tokio::test]
async fn acp_session_load_replays_history_and_rejects_relative_roots() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_acp_connection(&runtime).await?;
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;
    let session_id = create_acp_session(&runtime, connection_id, &cwd, 10).await?;

    send_session_prompt(
        &runtime,
        connection_id,
        11,
        session_id,
        "write one ACP lifecycle test reply",
    )
    .await?;
    assert_eq!(
        wait_for_response::<AcpPromptResult>(&mut notifications_rx, 11)
            .await?
            .result
            .stop_reason,
        AcpStopReason::EndTurn
    );

    let (load_connection_id, mut load_notifications_rx) =
        initialize_acp_connection(&runtime).await?;
    assert!(
        session_load(
            &runtime,
            load_connection_id,
            12,
            session_id,
            &cwd,
            serde_json::json!({})
        )
        .await?["result"]
            .is_object()
    );
    let replayed_updates = wait_for_replayed_history(&mut load_notifications_rx).await?;
    assert!(
        replayed_updates
            .iter()
            .any(|update| matches!(update, AcpSessionUpdate::UserMessageChunk { .. }))
    );
    assert!(
        replayed_updates
            .iter()
            .any(|update| matches!(update, AcpSessionUpdate::AgentMessageChunk { .. }))
    );

    let (resume_connection_id, mut resume_notifications_rx) =
        initialize_acp_connection(&runtime).await?;
    assert!(
        session_resume(
            &runtime,
            resume_connection_id,
            13,
            session_id,
            &cwd,
            serde_json::json!({})
        )
        .await?["result"]
            .is_object()
    );
    assert_no_replayed_history(&mut resume_notifications_rx).await?;

    for (request_id, method, params, expected_message) in [
        (
            20,
            "session/list",
            serde_json::json!({ "cwd": "relative" }),
            "session/list cwd must be an absolute path",
        ),
        (
            21,
            "session/new",
            serde_json::json!({ "cwd": "relative", "mcpServers": [] }),
            "session/new cwd must be an absolute path",
        ),
    ] {
        assert_acp_error_message(
            &runtime,
            connection_id,
            serde_json::json!({ "id": request_id, "method": method, "params": params }),
            expected_message,
        )
        .await?;
    }
    for (request_id, method, params, expected_message) in [
        (
            22,
            "session/load",
            serde_json::json!({
                "sessionId": session_id,
                "cwd": "relative",
                "mcpServers": []
            }),
            "session/load cwd must be an absolute path",
        ),
        (
            23,
            "session/resume",
            serde_json::json!({
                "sessionId": session_id,
                "cwd": "relative",
                "mcpServers": []
            }),
            "session/resume cwd must be an absolute path",
        ),
        (
            24,
            "session/new",
            serde_json::json!({
                "cwd": path_value(&cwd),
                "additionalDirectories": ["relative"],
                "mcpServers": []
            }),
            "session/new additionalDirectories[0] must be an absolute path",
        ),
    ] {
        assert_acp_error_message(
            &runtime,
            connection_id,
            serde_json::json!({ "id": request_id, "method": method, "params": params }),
            expected_message,
        )
        .await?;
    }

    assert_removed_session_method(&runtime, connection_id, 25, "legacy/session/start").await?;
    assert_removed_session_method(&runtime, connection_id, 26, "legacy/session/list").await?;
    Ok(())
}

#[tokio::test]
async fn acp_session_prompt_streams_session_updates_without_devo_subscriptions() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let connection =
        initialize_acp_connection_with_transport(&runtime, ClientTransportKind::WebSocket).await?;
    let connection_id = connection.connection_id;
    let mut notifications_rx = connection.notifications_rx;
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;
    let session_id = create_acp_session(&runtime, connection_id, &cwd, 50).await?;

    send_session_prompt(
        &runtime,
        connection_id,
        51,
        session_id,
        "stream an ACP reply over websocket",
    )
    .await?;

    let (updates_before_response, updates_after_response, prompt_result): (
        Vec<AcpSessionUpdate>,
        Vec<AcpSessionUpdate>,
        AcpSuccessResponse<AcpPromptResult>,
    ) = wait_for_prompt_update_and_response(&mut notifications_rx, 51, session_id).await?;
    assert_eq!(prompt_result.result.stop_reason, AcpStopReason::EndTurn);
    assert!(
        updates_before_response
            .iter()
            .any(|update| matches!(update, AcpSessionUpdate::AgentMessageChunk { .. })),
        "expected ACP prompt turn to emit a native agent_message_chunk before the response; before={updates_before_response:?} after={updates_after_response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn acp_sessions_advertise_server_backed_slash_commands() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_acp_connection(&runtime).await?;
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;

    let session_id = create_acp_session(&runtime, connection_id, &cwd, 70).await?;
    assert_acp_slash_command_advertisement(
        &wait_for_available_commands_update(&mut notifications_rx, session_id).await?,
    );

    for (request_id, method) in [(71, "session/load"), (72, "session/resume")] {
        let (conn_id, mut rx) = initialize_acp_connection(&runtime).await?;
        let response = runtime
            .handle_incoming(
                conn_id,
                serde_json::json!({
                    "id": request_id,
                    "method": method,
                    "params": {
                        "sessionId": session_id,
                        "cwd": path_value(&cwd),
                        "mcpServers": []
                    }
                }),
            )
            .await
            .with_context(|| format!("{method} response"))?;
        assert!(response["result"].is_object());
        assert_acp_slash_command_advertisement(
            &wait_for_available_commands_update(&mut rx, session_id).await?,
        );
    }
    Ok(())
}

#[tokio::test]
async fn acp_session_prompt_runs_goal_slash_command_and_rejects_tui_only_command() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_acp_connection(&runtime).await?;
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;
    let session_id = create_acp_session(&runtime, connection_id, &cwd, 72).await?;

    assert_acp_error_message(
        &runtime,
        connection_id,
        acp_request(
            74,
            "session/prompt",
            serde_json::json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "/theme" }]
            }),
        ),
        "/theme is a TUI command and is not available over ACP",
    )
    .await?;

    let goal_response: AcpSuccessResponse<AcpPromptResult> = serde_json::from_value(handle_acp(
        &runtime,
        connection_id,
        acp_request(
            73,
            "session/prompt",
            serde_json::json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "/goal improve ACP slash command support" }]
            }),
        ),
    )
    .await?)?;
    assert_eq!(goal_response.result.stop_reason, AcpStopReason::EndTurn);
    assert_eq!(
        wait_for_agent_text_update(&mut notifications_rx, session_id).await?,
        "Goal set: improve ACP slash command support"
    );

    Ok(())
}

#[tokio::test]
async fn acp_session_additional_directories_roundtrip_new_load_and_resume() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, _notifications_rx) = initialize_acp_connection(&runtime).await?;
    let cwd = data_root.path().join("repo");
    let roots = ["first-root", "load-root", "resume-root"]
        .map(|name| data_root.path().join(name))
        .into_iter()
        .collect::<Vec<_>>();
    for path in [&cwd, &roots[0], &roots[1], &roots[2]] {
        std::fs::create_dir_all(path)?;
    }

    let new_session = acp_runtime_harness::create_acp_session(
        &runtime,
        connection_id,
        &cwd,
        13,
        serde_json::json!({ "additionalDirectories": [path_value(&roots[0])] }),
    )
    .await?;
    assert_eq!(
        decode_native_session_meta(&new_session.meta)?.additional_directories,
        vec![roots[0].clone()]
    );
    let session_id = new_session.session_id;
    assert_eq!(
        list_acp_sessions(&runtime, connection_id, 14, Some(&cwd), None)
            .await?
            .sessions[0]
            .additional_directories,
        vec![roots[0].clone()]
    );

    for (request_id, method, root, check_meta) in [
        (15, "load", &roots[1], false),
        (17, "resume", &roots[2], true),
    ] {
        let extra = serde_json::json!({ "additionalDirectories": [path_value(root)] });
        let response = if method == "load" {
            session_load(&runtime, connection_id, request_id, session_id, &cwd, extra).await?
        } else {
            session_resume(&runtime, connection_id, request_id, session_id, &cwd, extra).await?
        };
        assert!(response["result"].is_object());
        if check_meta {
            let resumed: AcpSuccessResponse<AcpResumeSessionResult> =
                serde_json::from_value(response)?;
            assert_eq!(
                decode_native_session_meta(&resumed.result.meta)?.additional_directories,
                vec![root.clone()]
            );
        }
        assert_eq!(
            list_acp_sessions(&runtime, connection_id, request_id + 1, Some(&cwd), None)
                .await?
                .sessions[0]
                .additional_directories,
            vec![root.clone()]
        );
    }

    let restored_runtime = build_runtime(data_root.path())?;
    restored_runtime.load_persisted_sessions().await?;
    let (restored_connection_id, _) = initialize_acp_connection(&restored_runtime).await?;
    assert_eq!(
        list_acp_sessions(
            &restored_runtime,
            restored_connection_id,
            19,
            Some(&cwd),
            None
        )
        .await?
        .sessions[0]
            .additional_directories,
        vec![roots[2].clone()]
    );
    Ok(())
}

#[tokio::test]
async fn acp_session_load_and_resume_accept_mcp_servers() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, _notifications_rx) = initialize_acp_connection(&runtime).await?;
    let cwd = data_root.path().join("repo");
    let load_mcp_command = data_root.path().join("missing-load-mcp-server");
    let resume_mcp_command = data_root.path().join("missing-resume-mcp-server");
    std::fs::create_dir_all(&cwd)?;

    let session_id = create_acp_session(&runtime, connection_id, &cwd, 19).await?;
    let wrong_cwd = data_root.path().join("wrong-repo");
    std::fs::create_dir_all(&wrong_cwd)?;

    for (request_id, method, tool_name, expected_message) in [
        (
            22,
            "session/load",
            "rejected-load-tools",
            "session/load cwd does not match the stored session cwd",
        ),
        (
            23,
            "session/resume",
            "rejected-resume-tools",
            "session/resume cwd does not match the stored session cwd",
        ),
    ] {
        assert_acp_error_message(
            &runtime,
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": method,
                "params": {
                    "sessionId": session_id,
                    "cwd": path_value(&wrong_cwd),
                    "mcpServers": [stdio_mcp_server_value(tool_name, &load_mcp_command)]
                }
            }),
            expected_message,
        )
        .await?;
    }

    for (request_id, method, tool_name, command) in [
        (20, "load", "load-tools", &load_mcp_command),
        (21, "resume", "resume-tools", &resume_mcp_command),
    ] {
        let extra = serde_json::json!({
            "mcpServers": [stdio_mcp_server_value(tool_name, command)]
        });
        let response = if method == "load" {
            session_load(&runtime, connection_id, request_id, session_id, &cwd, extra).await?
        } else {
            session_resume(&runtime, connection_id, request_id, session_id, &cwd, extra).await?
        };
        assert!(response["result"].is_object());
        if method == "load" {
            let loaded: AcpSuccessResponse<AcpLoadSessionResult> =
                serde_json::from_value(response)?;
            assert!(loaded.result.config_options.is_some());
        }
    }
    Ok(())
}

#[tokio::test]
async fn acp_auth_gates_acp_methods_on_connection() -> Result<()> {
    let data_root = TempDir::new()?;
    std::fs::write(
        data_root.path().join("config.toml"),
        r#"
[server.auth]
enabled = true
method_id = "agent-login"
name = "Agent login"
description = "Use the test login flow"
logout = true
"#,
    )?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, _notifications_rx, initialize) =
        initialize_acp_connection_with_response(&runtime).await?;
    assert_eq!(
        initialize.auth_methods,
        vec![AcpAuthMethod::agent(
            "agent-login",
            "Agent login",
            Some("Use the test login flow".to_string())
        )]
    );
    assert_eq!(
        initialize.agent_capabilities.auth.logout,
        Some(AcpLogoutCapabilities::default())
    );
    assert!(
        initialize
            .meta
            .as_ref()
            .is_some_and(|meta| !meta.contains_key("devo/serverHome"))
    );
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;

    assert_auth_required(
        &runtime,
        connection_id,
        acp_request(
            30,
            "session/new",
            serde_json::json!({ "cwd": path_value(&cwd), "mcpServers": [] }),
        ),
    )
    .await?;
    assert_eq!(
        serde_json::from_value::<AcpErrorResponse>(
            handle_acp(
                &runtime,
                connection_id,
                acp_request(
                    32,
                    "authenticate",
                    serde_json::json!({ "methodId": "wrong-login" })
                ),
            )
            .await?
        )?
        .error
        .code,
        -32602
    );
    assert_eq!(
        handle_acp(
            &runtime,
            connection_id,
            acp_request(
                33,
                "authenticate",
                serde_json::json!({ "methodId": "agent-login" })
            ),
        )
        .await?,
        serde_json::json!({ "jsonrpc": "2.0", "id": 33, "result": {} })
    );

    let session_id = create_acp_session(&runtime, connection_id, &cwd, 34).await?;
    assert!(
        list_acp_sessions(&runtime, connection_id, 35, None, None)
            .await?
            .sessions
            .iter()
            .any(|session| session.session_id == session_id)
    );
    assert_eq!(
        handle_acp(
            &runtime,
            connection_id,
            acp_request(36, "logout", serde_json::json!({}))
        )
        .await?,
        serde_json::json!({ "jsonrpc": "2.0", "id": 36, "result": {} })
    );
    assert_auth_required(
        &runtime,
        connection_id,
        acp_request(37, "session/list", serde_json::json!({})),
    )
    .await?;
    assert_eq!(
        runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "method": "session/cancel",
                    "params": { "sessionId": session_id }
                }),
            )
            .await,
        None
    );
    Ok(())
}

#[tokio::test]
async fn legacy_initialize_params_are_rejected() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (notifications_tx, _notifications_rx) = devo_server::test_outbound_channel(4096);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let initialize_response = handle_acp(
        &runtime,
        connection_id,
        acp_request(
            40,
            "initialize",
            serde_json::json!({
                "client_name": "legacy-auth-test",
                "client_version": "1.0.0",
                "transport": "stdio",
                "supports_streaming": true,
                "supports_binary_images": false,
                "opt_out_notification_methods": []
            }),
        ),
    )
    .await?;
    let error: AcpErrorResponse = serde_json::from_value(initialize_response)?;
    assert_eq!(error.error.code, -32602);
    assert!(error.error.message.contains("invalid initialize params"));
    Ok(())
}

fn build_runtime(data_root: &Path) -> Result<Arc<ServerRuntime>> {
    build_acp_test_runtime(
        data_root,
        Arc::new(SingleReplyProvider),
        "acp_session_lifecycle.db",
    )
}

const LIFECYCLE_CLIENT: AcpTestClient =
    AcpTestClient::new("acp-session-lifecycle-test", "ACP Session Lifecycle Test");

async fn initialize_acp_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let connection =
        initialize_acp_connection_with_transport(runtime, ClientTransportKind::Stdio).await?;
    Ok((connection.connection_id, connection.notifications_rx))
}

async fn initialize_acp_connection_with_response(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>, AcpInitializeResult)> {
    let connection =
        initialize_acp_connection_with_transport(runtime, ClientTransportKind::Stdio).await?;
    Ok((
        connection.connection_id,
        connection.notifications_rx,
        connection.initialize,
    ))
}

async fn initialize_acp_connection_with_transport(
    runtime: &Arc<ServerRuntime>,
    transport: ClientTransportKind,
) -> Result<acp_runtime_harness::AcpConnection> {
    let connection = connect_acp(runtime, transport, LIFECYCLE_CLIENT, 1).await?;
    assert!(connection.initialize.agent_capabilities.load_session);
    assert!(
        connection
            .initialize
            .agent_capabilities
            .session_capabilities
            .list
            .is_some()
    );
    Ok(connection)
}

async fn create_acp_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &Path,
    request_id: u64,
) -> Result<SessionId> {
    create_acp_session_id(runtime, connection_id, cwd, request_id).await
}

#[tokio::test]
async fn acp_session_list_includes_live_actor_missing_from_database() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_acp_connection(&runtime).await?;
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;
    let session_id = create_acp_session(&runtime, connection_id, &cwd, 10).await?;

    devo_server::db::Database::open(data_root.path().join("test_persistence.db"))?
        .delete_session(&session_id)?;
    let listed = list_acp_sessions(&runtime, connection_id, 11, None, None)
        .await?
        .sessions
        .iter()
        .filter_map(|session| session.meta.as_ref())
        .filter_map(|meta| meta.get(devo_server::DEVO_SESSION_META))
        .map(|value| serde_json::from_value::<Session>(value.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        listed
            .iter()
            .filter(|session| session.id.as_str() == session_id.to_string())
            .count(),
        1
    );

    send_session_prompt(
        &runtime,
        connection_id,
        11,
        session_id,
        "persist for lazy ACP resume",
    )
    .await?;
    let _: AcpSuccessResponse<AcpPromptResult> =
        wait_for_response(&mut notifications_rx, 11).await?;
    let restored_runtime = build_runtime(data_root.path())?;
    restored_runtime.refresh_session_index()?;
    let (restored_connection_id, mut restored_notifications_rx) =
        initialize_acp_connection(&restored_runtime).await?;
    assert!(
        session_resume(
            &restored_runtime,
            restored_connection_id,
            12,
            session_id,
            &cwd,
            serde_json::json!({})
        )
        .await?["result"]
            .is_object()
    );
    assert_no_replayed_history(&mut restored_notifications_rx).await?;
    Ok(())
}
