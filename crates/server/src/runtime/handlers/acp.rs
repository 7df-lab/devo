use super::super::*;
use super::session::RuntimeSessionToolRegistryUpdate;

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use devo_core::AppConfigStore;
use devo_core::McpConfig;
use devo_core::McpOutputLimits;
use devo_core::McpRootsPolicy;
use devo_core::McpServerId;
use devo_core::McpServerRecord;
use devo_core::McpStartupPolicy;
use devo_core::McpTransportConfig;
use devo_core::McpTrustPolicy;
use devo_core::tools::ToolPlanConfig;
use devo_mcp::manager::RmcpMcpManager;

use crate::ACP_SESSION_UPDATE_METHOD;
use crate::AcpAgentCapabilities;
use crate::AcpCancelParams;
use crate::AcpClientNotification;
use crate::AcpCloseSessionParams;
use crate::AcpCloseSessionResult;
use crate::AcpContentBlock;
use crate::AcpDeleteSessionParams;
use crate::AcpDeleteSessionResult;
use crate::AcpErrorCode;
use crate::AcpErrorResponse;
use crate::AcpImplementation;
use crate::AcpInitializeParams;
use crate::AcpInitializeResult;
use crate::AcpListSessionsParams;
use crate::AcpListSessionsResult;
use crate::AcpLoadSessionParams;
use crate::AcpLoadSessionResult;
use crate::AcpMcpCapabilities;
use crate::AcpMcpServer;
use crate::AcpMcpServerStdio;
use crate::AcpNewSessionParams;
use crate::AcpNewSessionResult;
use crate::AcpPlanEntry;
use crate::AcpPlanEntryPriority;
use crate::AcpPlanEntryStatus;
use crate::AcpPromptCapabilities;
use crate::AcpPromptParams;
use crate::AcpPromptResult;
use crate::AcpResumeSessionParams;
use crate::AcpResumeSessionResult;
use crate::AcpSessionAdditionalDirectoriesCapabilities;
use crate::AcpSessionCapabilities;
use crate::AcpSessionCloseCapabilities;
use crate::AcpSessionDeleteCapabilities;
use crate::AcpSessionListCapabilities;
use crate::AcpSessionNotification;
use crate::AcpSessionResumeCapabilities;
use crate::AcpSessionUpdate;
use crate::AcpSetConfigOptionParams;
use crate::AcpSetConfigOptionResult;
use crate::AcpSetModeParams;
use crate::AcpStopReason;
use crate::AcpSuccessResponse;
use crate::AcpToolCallContent;
use crate::AcpToolCallStatus;
use crate::AcpToolKind;
use crate::CollaborationMode;
use crate::DEVO_SESSION_META;
use crate::DEVO_SESSION_RESUME_META;
use crate::SessionHistoryItem;
use crate::SessionHistoryItemKind;
use crate::SessionHistoryMetadata;
use crate::SessionPlanStepStatus;
use crate::TurnExecutionMode;
use crate::acp_session_info_from_metadata;
use crate::input_items_from_acp_prompt;

const ACP_SESSION_LIST_PAGE_SIZE: usize = 50;
const ACP_SESSION_LIST_CURSOR_PREFIX: &str = "devo-session-list-v1:";

