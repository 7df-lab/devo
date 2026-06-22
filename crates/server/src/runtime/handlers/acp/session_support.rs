use super::*;

const ACP_SESSION_LIST_CURSOR_PREFIX: &str = "devo-session-list-v1:";

impl ServerRuntime {
    pub(super) async fn validate_acp_existing_session_cwd(
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

    pub(super) async fn acp_session_tool_registry(
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
    pub(super) async fn apply_acp_session_additional_directories(
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

    pub(super) async fn send_acp_history_updates(
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
}
pub(super) fn validate_acp_session_roots(
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

pub(super) fn encode_session_list_cursor(start: usize) -> String {
    URL_SAFE_NO_PAD.encode(format!("{ACP_SESSION_LIST_CURSOR_PREFIX}{start}"))
}

pub(super) fn decode_session_list_cursor(cursor: &str) -> Result<usize, String> {
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
