use super::*;

impl ServerRuntime {
    pub(super) async fn clear_active_turn_runtime_handles(&self, session_id: SessionId) {
        self.active_tasks.lock().await.remove(&session_id);
        self.active_turn_cancellations
            .lock()
            .await
            .remove(&session_id);
    }

    pub(super) async fn clear_active_turn_reservation(
        &self,
        session_arc: &Arc<Mutex<RuntimeSession>>,
        turn_id: TurnId,
    ) {
        let mut session = session_arc.lock().await;
        if session
            .active_turn
            .as_ref()
            .is_some_and(|active| active.turn_id == turn_id)
        {
            session.active_turn = None;
            session.summary.status = SessionRuntimeStatus::Idle;
            session.summary.updated_at = Utc::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use anyhow::Result;
    use async_trait::async_trait;
    use devo_core::AppConfigStore;
    use devo_core::BundledSkillsConfig;
    use devo_core::FileSystemSkillCatalog;
    use devo_core::PresetModelCatalog;
    use devo_core::ProviderVendorCatalog;
    use devo_core::SkillsConfig;
    use devo_core::tools::ToolRegistry;
    use devo_protocol::ErrorResponse;
    use devo_protocol::ModelRequest;
    use devo_protocol::ModelResponse;
    use devo_protocol::StreamEvent;
    use devo_protocol::SuccessResponse;
    use devo_provider::ModelProviderSDK;
    use devo_provider::SingleProviderRouter;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    struct NoopProvider;

    #[async_trait]
    impl ModelProviderSDK for NoopProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            anyhow::bail!("noop provider does not support completion")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>>
        {
            anyhow::bail!("noop provider does not support streaming")
        }

        fn name(&self) -> &str {
            "noop-provider"
        }
    }

    fn build_runtime(data_root: &std::path::Path) -> Arc<ServerRuntime> {
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(NoopProvider);
        let db = Arc::new(
            crate::db::Database::open(data_root.join("turn_reservation.db"))
                .expect("open test database"),
        );
        ServerRuntime::new(
            data_root.to_path_buf(),
            ServerRuntimeDependencies::new(
                Arc::clone(&provider),
                Arc::new(SingleProviderRouter::new(provider)),
                Arc::new(ToolRegistry::new()),
                "test-model".to_string(),
                Arc::new(PresetModelCatalog::default()),
                Arc::new(ProviderVendorCatalog::default()),
                None,
                Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                    bundled: Some(BundledSkillsConfig { enabled: false }),
                    ..SkillsConfig::default()
                })),
                devo_core::AgentsMdConfig::default(),
                db,
                Arc::new(std::sync::Mutex::new(
                    AppConfigStore::load(data_root.to_path_buf(), None)
                        .expect("load app config store"),
                )),
            ),
        )
    }

    async fn start_session(runtime: &Arc<ServerRuntime>, cwd: std::path::PathBuf) -> SessionId {
        let value = runtime
            .handle_session_start(
                /*connection_id*/ 1,
                serde_json::json!(1),
                serde_json::to_value(SessionStartParams {
                    cwd,
                    ephemeral: false,
                    title: None,
                    model: None,
                    model_binding_id: None,
                })
                .expect("session start params"),
            )
            .await;
        let response: SuccessResponse<SessionStartResult> =
            serde_json::from_value(value).expect("session start response");
        response.result.session.session_id
    }

    #[tokio::test]
    async fn shell_command_start_failure_clears_active_turn() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = start_session(&runtime, data_root.path().to_path_buf()).await;
        let bad_rollout_path = data_root.path().join("rollout-dir");
        std::fs::create_dir(&bad_rollout_path)?;
        let session_arc = runtime
            .sessions
            .lock()
            .await
            .get(&session_id)
            .cloned()
            .expect("session");
        {
            let mut session = session_arc.lock().await;
            session
                .record
                .as_mut()
                .expect("durable record")
                .rollout_path = bad_rollout_path;
        }

        let value = runtime
            .handle_turn_shell_command(
                serde_json::json!(2),
                serde_json::to_value(ShellCommandParams {
                    session_id,
                    command: "pwd".to_string(),
                    cwd: None,
                })
                .expect("shell command params"),
            )
            .await;
        let response: ErrorResponse = serde_json::from_value(value).expect("error response");
        let session = session_arc.lock().await;

        assert_eq!(response.error.code, ProtocolErrorCode::InternalError);
        assert_eq!(session.active_turn, None);
        assert_eq!(session.summary.status, SessionRuntimeStatus::Idle);
        assert_eq!(session.latest_turn, None);

        Ok(())
    }
}