impl ServerRuntime {
    pub(crate) async fn handle_acp_initialize(
        &self,
        connection_id: u64,
        id: Option<serde_json::Value>,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let request_id = id.unwrap_or(serde_json::Value::Null);
        let params: AcpInitializeParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid initialize params: {error}"),
                );
            }
        };
        let acp_auth_config = self.acp_auth_config();
        if let Some(connection) = self.connections.lock().await.get_mut(&connection_id) {
            connection.state = ConnectionState::Ready;
            connection.acp_authenticated = !acp_auth_config.enabled;
            connection.acp_client_capabilities = params.client_capabilities.clone();
        }
        tracing::info!(
            connection_id,
            protocol_version = params.protocol_version,
            client = ?params.client_info.as_ref().map(|info| info.name.as_str()),
            "accepted ACP initialize request"
        );
        let mut meta = serde_json::Map::new();
        meta.insert(
            "devo/platformFamily".to_string(),
            serde_json::Value::String(self.metadata.platform_family.clone()),
        );
        meta.insert(
            "devo/platformOs".to_string(),
            serde_json::Value::String(self.metadata.platform_os.clone()),
        );
        if !acp_auth_config.enabled {
            meta.insert(
                "devo/serverHome".to_string(),
                serde_json::Value::String(self.metadata.server_home.display().to_string()),
            );
        }
        acp_success_response(
            request_id,
            AcpInitializeResult {
                protocol_version: 1,
                agent_capabilities: AcpAgentCapabilities {
                    load_session: true,
                    prompt_capabilities: AcpPromptCapabilities {
                        embedded_context: true,
                        ..AcpPromptCapabilities::default()
                    },
                    mcp_capabilities: AcpMcpCapabilities {
                        http: true,
                        sse: true,
                        ..AcpMcpCapabilities::default()
                    },
                    auth: Self::acp_auth_capabilities(&acp_auth_config),
                    session_capabilities: AcpSessionCapabilities {
                        list: Some(AcpSessionListCapabilities::default()),
                        delete: Some(AcpSessionDeleteCapabilities::default()),
                        additional_directories: Some(
                            AcpSessionAdditionalDirectoriesCapabilities::default(),
                        ),
                        resume: Some(AcpSessionResumeCapabilities::default()),
                        close: Some(AcpSessionCloseCapabilities::default()),
                        ..AcpSessionCapabilities::default()
                    },
                    ..AcpAgentCapabilities::default()
                },
                auth_methods: Self::acp_auth_methods(&acp_auth_config),
                agent_info: Some(
                    AcpImplementation::new(
                        self.metadata.server_name.clone(),
                        self.metadata.server_version.clone(),
                    )
                    .with_title("Devo"),
                ),
                meta: Some(meta),
            },
        )
    }

    pub(crate) async fn handle_acp_session_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: AcpListSessionsParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/list params: {error}"),
                );
            }
        };
        if let Some(cwd) = params.cwd.as_ref()
            && !cwd.is_absolute()
        {
            return acp_error_response(
                request_id,
                AcpErrorCode::InvalidParams,
                "session/list cwd must be an absolute path",
            );
        }
        let start = match params.cursor.as_deref().map(decode_session_list_cursor) {
            Some(Ok(start)) => start,
            Some(Err(error)) => {
                return acp_error_response(request_id, AcpErrorCode::InvalidParams, error);
            }
            None => 0,
        };
        let sessions = self.list_session_summaries().await;
        let mut filtered_sessions = Vec::new();
        for session in sessions.iter().filter(|session| {
            params
                .cwd
                .as_ref()
                .is_none_or(|cwd| session.cwd.as_path() == cwd.as_path())
        }) {
            if !session.cwd.is_absolute() {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InternalError,
                    format!(
                        "stored session {} cwd is not an absolute path",
                        session.session_id
                    ),
                );
            }
            filtered_sessions.push(acp_session_info_from_metadata(session));
        }
        if start > filtered_sessions.len() {
            return acp_error_response(
                request_id,
                AcpErrorCode::InvalidParams,
                "session/list cursor is out of range",
            );
        }
        let next_start = start.saturating_add(ACP_SESSION_LIST_PAGE_SIZE);
        let next_cursor =
            (next_start < filtered_sessions.len()).then(|| encode_session_list_cursor(next_start));
        let sessions = filtered_sessions
            .into_iter()
            .skip(start)
            .take(ACP_SESSION_LIST_PAGE_SIZE)
            .collect();
        acp_success_response(
            request_id,
            AcpListSessionsResult {
                sessions,
                next_cursor,
                meta: None,
            },
        )
    }

    pub(crate) async fn handle_acp_session_load(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: AcpLoadSessionParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/load params: {error}"),
                );
            }
        };
        if let Err(error) =
            validate_acp_session_roots("session/load", &params.cwd, &params.additional_directories)
        {
            return acp_error_response(request_id, AcpErrorCode::InvalidParams, error);
        }
        if let Err((code, error)) = self
            .validate_acp_existing_session_cwd("session/load", params.session_id, &params.cwd)
            .await
        {
            return acp_error_response(request_id, code, error);
        }
        let tool_registry = match self
            .acp_session_tool_registry("session/load", &params.mcp_servers, &params.cwd)
            .await
        {
            Ok(tool_registry) => tool_registry,
            Err(error) => {
                return acp_error_response(request_id, AcpErrorCode::InvalidParams, error);
            }
        };
        let legacy_response = self
            .restore_existing_session_with_tool_registry_update(
                connection_id,
                request_id.clone(),
                SessionResumeParams {
                    session_id: params.session_id,
                },
                RuntimeSessionToolRegistryUpdate::ReplaceIfCwdMatches {
                    cwd: params.cwd.clone(),
                    tool_registry,
                },
            )
            .await;
        let mut legacy: SuccessResponse<SessionResumeResult> =
            match serde_json::from_value(legacy_response.clone()) {
                Ok(legacy) => legacy,
                Err(_) => return legacy_error_to_acp(request_id, legacy_response),
            };
        let updated_summary = match self
            .apply_acp_session_additional_directories(
                params.session_id,
                params.additional_directories.clone(),
            )
            .await
        {
            Ok(summary) => summary,
            Err(error) => return acp_error_response(request_id, AcpErrorCode::ServerError, error),
        };
        legacy.result.session = updated_summary;
        self.subscribe_connection_to_session(connection_id, params.session_id, None)
            .await;
        self.send_acp_history_updates(
            connection_id,
            params.session_id,
            &legacy.result.history_items,
        )
        .await;
        self.send_acp_available_commands_update(connection_id, params.session_id)
            .await;
        let config_options = match self.acp_session_config_options(params.session_id).await {
            Ok(config_options) => config_options,
            Err(error) => return acp_error_response(request_id, AcpErrorCode::ServerError, error),
        };
        acp_success_response(
            request_id,
            AcpLoadSessionResult {
                modes: None,
                config_options: Some(config_options),
                meta: None,
            },
        )
    }

    pub(crate) async fn handle_acp_session_new(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: AcpNewSessionParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/new params: {error}"),
                );
            }
        };
        if let Err(error) =
            validate_acp_session_roots("session/new", &params.cwd, &params.additional_directories)
        {
            return acp_error_response(request_id, AcpErrorCode::InvalidParams, error);
        }
        let tool_registry = match self
            .acp_session_tool_registry("session/new", &params.mcp_servers, &params.cwd)
            .await
        {
            Ok(tool_registry) => tool_registry,
            Err(error) => {
                return acp_error_response(request_id, AcpErrorCode::InvalidParams, error);
            }
        };
        let legacy_response = self
            .start_session_with_registry(
                connection_id,
                request_id.clone(),
                SessionStartParams {
                    cwd: params.cwd,
                    additional_directories: params.additional_directories,
                    ephemeral: false,
                    title: None,
                    model: None,
                    model_binding_id: None,
                },
                tool_registry,
            )
            .await;
        let legacy: SuccessResponse<SessionStartResult> =
            match serde_json::from_value(legacy_response.clone()) {
                Ok(legacy) => legacy,
                Err(_) => return legacy_error_to_acp(request_id, legacy_response),
            };
        let mut meta = serde_json::Map::new();
        meta.insert(
            DEVO_SESSION_META.to_string(),
            serde_json::to_value(&legacy.result.session).expect("serialize session metadata"),
        );
        self.subscribe_connection_to_session(connection_id, legacy.result.session.session_id, None)
            .await;
        self.send_acp_available_commands_update(connection_id, legacy.result.session.session_id)
            .await;
        let config_options = match self
            .acp_session_config_options(legacy.result.session.session_id)
            .await
        {
            Ok(config_options) => config_options,
            Err(error) => return acp_error_response(request_id, AcpErrorCode::ServerError, error),
        };
        acp_success_response(
            request_id,
            AcpNewSessionResult {
                session_id: legacy.result.session.session_id,
                modes: None,
                config_options: Some(config_options),
                meta: Some(meta),
            },
        )
    }

    async fn validate_acp_existing_session_cwd(
        &self,
        method: &str,
        session_id: SessionId,
        cwd: &Path,
    ) -> Result<(), (AcpErrorCode, String)> {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return Err((
                AcpErrorCode::ServerError,
                "session does not exist".to_string(),
            ));
        };
        let stored_cwd = session_arc.lock().await.summary.cwd.clone();
        if stored_cwd != cwd {
            return Err((
                AcpErrorCode::InvalidParams,
                format!("{method} cwd does not match the stored session cwd"),
            ));
        }
        Ok(())
    }

    async fn acp_session_tool_registry(
        &self,
        method: &str,
        mcp_servers: &[AcpMcpServer],
        cwd: &Path,
    ) -> Result<Option<Arc<devo_core::tools::ToolRegistry>>, String> {
        if mcp_servers.is_empty() {
            return Ok(None);
        }

        let mcp_config = acp_mcp_config(method, mcp_servers)?;
        let user_config_dir = self
            .deps
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .user_config_dir()
            .to_path_buf();
        let config_store = AppConfigStore::load(user_config_dir, Some(cwd))
            .map_err(|error| format!("failed to load {method} workspace config: {error}"))?;
        let (tool_plan, oauth_store_mode) = {
            let config = config_store.effective_config();
            (
                ToolPlanConfig::from_app_config(config),
                config.mcp_oauth_credentials_store.unwrap_or_default(),
            )
        };
        let mcp_manager = Arc::new(RmcpMcpManager::new(mcp_config, oauth_store_mode));
        let registry =
            devo_core::tools::handlers::build_registry_from_plan_with_mcp(&tool_plan, mcp_manager)
                .await;

        Ok(Some(Arc::new(registry)))
    }

    pub(crate) async fn handle_acp_session_resume(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: AcpResumeSessionParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/resume params: {error}"),
                );
            }
        };
        if let Err(error) = validate_acp_session_roots(
            "session/resume",
            &params.cwd,
            &params.additional_directories,
        ) {
            return acp_error_response(request_id, AcpErrorCode::InvalidParams, error);
        }
        if let Err((code, error)) = self
            .validate_acp_existing_session_cwd("session/resume", params.session_id, &params.cwd)
            .await
        {
            return acp_error_response(request_id, code, error);
        }
        let tool_registry = match self
            .acp_session_tool_registry("session/resume", &params.mcp_servers, &params.cwd)
            .await
        {
            Ok(tool_registry) => tool_registry,
            Err(error) => {
                return acp_error_response(request_id, AcpErrorCode::InvalidParams, error);
            }
        };
        let legacy_response = self
            .restore_existing_session_with_tool_registry_update(
                connection_id,
                request_id.clone(),
                SessionResumeParams {
                    session_id: params.session_id,
                },
                RuntimeSessionToolRegistryUpdate::ReplaceIfCwdMatches {
                    cwd: params.cwd.clone(),
                    tool_registry,
                },
            )
            .await;
        let mut legacy: SuccessResponse<SessionResumeResult> =
            match serde_json::from_value(legacy_response.clone()) {
                Ok(legacy) => legacy,
                Err(_) => return legacy_error_to_acp(request_id, legacy_response),
            };
        let updated_summary = match self
            .apply_acp_session_additional_directories(
                params.session_id,
                params.additional_directories.clone(),
            )
            .await
        {
            Ok(summary) => summary,
            Err(error) => return acp_error_response(request_id, AcpErrorCode::ServerError, error),
        };
        legacy.result.session = updated_summary;
        let mut meta = serde_json::Map::new();
        meta.insert(
            DEVO_SESSION_META.to_string(),
            serde_json::to_value(&legacy.result.session).expect("serialize session metadata"),
        );
        meta.insert(
            DEVO_SESSION_RESUME_META.to_string(),
            serde_json::to_value(&legacy.result).expect("serialize session resume result"),
        );
        self.subscribe_connection_to_session(connection_id, params.session_id, None)
            .await;
        self.send_acp_available_commands_update(connection_id, params.session_id)
            .await;
        let config_options = match self.acp_session_config_options(params.session_id).await {
            Ok(config_options) => config_options,
            Err(error) => return acp_error_response(request_id, AcpErrorCode::ServerError, error),
        };
        acp_success_response(
            request_id,
            AcpResumeSessionResult {
                modes: None,
                config_options: Some(config_options),
                meta: Some(meta),
            },
        )
    }

    async fn apply_acp_session_additional_directories(
        &self,
        session_id: SessionId,
        additional_directories: Vec<PathBuf>,
    ) -> Result<SessionMetadata, String> {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return Err("session does not exist".to_string());
        };
        let (summary, core_session, profile) = {
            let mut session = session_arc.lock().await;
            let updated_at = Utc::now();
            session.summary.additional_directories = additional_directories.clone();
            session.summary.updated_at = updated_at;
            if let Some(record) = session.record.as_mut() {
                record.additional_directories = additional_directories.clone();
                record.updated_at = updated_at;
                if let Err(error) = self.rollout_store.append_session_meta(record) {
                    return Err(format!(
                        "failed to persist ACP session additional directories: {error}"
                    ));
                }
            }

            let profile = devo_safety::RuntimePermissionProfile::from_preset(
                session.config.permission_profile.preset,
                session.summary.cwd.clone(),
            )
            .with_additional_roots(additional_directories.clone());
            session.config.permission_profile = profile.clone();
            (
                session.summary.clone(),
                Arc::clone(&session.core_session),
                profile,
            )
        };
        core_session.lock().await.config.permission_profile = profile;

        if !summary.ephemeral
            && let Err(error) = self.deps.db.upsert_session(&summary)
        {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "failed to persist ACP session additional directories"
            );
        }
        Ok(summary)
    }

    pub(crate) async fn handle_acp_session_prompt(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> Option<serde_json::Value> {
        let params: AcpPromptParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return Some(acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/prompt params: {error}"),
                ));
            }
        };
        if self.session_has_active_turn(params.session_id).await {
            return Some(acp_error_response(
                request_id,
                AcpErrorCode::ServerError,
                "session already has an active prompt turn",
            ));
        }
        self.subscribe_connection_to_session(connection_id, params.session_id, None)
            .await;
        match self
            .handle_acp_slash_command_prompt(
                connection_id,
                request_id.clone(),
                params.session_id,
                &params.prompt,
            )
            .await
        {
            super::acp_slash_commands::AcpSlashCommandPromptResult::NotCommand => {}
            super::acp_slash_commands::AcpSlashCommandPromptResult::Response(response) => {
                return Some(response);
            }
            super::acp_slash_commands::AcpSlashCommandPromptResult::Pending => return None,
        }
        let session_id = params.session_id;
        let input = match input_items_from_acp_prompt(params.prompt) {
            Ok(input) => input,
            Err(error) => {
                return Some(acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    error,
                ));
            }
        };
        let legacy_response = self
            .handle_turn_start_with_queue_policy(
                Some(connection_id),
                request_id.clone(),
                TurnStartParams {
                    session_id,
                    input,
                    model: None,
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    sandbox: None,
                    approval_policy: None,
                    cwd: None,
                    collaboration_mode: CollaborationMode::Build,
                    execution_mode: TurnExecutionMode::Regular,
                },
                TurnStartQueuePolicy::RejectActive,
            )
            .await;
        let legacy: SuccessResponse<TurnStartResult> =
            match serde_json::from_value(legacy_response.clone()) {
                Ok(legacy) => legacy,
                Err(_) => return Some(legacy_error_to_acp(request_id, legacy_response)),
            };
        let Some(turn_id) = legacy.result.turn_id() else {
            return Some(acp_error_response(
                request_id,
                AcpErrorCode::ServerError,
                "session/prompt cannot queue behind an active turn",
            ));
        };
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let stop_reason = runtime
                .wait_for_acp_prompt_stop_reason(session_id, turn_id)
                .await;
            runtime
                .send_raw_to_connection(
                    connection_id,
                    serde_json::to_value(AcpSuccessResponse::new(
                        request_id,
                        AcpPromptResult {
                            stop_reason,
                            meta: None,
                        },
                    ))
                    .expect("serialize ACP prompt response"),
                )
                .await;
        });
        None
    }

    pub(crate) async fn handle_acp_session_cancel(self: &Arc<Self>, params: serde_json::Value) {
        let params: AcpCancelParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                tracing::warn!(%error, "invalid session/cancel params");
                return;
            }
        };
        let Some(turn_id) = self.active_turn_id(params.session_id).await else {
            tracing::debug!(session_id = %params.session_id, "session/cancel had no active turn");
            return;
        };
        let _ = self
            .handle_turn_interrupt(
                serde_json::Value::Null,
                serde_json::to_value(TurnInterruptParams {
                    session_id: params.session_id,
                    turn_id,
                    reason: Some("cancelled by ACP client".to_string()),
                })
                .expect("serialize turn interrupt params"),
            )
            .await;
    }

    pub(crate) async fn handle_acp_session_close(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: AcpCloseSessionParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/close params: {error}"),
                );
            }
        };
        if !self.sessions.lock().await.contains_key(&params.session_id) {
            return acp_error_response(
                request_id,
                AcpErrorCode::ServerError,
                "session does not exist",
            );
        }
        self.handle_acp_session_cancel(
            serde_json::to_value(AcpCancelParams {
                session_id: params.session_id,
                meta: None,
            })
            .expect("serialize ACP cancel params"),
        )
        .await;
        self.sessions.lock().await.remove(&params.session_id);
        acp_success_response(request_id, AcpCloseSessionResult::default())
    }

    pub(crate) async fn handle_acp_session_delete(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: AcpDeleteSessionParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/delete params: {error}"),
                );
            }
        };
        if self.sessions.lock().await.contains_key(&params.session_id) {
            self.handle_acp_session_cancel(
                serde_json::to_value(AcpCancelParams {
                    session_id: params.session_id,
                    meta: None,
                })
                .expect("serialize ACP cancel params"),
            )
            .await;
        }
        let removed = self.sessions.lock().await.remove(&params.session_id);
        let persisted = match self.deps.db.get_session(&params.session_id) {
            Ok(session) => session,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InternalError,
                    format!("failed to inspect session before delete: {error}"),
                );
            }
        };
        let deleted_rollout = match self
            .rollout_store
            .delete_session_rollouts(&params.session_id)
        {
            Ok(deleted) => deleted,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InternalError,
                    format!("failed to delete session rollout: {error}"),
                );
            }
        };
        if removed.is_none() && persisted.is_none() && !deleted_rollout {
            return acp_success_response(request_id, AcpDeleteSessionResult::default());
        }
        if persisted.is_some()
            && let Err(error) = self.deps.db.delete_session(&params.session_id)
        {
            return acp_error_response(
                request_id,
                AcpErrorCode::InternalError,
                format!("failed to delete session: {error}"),
            );
        }
        acp_success_response(request_id, AcpDeleteSessionResult::default())
    }

    pub(crate) async fn handle_acp_session_set_mode(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let _params: AcpSetModeParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/set_mode params: {error}"),
                );
            }
        };
        acp_error_response(
            request_id,
            AcpErrorCode::MethodNotFound,
            "session/set_mode is not supported",
        )
    }

    pub(crate) async fn handle_acp_session_set_config_option(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: AcpSetConfigOptionParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/set_config_option params: {error}"),
                );
            }
        };
        let config_options = match self.set_acp_session_config_option(params).await {
            Ok(config_options) => config_options,
            Err((code, message)) => return acp_error_response(request_id, code, message),
        };
        acp_success_response(
            request_id,
            AcpSetConfigOptionResult {
                config_options,
                meta: None,
            },
        )
    }

    async fn send_acp_history_updates(
        &self,
        connection_id: u64,
        session_id: SessionId,
        history_items: &[SessionHistoryItem],
    ) {
        for (index, item) in history_items.iter().enumerate() {
            let Some(update) = acp_update_from_history_item(index, item) else {
                continue;
            };
            let notification = AcpClientNotification::new(
                ACP_SESSION_UPDATE_METHOD,
                AcpSessionNotification {
                    session_id,
                    update,
                    meta: None,
                },
            );
            self.send_raw_to_connection(
                connection_id,
                serde_json::to_value(notification).expect("serialize ACP history notification"),
            )
            .await;
        }
    }

    async fn session_has_active_turn(&self, session_id: SessionId) -> bool {
        self.active_turn_id(session_id).await.is_some()
    }

    async fn active_turn_id(&self, session_id: SessionId) -> Option<TurnId> {
        let session = self.sessions.lock().await.get(&session_id).cloned()?;
        session
            .lock()
            .await
            .active_turn
            .as_ref()
            .map(|turn| turn.turn_id)
    }

    pub(super) async fn wait_for_acp_prompt_stop_reason(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> AcpStopReason {
        let receiver = self.subscribe_terminal_turn_status(turn_id).await;
        if let Some(status) = self.recent_terminal_turn_status(turn_id).await {
            self.record_terminal_turn_status(turn_id, status).await;
        } else if !self.sessions.lock().await.contains_key(&session_id) {
            return AcpStopReason::Cancelled;
        }
        let status = match receiver.await {
            Ok(status) => status,
            Err(_) => return AcpStopReason::Refusal,
        };
        acp_stop_reason_from_terminal_turn(status)
    }
}

