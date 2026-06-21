use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

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
use devo_server::AcpDeleteSessionResult;
use devo_server::AcpInitializeResult;
use devo_server::AcpListSessionsResult;
use devo_server::AcpNewSessionResult;
use devo_server::AcpSessionDeleteCapabilities;
use devo_server::AcpSuccessResponse;
use devo_server::ClientTransportKind;
use devo_server::DEVO_SESSION_META;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use devo_server::acp_session_info_from_metadata;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;

struct NoopProvider;

#[async_trait]
impl ModelProviderSDK for NoopProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "noop-response".to_string(),
            content: vec![ResponseContent::Text("noop".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(stream::empty()))
    }

    fn name(&self) -> &str {
        "noop-acp-delete-provider"
    }
}

#[tokio::test]
async fn acp_session_delete_removes_session_from_history_and_is_idempotent() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(NoopProvider);
    let db = Arc::new(devo_server::db::Database::open(
        data_root.path().join("acp_session_delete.db"),
    )?);
    let runtime = ServerRuntime::new(
        data_root.path().to_path_buf(),
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
            Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                bundled: Some(BundledSkillsConfig { enabled: false }),
                ..SkillsConfig::default()
            })),
            devo_core::AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(AppConfigStore::load(
                data_root.path().to_path_buf(),
                None,
            )?)),
        ),
    );
    let (notifications_tx, _notifications_rx) = mpsc::channel(/*buffer*/ 4096);
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
                        "name": "acp-session-delete-test",
                        "title": "ACP Session Delete Test",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    let initialize: AcpSuccessResponse<AcpInitializeResult> =
        serde_json::from_value(initialize_response)?;
    assert_eq!(
        initialize
            .result
            .agent_capabilities
            .session_capabilities
            .delete,
        Some(AcpSessionDeleteCapabilities::default())
    );

    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;
    let new_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": cwd.to_string_lossy().into_owned(),
                    "mcpServers": []
                }
            }),
        )
        .await
        .context("session/new response")?;
    let new_session: AcpSuccessResponse<AcpNewSessionResult> =
        serde_json::from_value(new_response)?;
    let session_id = new_session.result.session_id;
    let session_metadata: SessionMetadata = serde_json::from_value(
        new_session
            .result
            .meta
            .as_ref()
            .and_then(|meta| meta.get(DEVO_SESSION_META))
            .cloned()
            .context("missing Devo session metadata")?,
    )?;

    assert_eq!(
        list_acp_sessions(&runtime, connection_id, 3, &cwd).await?,
        AcpListSessionsResult {
            sessions: vec![acp_session_info_from_metadata(&session_metadata)],
            next_cursor: None,
            meta: None,
        }
    );
    assert_eq!(
        delete_acp_session(&runtime, connection_id, 4, &session_id).await?,
        AcpSuccessResponse::new(serde_json::json!(4), AcpDeleteSessionResult::default())
    );
    assert_eq!(
        list_acp_sessions(&runtime, connection_id, 5, &cwd).await?,
        AcpListSessionsResult {
            sessions: Vec::new(),
            next_cursor: None,
            meta: None,
        }
    );
    assert_eq!(
        delete_acp_session(&runtime, connection_id, 6, &session_id).await?,
        AcpSuccessResponse::new(serde_json::json!(6), AcpDeleteSessionResult::default())
    );
    Ok(())
}

async fn list_acp_sessions(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    cwd: &Path,
) -> Result<AcpListSessionsResult> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "session/list",
                "params": {
                    "cwd": cwd.to_string_lossy().into_owned()
                }
            }),
        )
        .await
        .context("session/list response")?;
    Ok(serde_json::from_value::<AcpSuccessResponse<AcpListSessionsResult>>(response)?.result)
}

async fn delete_acp_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: &SessionId,
) -> Result<AcpSuccessResponse<AcpDeleteSessionResult>> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "session/delete",
                "params": {
                    "sessionId": session_id
                }
            }),
        )
        .await
        .context("session/delete response")?;
    serde_json::from_value(response).context("decode session/delete response")
}
