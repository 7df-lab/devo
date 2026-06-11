use super::*;

impl ServerRuntime {
    pub(super) async fn run_subagent_start_hook(&self, child_session_id: SessionId) {
        let Some(context) = self.hook_context_for_session(child_session_id).await else {
            return;
        };
        context
            .runner
            .run(devo_core::HookInput::new(
                &context.base,
                devo_core::HookEvent::SubagentStart,
                subagent_hook_extra(&context.base, /*stop_hook_active*/ None),
            ))
            .await;
    }

    pub(super) async fn run_subagent_stop_hook(&self, child_session_id: SessionId) {
        let Some(context) = self.hook_context_for_session(child_session_id).await else {
            return;
        };
        context
            .runner
            .run(devo_core::HookInput::new(
                &context.base,
                devo_core::HookEvent::SubagentStop,
                subagent_hook_extra(&context.base, Some(false)),
            ))
            .await;
    }

    pub(super) async fn fail_child_agent_startup(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
        error_message: String,
    ) {
        self.set_agent_status(parent_session_id, child_session_id, SubagentStatus::Failed)
            .await;
        if let Some(session_arc) = self.sessions.lock().await.get(&child_session_id).cloned() {
            let mut session = session_arc.lock().await;
            session.summary.status = SessionRuntimeStatus::Idle;
            session.summary.updated_at = Utc::now();
        }
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id: child_session_id,
                status: SessionRuntimeStatus::Idle,
            },
        ))
        .await;
        self.record_subagent_status_event_with_text(
            parent_session_id,
            child_session_id,
            SubagentStatus::Failed,
            TurnId::new(),
            Some(format!("failed to start child agent: {error_message}")),
        )
        .await;
        self.run_subagent_stop_hook(child_session_id).await;
    }

    pub(super) async fn interrupt_child_runtime_work(
        self: &Arc<Self>,
        child_session_id: SessionId,
    ) -> Option<TurnMetadata> {
        if let Some(cancel_token) = self
            .active_turn_cancellations
            .lock()
            .await
            .remove(&child_session_id)
        {
            cancel_token.cancel();
        }
        if let Some(task) = self.active_tasks.lock().await.remove(&child_session_id) {
            task.abort();
        }
        let session_arc = self.sessions.lock().await.get(&child_session_id).cloned()?;
        let mut session = session_arc.lock().await;
        session.summary.status = SessionRuntimeStatus::Idle;
        session.summary.updated_at = Utc::now();
        session.active_turn.take().map(|mut turn| {
            turn.status = TurnStatus::Interrupted;
            turn.completed_at = Some(Utc::now());
            session.latest_turn = Some(turn.clone());
            turn
        })
    }
}

fn subagent_hook_extra(
    base: &devo_core::HookBaseInput,
    stop_hook_active: Option<bool>,
) -> serde_json::Map<String, serde_json::Value> {
    let agent_id = base
        .agent_id
        .clone()
        .unwrap_or_else(|| base.session_id.clone());
    let agent_type = base
        .agent_type
        .clone()
        .unwrap_or_else(|| "default".to_string());
    let mut extra = serde_json::Map::from_iter([
        ("agent_id".to_string(), serde_json::Value::String(agent_id)),
        (
            "agent_type".to_string(),
            serde_json::Value::String(agent_type),
        ),
    ]);
    if let Some(stop_hook_active) = stop_hook_active {
        extra.insert(
            "stop_hook_active".to_string(),
            serde_json::Value::Bool(stop_hook_active),
        );
        extra.insert(
            "agent_transcript_path".to_string(),
            serde_json::Value::String(base.transcript_path.clone()),
        );
    }
    extra
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
    use devo_core::tools::AgentToolCoordinator;
    use devo_core::tools::ToolRegistry;
    use devo_protocol::AgentInfo;
    use devo_protocol::AgentListParams;
    use devo_protocol::ModelRequest;
    use devo_protocol::ModelResponse;
    use devo_protocol::StreamEvent;
    use devo_protocol::WaitAgentParams;
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

    fn build_runtime(data_root: &std::path::Path) -> Result<Arc<ServerRuntime>> {
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(NoopProvider);
        let db_path = data_root.join("startup_failure.db");
        let db = Arc::new(crate::db::Database::open(db_path).expect("open test database"));
        Ok(ServerRuntime::new(
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
        ))
    }

    #[tokio::test]
    async fn startup_failure_marks_failed_and_records_output_event() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path())?;
        let parent_session_id = SessionId::new();
        let child_session_id = SessionId::new();
        let spawned_at = Utc::now();
        runtime
            .agent_output_buffers
            .lock()
            .await
            .entry(parent_session_id)
            .or_default();
        runtime
            .agent_registries
            .lock()
            .await
            .entry(parent_session_id)
            .or_insert_with(AgentRegistry::new)
            .register(
                parent_session_id,
                child_session_id,
                SubagentMetadata {
                    session_id: child_session_id,
                    parent_session_id,
                    agent_path: "root/review".to_string(),
                    nickname: "review".to_string(),
                    role: "default".to_string(),
                    status: SubagentStatus::Spawning,
                    spawned_at,
                    closed_at: None,
                    last_task_message: Some("review this".to_string()),
                    close_requested: false,
                },
            );

        runtime
            .fail_child_agent_startup(
                parent_session_id,
                child_session_id,
                "rollout append failed".to_string(),
            )
            .await;

        let agents = Arc::clone(&runtime)
            .list_agents(AgentListParams {
                session_id: parent_session_id,
                path_prefix: None,
            })
            .await?;
        assert_eq!(
            agents,
            vec![AgentInfo {
                session_id: child_session_id,
                parent_session_id: Some(parent_session_id),
                agent_path: "root/review".to_string(),
                agent_nickname: "review".to_string(),
                agent_role: "default".to_string(),
                status: "failed".to_string(),
                last_task_message: Some("review this".to_string()),
            }]
        );
        let wait_result = Arc::clone(&runtime)
            .wait_agent(WaitAgentParams {
                session_id: parent_session_id,
                target: None,
                after_sequence: None,
                timeout_ms: Some(1),
            })
            .await?;
        assert_eq!(
            wait_result.events,
            vec![devo_protocol::AgentOutputEvent {
                sequence: 1,
                child_session_id,
                agent_path: "root/review".to_string(),
                turn_id: wait_result.events[0].turn_id,
                kind: "status".to_string(),
                text: Some("failed to start child agent: rollout append failed".to_string()),
                status: Some("failed".to_string()),
                created_at: wait_result.events[0].created_at,
            }]
        );

        Ok(())
    }
}