fn acp_stop_reason_from_terminal_turn(snapshot: TerminalTurnSnapshot) -> AcpStopReason {
    match snapshot.status {
        TurnStatus::Completed => match snapshot.stop_reason {
            Some(devo_core::StopReason::MaxTokens) => AcpStopReason::MaxTokens,
            Some(
                devo_core::StopReason::EndTurn
                | devo_core::StopReason::ToolUse
                | devo_core::StopReason::StopSequence,
            )
            | None => AcpStopReason::EndTurn,
        },
        TurnStatus::Interrupted => AcpStopReason::Cancelled,
        TurnStatus::Failed => match snapshot.failure_reason {
            Some(devo_protocol::TurnFailureReason::MaxTurnRequests) => {
                AcpStopReason::MaxTurnRequests
            }
            None => AcpStopReason::Refusal,
        },
        TurnStatus::Pending | TurnStatus::Running | TurnStatus::WaitingApproval => {
            AcpStopReason::Refusal
        }
    }
}

fn validate_acp_session_roots(
    method: &str,
    cwd: &Path,
    additional_directories: &[PathBuf],
) -> Result<(), String> {
    if !cwd.is_absolute() {
        return Err(format!("{method} cwd must be an absolute path"));
    }
    if let Some((index, _)) = additional_directories
        .iter()
        .enumerate()
        .find(|(_, directory)| !directory.is_absolute())
    {
        return Err(format!(
            "{method} additionalDirectories[{index}] must be an absolute path"
        ));
    }
    Ok(())
}

