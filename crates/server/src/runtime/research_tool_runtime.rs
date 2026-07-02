use std::sync::Arc;

use devo_core::SessionState;
use tokio_util::sync::CancellationToken;

use super::*;

impl ServerRuntime {
    pub(super) async fn scratch_session(
        &self,
        session_id: SessionId,
    ) -> anyhow::Result<SessionState> {
        let Some(session_handle) = self.session(session_id).await else {
            anyhow::bail!("session does not exist");
        };
        let Some(runtime_session) = session_handle.export_runtime_session().await else {
            anyhow::bail!("session does not exist");
        };
        let core_session = runtime_session.core_session.lock().await;
        let mut scratch = SessionState::new(core_session.config.clone(), core_session.cwd.clone());
        scratch.id = session_id.to_string();
        Ok(scratch)
    }

    pub(super) async fn tool_runtime_for_research(
        self: &Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
        turn_config: &TurnConfig,
        registry: Arc<ToolRegistry>,
    ) -> anyhow::Result<ToolRuntime> {
        let Some(session_handle) = self.session(session_id).await else {
            anyhow::bail!("session does not exist");
        };
        let Some(snapshot) = session_handle.hook_context_snapshot().await else {
            anyhow::bail!("session does not exist");
        };
        let cwd = snapshot.summary.cwd.clone();
        let permission_mode = snapshot.config.permission_mode;
        let permission_profile = snapshot.config.permission_profile.clone();
        let runtime_context = snapshot.runtime_context;
        let provider_http = runtime_context
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .provider_http
            .clone();
        let turn_cancel_token = self
            .active_turn_cancellations
            .lock()
            .await
            .get(&session_id)
            .cloned()
            .unwrap_or_else(CancellationToken::new);
        let tool_execution_start_runtime = Arc::clone(self);
        Ok(ToolRuntime::new_with_context_and_options(
            registry,
            self.build_permission_checker(session_id, turn_id, permission_mode, permission_profile),
            ToolRuntimeContext {
                session_id: session_id.to_string(),
                turn_id: Some(turn_id.to_string()),
                cwd,
                agent_scope: ToolAgentScope::Parent,
                agent_context_mode: devo_protocol::AgentContextMode::DeepResearch,
                collaboration_mode: devo_protocol::CollaborationMode::Build,
                agent_coordinator: Some(Arc::clone(self) as Arc<dyn AgentToolCoordinator>),
                client_filesystem: Some(Arc::clone(self) as Arc<dyn ClientFilesystem>),
                client_terminal: Some(Arc::clone(self) as Arc<dyn ClientTerminal>),
                local_web_search: match &turn_config.web_search {
                    devo_core::ResolvedWebSearchConfig::Local(config) => Some(config.clone()),
                    devo_core::ResolvedWebSearchConfig::Disabled
                    | devo_core::ResolvedWebSearchConfig::Provider => None,
                },
                hooks: self.hook_context_for_session(session_id).await,
                network_proxy: provider_http.proxy_url,
                network_no_proxy: provider_http.no_proxy,
            },
            ToolExecutionOptions {
                cancel_token: turn_cancel_token,
                on_tool_execution_start: Some(Arc::new(move |call: ToolCall| {
                    let runtime = Arc::clone(&tool_execution_start_runtime);
                    let tool_call_id = call.id;
                    Box::pin(async move {
                        runtime
                            .broadcast_event(ServerEvent::ToolCallStatusUpdated(
                                devo_protocol::ToolCallStatusUpdatedPayload {
                                    session_id,
                                    turn_id,
                                    tool_call_id,
                                    status: "in_progress".to_string(),
                                    terminal_id: None,
                                },
                            ))
                            .await;
                    })
                })),
                ..ToolExecutionOptions::default()
            },
        ))
    }
}
