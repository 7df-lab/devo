use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AppConfigStore;
use devo_core::BundledSkillsConfig;
use devo_core::FileSystemSkillCatalog;
use devo_core::PresetModelCatalog;
use devo_core::ProviderVendorCatalog;
use devo_core::SkillsConfig;
use devo_core::tools::ToolRegistry;
use devo_protocol::AcpAuthMethod;
use devo_protocol::AcpLoadSessionResult;
use devo_protocol::AcpNewSessionResult;
use devo_protocol::AcpPromptResult;
use devo_protocol::AcpResumeSessionResult;
use devo_protocol::AcpSessionNotification;
use devo_protocol::AcpSessionUpdate;
use devo_protocol::AcpStopReason;
use devo_protocol::Model;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::SessionId;
use devo_protocol::SessionMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::AcpErrorResponse;
use devo_server::AcpInitializeResult;
use devo_server::AcpListSessionsResult;
use devo_server::AcpSuccessResponse;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::timeout;

struct SingleReplyProvider;

#[async_trait]
impl ModelProviderSDK for SingleReplyProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title-1".into(),
            content: vec![ResponseContent::Text("Generated ACP title".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Hello from ACP lifecycle test.".into(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "resp-1".into(),
                    content: vec![ResponseContent::Text(
                        "Hello from ACP lifecycle test.".into(),
                    )],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: ResponseMetadata::default(),
                },
            }),
        ])))
    }

    fn name(&self) -> &str {
        "single-reply-acp-provider"
    }
}

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
async fn acp_session_load_replays_history_and_rejects_relative_roots() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, mut notifications_rx) = initialize_acp_connection(&runtime).await?;
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;
    let session_id = create_acp_session(&runtime, connection_id, &cwd, 10).await?;

    let prompt_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 11,
                "method": "session/prompt",
                "params": {
                    "sessionId": session_id,
                    "prompt": [
                        {
                            "type": "text",
                            "text": "write one ACP lifecycle test reply"
                        }
                    ]
                }
            }),
        )
        .await;
    assert_eq!(prompt_response, None);
    let prompt_result: AcpSuccessResponse<AcpPromptResult> =
        wait_for_response(&mut notifications_rx, 11).await?;
    assert_eq!(prompt_result.result.stop_reason, AcpStopReason::EndTurn);

    let (load_connection_id, mut load_notifications_rx) =
        initialize_acp_connection(&runtime).await?;
    let load_response = runtime
        .handle_incoming(
            load_connection_id,
            serde_json::json!({
                "id": 12,
                "method": "session/load",
                "params": {
                    "sessionId": session_id,
                    "cwd": path_value(&cwd),
                    "mcpServers": []
                }
            }),
        )
        .await
        .context("session/load response")?;
    assert_eq!(load_response["result"], serde_json::Value::Null);
    let _: AcpSuccessResponse<AcpLoadSessionResult> = serde_json::from_value(load_response)?;
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
    let resume_response = runtime
        .handle_incoming(
            resume_connection_id,
            serde_json::json!({
                "id": 13,
                "method": "session/resume",
                "params": {
                    "sessionId": session_id,
                    "cwd": path_value(&cwd),
                    "mcpServers": []
                }
            }),
        )
        .await
        .context("session/resume response")?;
    assert!(resume_response["result"].is_object());
    let _: AcpSuccessResponse<AcpResumeSessionResult> = serde_json::from_value(resume_response)?;
    assert_no_replayed_history(&mut resume_notifications_rx).await?;

    assert_acp_error_message(
        &runtime,
        connection_id,
        serde_json::json!({
            "id": 20,
            "method": "session/list",
            "params": {
                "cwd": "relative"
            }
        }),
        "session/list cwd must be an absolute path",
    )
    .await?;
    assert_acp_error_message(
        &runtime,
        connection_id,
        serde_json::json!({
            "id": 21,
            "method": "session/new",
            "params": {
                "cwd": "relative",
                "mcpServers": []
            }
        }),
        "session/new cwd must be an absolute path",
    )
    .await?;
    assert_acp_error_message(
        &runtime,
        connection_id,
        serde_json::json!({
            "id": 22,
            "method": "session/load",
            "params": {
                "sessionId": session_id,
                "cwd": "relative",
                "mcpServers": []
            }
        }),
        "session/load cwd must be an absolute path",
    )
    .await?;
    assert_acp_error_message(
        &runtime,
        connection_id,
        serde_json::json!({
            "id": 23,
            "method": "session/resume",
            "params": {
                "sessionId": session_id,
                "cwd": "relative",
                "mcpServers": []
            }
        }),
        "session/resume cwd must be an absolute path",
    )
    .await?;
    assert_acp_error_message(
        &runtime,
        connection_id,
        serde_json::json!({
            "id": 24,
            "method": "session/new",
            "params": {
                "cwd": path_value(&cwd),
                "additionalDirectories": ["relative"],
                "mcpServers": []
            }
        }),
        "session/new additionalDirectories[0] must be an absolute path",
    )
    .await?;

    assert_legacy_session_method_removed(&runtime, connection_id, 25, "_devo/session/start")
        .await?;
    assert_legacy_session_method_removed(&runtime, connection_id, 26, "_devo/session/list").await?;
    Ok(())
}