fn encode_session_list_cursor(start: usize) -> String {
    URL_SAFE_NO_PAD.encode(format!("{ACP_SESSION_LIST_CURSOR_PREFIX}{start}"))
}

fn decode_session_list_cursor(cursor: &str) -> Result<usize, String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| "session/list cursor is invalid".to_string())?;
    let value =
        std::str::from_utf8(&bytes).map_err(|_| "session/list cursor is invalid".to_string())?;
    let start = value
        .strip_prefix(ACP_SESSION_LIST_CURSOR_PREFIX)
        .ok_or_else(|| "session/list cursor is invalid".to_string())?;
    start
        .parse::<usize>()
        .map_err(|_| "session/list cursor is invalid".to_string())
}

fn acp_mcp_config(method: &str, mcp_servers: &[AcpMcpServer]) -> Result<McpConfig, String> {
    let mut ids = HashSet::new();
    let mut records = Vec::with_capacity(mcp_servers.len());

    for mcp_server in mcp_servers {
        let (id, transport) = match mcp_server {
            AcpMcpServer::Stdio(server) => (
                acp_mcp_server_id(method, server.name.as_str())?,
                McpTransportConfig::Stdio {
                    command: acp_stdio_command(method, server)?,
                    cwd: None,
                    env: acp_stdio_env(method, server)?,
                    env_vars: Vec::new(),
                },
            ),
            AcpMcpServer::Http(server) => {
                if server.url.trim().is_empty() {
                    return Err(format!(
                        "{method} mcpServers http entry '{}' must include a non-empty url",
                        server.name
                    ));
                }
                (
                    acp_mcp_server_id(method, server.name.as_str())?,
                    McpTransportConfig::StreamableHttp {
                        url: server.url.clone(),
                        auth: None,
                        http_headers: acp_http_headers(
                            method,
                            "http",
                            server.name.as_str(),
                            &server.headers,
                        )?,
                        env_http_headers: BTreeMap::new(),
                    },
                )
            }
            AcpMcpServer::Sse(server) => {
                if server.url.trim().is_empty() {
                    return Err(format!(
                        "{method} mcpServers sse entry '{}' must include a non-empty url",
                        server.name
                    ));
                }
                (
                    acp_mcp_server_id(method, server.name.as_str())?,
                    McpTransportConfig::Sse {
                        url: server.url.clone(),
                        auth: None,
                        http_headers: acp_http_headers(
                            method,
                            "sse",
                            server.name.as_str(),
                            &server.headers,
                        )?,
                        env_http_headers: BTreeMap::new(),
                    },
                )
            }
            AcpMcpServer::Unsupported(server) => {
                return Err(format!(
                    "{method} mcpServers transport '{}' is not supported",
                    server.transport_type
                ));
            }
        };
        if !ids.insert(id.clone()) {
            return Err(format!(
                "{method} mcpServers contains duplicate server name '{id}'"
            ));
        }

        records.push(McpServerRecord {
            id: McpServerId(id.clone()),
            display_name: id,
            transport,
            startup_policy: McpStartupPolicy::Eager,
            enabled: true,
            trust_policy: McpTrustPolicy::default(),
            allowed_capabilities: Vec::new(),
            roots_policy: McpRootsPolicy::default(),
            output_limits: McpOutputLimits::default(),
            auth_ref: None,
        });
    }

    Ok(McpConfig {
        servers: records,
        auto_start: true,
        refresh_on_config_reload: false,
    })
}

