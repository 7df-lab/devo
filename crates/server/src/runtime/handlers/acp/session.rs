use super::*;

const ACP_SESSION_LIST_PAGE_SIZE: usize = 50;

#[derive(serde::Deserialize)]
struct LegacySessionIdentity {
    id: devo_protocol::native::ids::SessionId,
}

#[derive(serde::Deserialize)]
struct LegacySessionStartProjection {
    session: LegacySessionIdentity,
}

#[derive(serde::Deserialize)]
struct LegacySessionResumeProjection {
    history_items: Vec<SessionHistoryEntry>,
}

/// Accept the pre-ACP session creation shape used by existing Devo clients.
///
/// ACP v1 describes `mcpServers` as part of the session lifecycle request
/// contract, while the older Devo API omitted it when no MCP servers were
/// configured. The adapter normalizes that omission to the ACP empty-list
/// value before deserialization; supplied MCP entries still go through the
/// normal ACP validation path.
fn normalize_missing_mcp_servers(mut params: serde_json::Value) -> serde_json::Value {
    if let Some(object) = params.as_object_mut() {
        object
            .entry("mcpServers")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    }
    params
}

impl ServerRuntime {
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
        let sessions = self.list_native_sessions().await;
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
                    format!("stored session {} cwd is not an absolute path", session.id),
                );
            }
            filtered_sessions.push(acp_session_info_from_native_session(session));
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
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = normalize_missing_mcp_servers(params);
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
        let legacy: SuccessResponse<LegacySessionResumeProjection> =
            match serde_json::from_value(legacy_response.clone()) {
                Ok(legacy) => legacy,
                Err(_) => return legacy_error_to_acp(request_id, legacy_response),
            };
        if let Err(error) = self
            .apply_acp_session_additional_directories(
                params.session_id,
                params.additional_directories.clone(),
            )
            .await
        {
            return acp_error_response(request_id, AcpErrorCode::ServerError, error);
        }
        self.subscribe_connection_to_session(connection_id, params.session_id, None)
            .await;
        self.send_acp_history_updates(
            connection_id,
            params.session_id,
            &legacy.result.history_items,
        )
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
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = normalize_missing_mcp_servers(params);
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
                    ephemeral: params.ephemeral,
                    title: params.title,
                    model: params.model,
                    model_binding_id: params.model_binding_id,
                },
                tool_registry,
            )
            .await;
        let legacy: SuccessResponse<LegacySessionStartProjection> =
            match serde_json::from_value(legacy_response.clone()) {
                Ok(legacy) => legacy,
                Err(_) => return legacy_error_to_acp(request_id, legacy_response),
            };
        let session_id = SessionId::from(legacy.result.session.id.as_str());
        let Some(handle) = self.session(session_id).await else {
            return acp_error_response(
                request_id,
                AcpErrorCode::ServerError,
                "new session actor unavailable",
            );
        };
        let Some(session) = handle.native_session().await else {
            return acp_error_response(
                request_id,
                AcpErrorCode::ServerError,
                "new session actor unavailable",
            );
        };
        let mut meta = serde_json::Map::new();
        meta.insert(
            DEVO_SESSION_META.to_string(),
            serde_json::to_value(&session).expect("serialize native session"),
        );
        self.subscribe_connection_to_session(connection_id, session_id, None)
            .await;
        let config_options = match self.acp_session_config_options(session_id).await {
            Ok(config_options) => config_options,
            Err(error) => return acp_error_response(request_id, AcpErrorCode::ServerError, error),
        };
        acp_success_response(
            request_id,
            AcpNewSessionResult {
                session_id,
                modes: None,
                config_options: Some(config_options),
                meta: Some(meta),
            },
        )
    }

    pub(crate) async fn handle_acp_session_resume(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = normalize_missing_mcp_servers(params);
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
        let _legacy: SuccessResponse<LegacySessionResumeProjection> =
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
        let mut meta = serde_json::Map::new();
        meta.insert(
            DEVO_SESSION_META.to_string(),
            serde_json::to_value(&updated_summary.native).expect("serialize native session"),
        );
        self.subscribe_connection_to_session(connection_id, params.session_id, None)
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
                AcpErrorCode::ResourceNotFound,
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
        self.remove_session_actor(params.session_id).await;
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
        let deleted_session_ids = match self.delete_session_tree(params.session_id).await {
            Ok(deleted_session_ids) => deleted_session_ids,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InternalError,
                    format!("failed to delete session: {error}"),
                );
            }
        };
        if !deleted_session_ids.is_empty() {
            self.broadcast_notification(
                devo_protocol::native::event::ServerNotification::SessionDeleted {
                    session_id: params.session_id,
                    deleted_session_ids: deleted_session_ids.to_vec(),
                },
            )
            .await;
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
}