#[tokio::test]
async fn acp_session_additional_directories_roundtrip_new_load_and_resume() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (connection_id, _notifications_rx) = initialize_acp_connection(&runtime).await?;
    let cwd = data_root.path().join("repo");
    let first_root = data_root.path().join("first-root");
    let load_root = data_root.path().join("load-root");
    let resume_root = data_root.path().join("resume-root");
    std::fs::create_dir_all(&cwd)?;
    std::fs::create_dir_all(&first_root)?;
    std::fs::create_dir_all(&load_root)?;
    std::fs::create_dir_all(&resume_root)?;

    let new_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 13,
                "method": "session/new",
                "params": {
                    "cwd": path_value(&cwd),
                    "additionalDirectories": [path_value(&first_root)],
                    "mcpServers": []
                }
            }),
        )
        .await
        .context("session/new with additionalDirectories response")?;
    let new_session: AcpSuccessResponse<AcpNewSessionResult> =
        serde_json::from_value(new_response)?;
    assert_eq!(
        decode_devo_session_meta(&new_session.result.meta)?.additional_directories,
        vec![first_root.clone()]
    );
    let session_id = new_session.result.session_id;

    let listed = list_acp_sessions(&runtime, connection_id, 14, Some(&cwd), None).await?;
    assert_eq!(listed.sessions.len(), 1);
    assert_eq!(listed.sessions[0].additional_directories, vec![first_root]);

    let load_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 15,
                "method": "session/load",
                "params": {
                    "sessionId": session_id,
                    "cwd": path_value(&cwd),
                    "additionalDirectories": [path_value(&load_root)],
                    "mcpServers": []
                }
            }),
        )
        .await
        .context("session/load with additionalDirectories response")?;
    assert_eq!(load_response["result"], serde_json::Value::Null);
    let _: AcpSuccessResponse<AcpLoadSessionResult> = serde_json::from_value(load_response)?;
    let listed = list_acp_sessions(&runtime, connection_id, 16, Some(&cwd), None).await?;
    assert_eq!(listed.sessions[0].additional_directories, vec![load_root]);

    let resume_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 17,
                "method": "session/resume",
                "params": {
                    "sessionId": session_id,
                    "cwd": path_value(&cwd),
                    "additionalDirectories": [path_value(&resume_root)],
                    "mcpServers": []
                }
            }),
        )
        .await
        .context("session/resume with additionalDirectories response")?;
    let resumed: AcpSuccessResponse<AcpResumeSessionResult> =
        serde_json::from_value(resume_response)?;
    assert_eq!(
        decode_devo_session_meta(&resumed.result.meta)?.additional_directories,
        vec![resume_root.clone()]
    );
    let listed = list_acp_sessions(&runtime, connection_id, 18, Some(&cwd), None).await?;
    assert_eq!(
        listed.sessions[0].additional_directories,
        vec![resume_root.clone()]
    );

    let restored_runtime = build_runtime(data_root.path())?;
    restored_runtime.load_persisted_sessions().await?;
    let (restored_connection_id, _notifications_rx) =
        initialize_acp_connection(&restored_runtime).await?;
    let restored_listed = list_acp_sessions(
        &restored_runtime,
        restored_connection_id,
        19,
        Some(&cwd),
        None,
    )
    .await?;
    assert_eq!(
        restored_listed.sessions[0].additional_directories,
        vec![resume_root]
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

    assert_acp_error_message(
        &runtime,
        connection_id,
        serde_json::json!({
            "id": 22,
            "method": "session/load",
            "params": {
                "sessionId": session_id,
                "cwd": path_value(&wrong_cwd),
                "mcpServers": [stdio_mcp_server_value("rejected-load-tools", &load_mcp_command)]
            }
        }),
        "session/load cwd does not match the stored session cwd",
    )
    .await?;
    assert_acp_error_message(
        &runtime,
        connection_id,
        serde_json::json!({
            "id": 23,
            "method": "session/resume",
            "params": {
                "sessionId": session_id,
                "cwd": path_value(&wrong_cwd),
                "mcpServers": [stdio_mcp_server_value("rejected-resume-tools", &resume_mcp_command)]
            }
        }),
        "session/resume cwd does not match the stored session cwd",
    )
    .await?;

    let load_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 20,
                "method": "session/load",
                "params": {
                    "sessionId": session_id,
                    "cwd": path_value(&cwd),
                    "mcpServers": [stdio_mcp_server_value("load-tools", &load_mcp_command)]
                }
            }),
        )
        .await
        .context("session/load with mcpServers response")?;
    assert_eq!(load_response["result"], serde_json::Value::Null);
    let _: AcpSuccessResponse<AcpLoadSessionResult> = serde_json::from_value(load_response)?;

    let resume_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 21,
                "method": "session/resume",
                "params": {
                    "sessionId": session_id,
                    "cwd": path_value(&cwd),
                    "mcpServers": [stdio_mcp_server_value("resume-tools", &resume_mcp_command)]
                }
            }),
        )
        .await
        .context("session/resume with mcpServers response")?;
    assert!(resume_response["result"].is_object());
    let _: AcpSuccessResponse<AcpResumeSessionResult> = serde_json::from_value(resume_response)?;
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
        Some(serde_json::json!({}))
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
        serde_json::json!({
            "id": 30,
            "method": "session/new",
            "params": {
                "cwd": path_value(&cwd),
                "mcpServers": []
            }
        }),
    )
    .await?;
    let invalid_auth_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 32,
                "method": "authenticate",
                "params": {
                    "methodId": "wrong-login"
                }
            }),
        )
        .await
        .context("invalid authenticate response")?;
    let invalid_auth_error: AcpErrorResponse = serde_json::from_value(invalid_auth_response)?;
    assert_eq!(invalid_auth_error.error.code, -32602);

    let auth_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 33,
                "method": "authenticate",
                "params": {
                    "methodId": "agent-login"
                }
            }),
        )
        .await
        .context("authenticate response")?;
    assert_eq!(
        auth_response,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 33,
            "result": {}
        })
    );

    let session_id = create_acp_session(&runtime, connection_id, &cwd, 34).await?;
    let sessions = list_acp_sessions(&runtime, connection_id, 35, None, None).await?;
    assert!(
        sessions
            .sessions
            .iter()
            .any(|session| session.session_id == session_id)
    );

    let logout_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 36,
                "method": "logout",
                "params": {}
            }),
        )
        .await
        .context("logout response")?;
    assert_eq!(
        logout_response,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 36,
            "result": {}
        })
    );

    assert_auth_required(
        &runtime,
        connection_id,
        serde_json::json!({
            "id": 37,
            "method": "session/list",
            "params": {}
        }),
    )
    .await?;
    let cancel_notification = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "method": "session/cancel",
                "params": {
                    "sessionId": session_id
                }
            }),
        )
        .await;
    assert_eq!(cancel_notification, None);
    Ok(())
}