fn acp_mcp_server_id(method: &str, name: &str) -> Result<String, String> {
    let id = name.trim().to_string();
    if id.is_empty() {
        return Err(format!(
            "{method} mcpServers entries must include a non-empty name"
        ));
    }
    Ok(id)
}

fn acp_stdio_command(method: &str, server: &AcpMcpServerStdio) -> Result<Vec<String>, String> {
    if server.command.as_os_str().is_empty() {
        return Err(format!(
            "{method} mcpServers entry '{}' must include a non-empty command",
            server.name
        ));
    }

    let mut command = Vec::with_capacity(server.args.len() + 1);
    command.push(server.command.to_string_lossy().into_owned());
    command.extend(server.args.iter().cloned());
    Ok(command)
}

fn acp_stdio_env(
    method: &str,
    server: &AcpMcpServerStdio,
) -> Result<BTreeMap<String, String>, String> {
    let mut env = BTreeMap::new();
    for variable in &server.env {
        let name = variable.name.trim();
        if name.is_empty() {
            return Err(format!(
                "{method} mcpServers entry '{}' contains an env variable with an empty name",
                server.name
            ));
        }
        if env
            .insert(name.to_string(), variable.value.clone())
            .is_some()
        {
            return Err(format!(
                "{method} mcpServers entry '{}' contains duplicate env variable '{name}'",
                server.name
            ));
        }
    }
    Ok(env)
}

