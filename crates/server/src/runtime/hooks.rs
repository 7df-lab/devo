use serde_json::Map;
use serde_json::Value;

use super::*;

impl ServerRuntime {
    pub(crate) fn hook_runner(&self) -> Option<devo_core::HookRunner> {
        let config = {
            let config_store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            config_store.effective_config().hooks.clone()
        };
        (!config.is_empty()).then(|| devo_core::HookRunner::new(config))
    }

    pub(crate) async fn hook_context_for_session(
        &self,
        session_id: SessionId,
    ) -> Option<devo_core::HookRuntimeContext> {
        let runner = self.hook_runner()?;
        let session_arc = self.sessions.lock().await.get(&session_id).cloned()?;
        let session = session_arc.lock().await;
        let transcript_path = session
            .record
            .as_ref()
            .map(|record| record.rollout_path.display().to_string())
            .unwrap_or_default();
        let permission_mode = Some(permission_mode_label(session.config.permission_mode));
        let agent_id = session
            .summary
            .parent_session_id
            .is_some()
            .then(|| session_id.to_string());
        let agent_type = session
            .summary
            .agent_role
            .clone()
            .or_else(|| session.summary.agent_nickname.clone());
        Some(devo_core::HookRuntimeContext {
            runner,
            base: devo_core::HookBaseInput {
                session_id: session_id.to_string(),
                transcript_path,
                cwd: session.summary.cwd.clone(),
                permission_mode,
                agent_id,
                agent_type,
            },
        })
    }

    pub(crate) async fn run_session_hook(
        &self,
        session_id: SessionId,
        event: devo_core::HookEvent,
        extra: Map<String, Value>,
    ) -> devo_core::HookRunReport {
        let Some(context) = self.hook_context_for_session(session_id).await else {
            return devo_core::HookRunReport::default();
        };
        let input = devo_core::HookInput::new(&context.base, event, extra);
        context.runner.run(input).await
    }

    pub(crate) async fn run_global_hook(
        &self,
        event: devo_core::HookEvent,
        extra: Map<String, Value>,
    ) -> devo_core::HookRunReport {
        let Some(runner) = self.hook_runner() else {
            return devo_core::HookRunReport::default();
        };
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let base = devo_core::HookBaseInput {
            session_id: String::new(),
            transcript_path: String::new(),
            cwd,
            permission_mode: None,
            agent_id: None,
            agent_type: None,
        };
        runner
            .run(devo_core::HookInput::new(&base, event, extra))
            .await
    }

    pub(crate) async fn config_change_hook_block_reason(
        &self,
        source: &str,
        file_path: Option<String>,
    ) -> Option<String> {
        let mut extra = Map::from_iter([("source".to_string(), Value::String(source.to_string()))]);
        if let Some(file_path) = file_path {
            extra.insert("file_path".to_string(), Value::String(file_path));
        }
        self.run_global_hook(devo_core::HookEvent::ConfigChange, extra)
            .await
            .first_blocking_reason()
            .map(str::to_string)
    }
}

pub(crate) fn permission_mode_label(mode: devo_safety::PermissionMode) -> String {
    match mode {
        devo_safety::PermissionMode::AutoApprove => "auto-approve",
        devo_safety::PermissionMode::Interactive => "interactive",
        devo_safety::PermissionMode::Deny => "deny",
    }
    .to_string()
}