#[tokio::test]
async fn legacy_initialize_params_are_rejected() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (notifications_tx, _notifications_rx) = mpsc::channel(/*buffer*/ 4096);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let initialize_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 40,
                "method": "initialize",
                "params": {
                    "client_name": "legacy-auth-test",
                    "client_version": "1.0.0",
                    "transport": "stdio",
                    "supports_streaming": true,
                    "supports_binary_images": false,
                    "opt_out_notification_methods": []
                }
            }),
        )
        .await
        .context("legacy initialize response")?;
    let error: AcpErrorResponse = serde_json::from_value(initialize_response)?;
    assert_eq!(error.error.code, -32602);
    assert!(error.error.message.contains("invalid initialize params"));
    Ok(())
}

fn build_runtime(data_root: &Path) -> Result<Arc<ServerRuntime>> {
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(SingleReplyProvider);
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("acp_session_lifecycle.db"),
    )?);
    Ok(ServerRuntime::new(
        data_root.to_path_buf(),
        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(SingleProviderRouter::new(provider)),
            Arc::new(ToolRegistry::new()),
            "test-model".to_string(),
            Arc::new(PresetModelCatalog::new(vec![Model {
                slug: "test-model".to_string(),
                display_name: "test-model".to_string(),
                ..Model::default()
            }])),
            Arc::new(ProviderVendorCatalog::default()),
            None,
            Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                bundled: Some(BundledSkillsConfig { enabled: false }),
                ..SkillsConfig::default()
            })),
            devo_core::AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(AppConfigStore::load(
                data_root.to_path_buf(),
                None,
            )?)),
        ),
    ))
}

async fn initialize_acp_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (connection_id, notifications_rx, _) =
        initialize_acp_connection_with_response(runtime).await?;
    Ok((connection_id, notifications_rx))
}

async fn initialize_acp_connection_with_response(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>, AcpInitializeResult)> {
    let (notifications_tx, notifications_rx) = mpsc::channel(/*buffer*/ 4096);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let initialize_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": {
                        "name": "acp-session-lifecycle-test",
                        "title": "ACP Session Lifecycle Test",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    let response: AcpSuccessResponse<AcpInitializeResult> =
        serde_json::from_value(initialize_response)?;
    let initialize_result = response.result;
    assert!(initialize_result.agent_capabilities.load_session);
    assert!(
        initialize_result
            .agent_capabilities
            .session_capabilities
            .list
            .is_some()
    );
    Ok((connection_id, notifications_rx, initialize_result))
}