fn acp_http_headers(
    method: &str,
    transport: &str,
    server_name: &str,
    headers: &[crate::AcpHttpHeader],
) -> Result<BTreeMap<String, String>, String> {
    let mut result = BTreeMap::new();
    for header in headers {
        let name = header.name.trim();
        if name.is_empty() {
            return Err(format!(
                "{method} mcpServers {transport} entry '{server_name}' contains a header with an empty name"
            ));
        }
        if result
            .insert(name.to_string(), header.value.clone())
            .is_some()
        {
            return Err(format!(
                "{method} mcpServers {transport} entry '{server_name}' contains duplicate header '{name}'"
            ));
        }
    }
    Ok(result)
}

fn acp_update_from_history_item(
    index: usize,
    item: &SessionHistoryItem,
) -> Option<AcpSessionUpdate> {
    if let Some(SessionHistoryMetadata::PlanUpdate { steps, .. }) = &item.metadata {
        return Some(AcpSessionUpdate::Plan {
            entries: steps
                .iter()
                .map(|step| AcpPlanEntry {
                    content: step.text.clone(),
                    priority: AcpPlanEntryPriority::Medium,
                    status: match step.status {
                        SessionPlanStepStatus::Completed => AcpPlanEntryStatus::Completed,
                        SessionPlanStepStatus::InProgress => AcpPlanEntryStatus::InProgress,
                        SessionPlanStepStatus::Pending | SessionPlanStepStatus::Cancelled => {
                            AcpPlanEntryStatus::Pending
                        }
                    },
                })
                .collect(),
            meta: None,
        });
    }
    let content = AcpContentBlock::text(item.body.clone());
    let message_id = Some(format!("history-{index}"));
    match item.kind {
        SessionHistoryItemKind::User => Some(AcpSessionUpdate::UserMessageChunk {
            content,
            message_id,
            meta: None,
        }),
        SessionHistoryItemKind::Assistant => Some(AcpSessionUpdate::AgentMessageChunk {
            content,
            message_id,
            meta: None,
        }),
        SessionHistoryItemKind::Reasoning | SessionHistoryItemKind::TurnSummary => {
            Some(AcpSessionUpdate::AgentThoughtChunk {
                content,
                message_id,
                meta: None,
            })
        }
        SessionHistoryItemKind::ToolCall => {
            let tool_call_id = history_tool_call_id(index, item);
            Some(AcpSessionUpdate::ToolCall {
                tool_call_id,
                title: item.title.clone(),
                kind: AcpToolKind::Other,
                status: AcpToolCallStatus::Completed,
                raw_input: item.tool_io.as_ref().map(|tool_io| tool_io.input.clone()),
                raw_output: item
                    .tool_io
                    .as_ref()
                    .and_then(|tool_io| tool_io.output.clone()),
                content: Vec::new(),
                locations: Vec::new(),
                meta: None,
            })
        }
        SessionHistoryItemKind::ToolResult
        | SessionHistoryItemKind::CommandExecution
        | SessionHistoryItemKind::Error => {
            let tool_call_id = history_tool_call_id(index, item);
            let text = if item.body.is_empty() {
                item.title.clone()
            } else {
                item.body.clone()
            };
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id,
                title: Some(item.title.clone()),
                kind: None,
                status: Some(if item.kind == SessionHistoryItemKind::Error {
                    AcpToolCallStatus::Failed
                } else {
                    AcpToolCallStatus::Completed
                }),
                raw_input: item.tool_io.as_ref().map(|tool_io| tool_io.input.clone()),
                raw_output: item
                    .tool_io
                    .as_ref()
                    .and_then(|tool_io| tool_io.output.clone()),
                content: vec![AcpToolCallContent::Content {
                    content: AcpContentBlock::text(text),
                }],
                locations: Vec::new(),
                meta: None,
            })
        }
    }
}

fn history_tool_call_id(index: usize, item: &SessionHistoryItem) -> String {
    item.tool_call_id
        .clone()
        .unwrap_or_else(|| format!("history-{index}"))
}

fn acp_success_response<T: serde::Serialize>(
    request_id: serde_json::Value,
    result: T,
) -> serde_json::Value {
    serde_json::to_value(AcpSuccessResponse::new(request_id, result))
        .expect("serialize ACP success response")
}

fn acp_error_response(
    request_id: serde_json::Value,
    code: AcpErrorCode,
    message: impl Into<String>,
) -> serde_json::Value {
    serde_json::to_value(AcpErrorResponse::new(
        request_id,
        code,
        message,
        serde_json::Value::Null,
    ))
    .expect("serialize ACP error response")
}

fn legacy_error_to_acp(
    request_id: serde_json::Value,
    legacy_response: serde_json::Value,
) -> serde_json::Value {
    if let Ok(error) = serde_json::from_value::<ErrorResponse>(legacy_response) {
        acp_error_response(request_id, AcpErrorCode::ServerError, error.error.message)
    } else {
        acp_error_response(
            request_id,
            AcpErrorCode::InternalError,
            "failed to decode internal runtime response",
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use devo_protocol::AcpEnvVariable;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn acp_stop_reason_maps_terminal_turn_metadata() {
        let mut turn = TurnMetadata {
            turn_id: TurnId::new(),
            session_id: SessionId::new(),
            sequence: 1,
            status: TurnStatus::Completed,
            kind: devo_protocol::TurnKind::Regular,
            model: "test-model".to_string(),
            model_binding_id: None,
            reasoning_effort_selection: None,
            reasoning_effort: None,
            request_model: "test-model".to_string(),
            request_thinking: None,
            started_at: chrono::Utc::now(),
            completed_at: Some(chrono::Utc::now()),
            usage: None,
            stop_reason: Some(devo_core::StopReason::MaxTokens),
            failure_reason: None,
        };
        assert_eq!(
            acp_stop_reason_from_terminal_turn(TerminalTurnSnapshot::from_turn(&turn)),
            AcpStopReason::MaxTokens
        );

        turn.status = TurnStatus::Failed;
        turn.stop_reason = None;
        turn.failure_reason = Some(devo_protocol::TurnFailureReason::MaxTurnRequests);
        assert_eq!(
            acp_stop_reason_from_terminal_turn(TerminalTurnSnapshot::from_turn(&turn)),
            AcpStopReason::MaxTurnRequests
        );

        turn.status = TurnStatus::Interrupted;
        assert_eq!(
            acp_stop_reason_from_terminal_turn(TerminalTurnSnapshot::from_turn(&turn)),
            AcpStopReason::Cancelled
        );
        turn.status = TurnStatus::Failed;
        turn.failure_reason = None;
        assert_eq!(
            acp_stop_reason_from_terminal_turn(TerminalTurnSnapshot::from_turn(&turn)),
            AcpStopReason::Refusal
        );
        turn.status = TurnStatus::Completed;
        assert_eq!(
            acp_stop_reason_from_terminal_turn(TerminalTurnSnapshot::from_turn(&turn)),
            AcpStopReason::EndTurn
        );
    }

    #[test]
    fn acp_mcp_config_converts_stdio_servers() {
        #[cfg(windows)]
        let command_path = PathBuf::from(r"C:\mcp\filesystem.exe");
        #[cfg(windows)]
        let command = r"C:\mcp\filesystem.exe".to_string();
        #[cfg(unix)]
        let command_path = PathBuf::from("/mcp/filesystem");
        #[cfg(unix)]
        let command = "/mcp/filesystem".to_string();

        let config = acp_mcp_config(
            "session/new",
            &[AcpMcpServer::Stdio(AcpMcpServerStdio {
                name: "filesystem".to_string(),
                command: command_path,
                args: vec!["--stdio".to_string()],
                env: vec![AcpEnvVariable {
                    name: "API_KEY".to_string(),
                    value: "secret123".to_string(),
                    meta: None,
                }],
                meta: None,
            })],
        )
        .expect("stdio MCP server should convert");

        assert_eq!(
            config,
            McpConfig {
                servers: vec![McpServerRecord {
                    id: McpServerId("filesystem".to_string()),
                    display_name: "filesystem".to_string(),
                    transport: McpTransportConfig::Stdio {
                        command: vec![command, "--stdio".to_string()],
                        cwd: None,
                        env: BTreeMap::from([("API_KEY".to_string(), "secret123".to_string())]),
                        env_vars: Vec::new(),
                    },
                    startup_policy: McpStartupPolicy::Eager,
                    enabled: true,
                    trust_policy: McpTrustPolicy::default(),
                    allowed_capabilities: Vec::new(),
                    roots_policy: McpRootsPolicy::default(),
                    output_limits: McpOutputLimits::default(),
                    auth_ref: None,
                }],
                auto_start: true,
                refresh_on_config_reload: false,
            }
        );
    }

    #[test]
    fn acp_mcp_config_converts_http_and_sse_servers() {
        let config = acp_mcp_config(
            "session/new",
            &[
                AcpMcpServer::Http(crate::AcpMcpServerHttp {
                    transport_type: crate::AcpMcpServerHttpType::Http,
                    name: "api-server".to_string(),
                    url: "https://api.example.com/mcp".to_string(),
                    headers: vec![crate::AcpHttpHeader {
                        name: "Authorization".to_string(),
                        value: "Bearer token123".to_string(),
                        meta: None,
                    }],
                    meta: None,
                }),
                AcpMcpServer::Sse(crate::AcpMcpServerSse {
                    transport_type: crate::AcpMcpServerSseType::Sse,
                    name: "event-stream".to_string(),
                    url: "https://events.example.com/mcp".to_string(),
                    headers: vec![crate::AcpHttpHeader {
                        name: "X-API-Key".to_string(),
                        value: "apikey456".to_string(),
                        meta: None,
                    }],
                    meta: None,
                }),
            ],
        )
        .expect("HTTP and SSE MCP servers should convert");

        assert_eq!(
            config,
            McpConfig {
                servers: vec![
                    McpServerRecord {
                        id: McpServerId("api-server".to_string()),
                        display_name: "api-server".to_string(),
                        transport: McpTransportConfig::StreamableHttp {
                            url: "https://api.example.com/mcp".to_string(),
                            auth: None,
                            http_headers: BTreeMap::from([(
                                "Authorization".to_string(),
                                "Bearer token123".to_string()
                            )]),
                            env_http_headers: BTreeMap::new(),
                        },
                        startup_policy: McpStartupPolicy::Eager,
                        enabled: true,
                        trust_policy: McpTrustPolicy::default(),
                        allowed_capabilities: Vec::new(),
                        roots_policy: McpRootsPolicy::default(),
                        output_limits: McpOutputLimits::default(),
                        auth_ref: None,
                    },
                    McpServerRecord {
                        id: McpServerId("event-stream".to_string()),
                        display_name: "event-stream".to_string(),
                        transport: McpTransportConfig::Sse {
                            url: "https://events.example.com/mcp".to_string(),
                            auth: None,
                            http_headers: BTreeMap::from([(
                                "X-API-Key".to_string(),
                                "apikey456".to_string()
                            )]),
                            env_http_headers: BTreeMap::new(),
                        },
                        startup_policy: McpStartupPolicy::Eager,
                        enabled: true,
                        trust_policy: McpTrustPolicy::default(),
                        allowed_capabilities: Vec::new(),
                        roots_policy: McpRootsPolicy::default(),
                        output_limits: McpOutputLimits::default(),
                        auth_ref: None,
                    },
                ],
                auto_start: true,
                refresh_on_config_reload: false,
            }
        );
    }

    #[test]
    fn acp_mcp_config_rejects_duplicate_server_names() {
        #[cfg(windows)]
        let command_path = PathBuf::from(r"C:\mcp\filesystem.exe");
        #[cfg(unix)]
        let command_path = PathBuf::from("/mcp/filesystem");

        let server = AcpMcpServer::Stdio(AcpMcpServerStdio {
            name: "filesystem".to_string(),
            command: command_path,
            args: Vec::new(),
            env: Vec::new(),
            meta: None,
        });

        assert_eq!(
            acp_mcp_config("session/resume", &[server.clone(), server])
                .expect_err("duplicate names should fail"),
            "session/resume mcpServers contains duplicate server name 'filesystem'"
        );
    }
}