async fn create_acp_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &Path,
    request_id: u64,
) -> Result<SessionId> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "session/new",
                "params": {
                    "cwd": path_value(cwd),
                    "mcpServers": []
                }
            }),
        )
        .await
        .context("session/new response")?;
    let response: AcpSuccessResponse<AcpNewSessionResult> = serde_json::from_value(response)?;
    Ok(response.result.session_id)
}

async fn list_acp_sessions(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    cwd: Option<&Path>,
    cursor: Option<String>,
) -> Result<AcpListSessionsResult> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "session/list",
                "params": {
                    "cwd": cwd.map(path_value),
                    "cursor": cursor
                }
            }),
        )
        .await
        .context("session/list response")?;
    Ok(serde_json::from_value::<AcpSuccessResponse<AcpListSessionsResult>>(response)?.result)
}

async fn wait_for_response<T>(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    request_id: u64,
) -> Result<AcpSuccessResponse<T>>
where
    T: serde::de::DeserializeOwned,
{
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("id") == Some(&serde_json::json!(request_id)) {
                return serde_json::from_value(value).context("decode ACP response");
            }
        }
        anyhow::bail!("notification channel closed before response {request_id}")
    })
    .await
    .with_context(|| format!("timed out waiting for response {request_id}"))?
}

async fn wait_for_replayed_history(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<Vec<AcpSessionUpdate>> {
    timeout(Duration::from_secs(5), async {
        let mut updates = Vec::new();
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") != Some(&serde_json::json!("session/update")) {
                continue;
            }
            let notification: AcpSessionNotification =
                serde_json::from_value(value["params"].clone())?;
            if notification.meta.is_some() {
                continue;
            }
            updates.push(notification.update);
            let has_user = updates
                .iter()
                .any(|update| matches!(update, AcpSessionUpdate::UserMessageChunk { .. }));
            let has_agent = updates
                .iter()
                .any(|update| matches!(update, AcpSessionUpdate::AgentMessageChunk { .. }));
            if has_user && has_agent {
                return Ok(updates);
            }
        }
        anyhow::bail!("notification channel closed before replayed history")
    })
    .await
    .context("timed out waiting for replayed history")?
}

async fn assert_no_replayed_history(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<()> {
    let result = timeout(Duration::from_millis(100), async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") == Some(&serde_json::json!("session/update")) {
                anyhow::bail!("unexpected session/update notification: {value}");
            }
        }
        Ok(())
    })
    .await;

    match result {
        Ok(result) => result,
        Err(_) => Ok(()),
    }
}

async fn assert_acp_error_message(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    message: serde_json::Value,
    expected_message: &str,
) -> Result<()> {
    let response = runtime
        .handle_incoming(connection_id, message)
        .await
        .context("ACP error response")?;
    let error: AcpErrorResponse = serde_json::from_value(response)?;
    assert_eq!(error.error.code, -32602);
    assert_eq!(error.error.message, expected_message);
    Ok(())
}

async fn assert_auth_required(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    message: serde_json::Value,
) -> Result<()> {
    let response = runtime
        .handle_incoming(connection_id, message)
        .await
        .context("auth-required response")?;
    let error: AcpErrorResponse = serde_json::from_value(response)?;
    assert_eq!(error.error.code, -32000);
    assert_eq!(error.error.message, "Authentication required");
    assert_eq!(
        error.error.data,
        serde_json::json!({ "reason": "auth_required" })
    );
    Ok(())
}

async fn assert_legacy_session_method_removed(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    method: &str,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": method,
                "params": {}
            }),
        )
        .await
        .context("legacy session method response")?;
    let response: serde_json::Value = response;
    assert_eq!(response["id"], serde_json::json!(request_id));
    assert_eq!(
        response["error"]["code"],
        serde_json::json!("InvalidParams")
    );
    assert_eq!(
        response["error"]["message"],
        serde_json::json!(format!("unknown method: {method}"))
    );
    Ok(())
}

fn decode_devo_session_meta(meta: &Option<devo_protocol::AcpMeta>) -> Result<SessionMetadata> {
    let session = meta
        .as_ref()
        .and_then(|meta| meta.get(devo_protocol::DEVO_SESSION_META))
        .cloned()
        .context("missing Devo session metadata")?;
    serde_json::from_value(session).context("decode Devo session metadata")
}

fn path_value(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn stdio_mcp_server_value(name: &str, command: &Path) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "command": path_value(command),
        "args": ["--stdio"],
        "env": [
            {
                "name": "ACP_TEST",
                "value": "1"
            }
        ]
    })
}
