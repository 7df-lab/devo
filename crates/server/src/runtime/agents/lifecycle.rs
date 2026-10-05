use super::*;

/// How long a cancelled child turn gets to unwind on its own before
/// `interrupt_child_runtime_work`'s orphan-recovery branch force-aborts it.
const CHILD_TURN_UNWIND_TIMEOUT: Duration = Duration::from_secs(2);

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
        Self::run_subagent_stop_hook_with_context(context).await;
    }

    /// Same as `run_subagent_stop_hook`, but reads hook context directly from
    /// `state` instead of round-tripping through the session actor mailbox.
    ///
    /// Must be used when `child_session_id` is the session actor currently
    /// executing this code: that actor's mailbox is not being polled until
    /// the in-flight turn finishes, so a mailbox round-trip here would
    /// deadlock forever waiting on itself.
    pub(super) async fn run_subagent_stop_hook_for_actor_state(
        &self,
        state: &SessionActorState,
        child_session_id: SessionId,
    ) {
        let Some(context) = Self::hook_context_from_actor_state(state, child_session_id) else {
            return;
        };
        Self::run_subagent_stop_hook_with_context(context).await;
    }

    async fn run_subagent_stop_hook_with_context(context: devo_core::HookRuntimeContext) {
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
        if let Some(session_handle) = self.sessions.lock().await.get(&child_session_id).cloned() {
            session_handle.set_runtime_session_idle(None).await;
        }
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::session_status_changed(
                child_session_id,
                SessionStatus::Idle,
                /*active_turn_id*/ None,
            ),
        )
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
    ) -> Option<crate::turn::RuntimeTurn> {
        let turn_id = self.active_turns.active_turn_id(child_session_id).await;
        // Subscribe before signalling so a fast unwind cannot publish the
        // terminal status between the cancel and the wait below.
        let terminal_rx = match turn_id {
            Some(turn_id) => {
                let receiver = self.subscribe_terminal_turn_status(turn_id).await;
                Some((turn_id, receiver))
            }
            None => None,
        };
        // Cancel via a clone rather than `remove` so a concurrent read of the
        // same token (e.g. `run_turn_model_query` fetching it to race against
        // the in-flight query) cannot lose the signal by finding the map entry
        // already gone and falling back to a fresh, disconnected token. The
        // entry itself is cleaned up later by `finalize_executed_turn`.
        if let Some(cancel_token) = self.active_turns.cancel_token(child_session_id).await {
            cancel_token.cancel();
        }
        // Let the turn task unwind itself first: `finalize_executed_turn`
        // records the terminal status and clears the registry entry. Aborting
        // immediately races that unwind and, when the abort wins, the leaked
        // registry entry keeps projecting the child as active in
        // `session/read` / `session/list` (and the TUI subagents panel) long
        // after the interruption.
        if let Some((turn_id, terminal_rx)) = terminal_rx
            && self.recent_terminal_turn_status(turn_id).await.is_none()
        {
            let _ = tokio::time::timeout(CHILD_TURN_UNWIND_TIMEOUT, terminal_rx).await;
        }
        if self
            .active_turns
            .active_turn_id(child_session_id)
            .await
            .is_some()
        {
            // The unwind lost the race (or the task is stuck): force-abort and
            // drop the leaked handles so status projections report idle. The
            // actor side is brought down by `interrupt_active_turn` below.
            tracing::warn!(
                session_id = %child_session_id,
                turn_id = ?turn_id,
                "child turn task did not unwind after cancel; forcing registry cleanup"
            );
            self.active_turns.abort_task(child_session_id).await;
            self.clear_active_turn_runtime_handles(child_session_id)
                .await;
            self.broadcast_notification(
                devo_protocol::native::event::ServerNotification::session_status_changed(
                    child_session_id,
                    SessionStatus::Idle,
                    /*active_turn_id*/ None,
                ),
            )
            .await;
        }
        let session_handle = self.sessions.lock().await.get(&child_session_id).cloned()?;
        session_handle.interrupt_active_turn().await?
    }
}

fn subagent_hook_extra(
    base: &devo_core::HookBaseInput,
    stop_hook_active: Option<bool>,
) -> serde_json::Map<String, serde_json::Value> {
    let agent_id = base
        .agent_id
        .clone()
        .unwrap_or_else(|| base.session_id.to_string());
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
    use devo_core::tools::AgentToolCoordinator;
    use devo_protocol::AgentInfo;
    use devo_protocol::AgentListParams;
    use devo_protocol::SuccessResponse;
    use devo_protocol::WaitAgentParams;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    fn build_runtime(data_root: &std::path::Path) -> Result<Arc<ServerRuntime>> {
        Ok(crate::test_support::TestRuntime::noop()
            .db_file("startup_failure.db")
            .runtime(data_root))
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
                    session_dir: None,
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
                parent_session_id: Some(parent_session_id,),
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
                timeout_secs: Some(0),
            })
            .await?;
        assert_eq!(
            wait_result.events,
            vec![devo_protocol::ParentAgentOutputEvent {
                sequence: 1,
                agent_path: "root/review".to_string(),
                agent_nickname: "review".to_string(),
                kind: devo_protocol::AgentOutputEventKind::Status,
                text: Some("failed to start child agent: rollout append failed".to_string()),
                status: Some("failed".to_string()),
            }]
        );

        Ok(())
    }

    #[tokio::test]
    async fn rlm_list_subagents_reports_children_in_kernel_payload_shape() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path())?;
        let parent_session_id = SessionId::new();
        let running_child = SessionId::new();
        let failed_child = SessionId::new();
        {
            let mut registries = runtime.agent_registries.lock().await;
            let registry = registries
                .entry(parent_session_id)
                .or_insert_with(AgentRegistry::new);
            registry.register(
                parent_session_id,
                running_child,
                SubagentMetadata {
                    session_id: running_child,
                    parent_session_id,
                    agent_path: "root/child-a".to_string(),
                    nickname: "child-a".to_string(),
                    role: "default".to_string(),
                    status: SubagentStatus::Running,
                    spawned_at: Utc::now(),
                    closed_at: None,
                    last_task_message: None,
                    close_requested: false,
                    session_dir: Some("/tmp/artifacts/child-a".to_string()),
                },
            );
            registry.register(
                parent_session_id,
                failed_child,
                SubagentMetadata {
                    session_id: failed_child,
                    parent_session_id,
                    agent_path: "root/child-b".to_string(),
                    nickname: "child-b".to_string(),
                    role: "default".to_string(),
                    status: SubagentStatus::Failed,
                    spawned_at: Utc::now(),
                    closed_at: None,
                    last_task_message: None,
                    close_requested: false,
                    session_dir: None,
                },
            );
        }

        let reply = runtime
            .host_rlm_list_subagents(parent_session_id.as_ref())
            .await;
        assert_eq!(reply["status"], "ok");
        let entries = reply["subagents"].as_array().expect("subagents array");
        assert_eq!(entries.len(), 2);

        let child_a = entries
            .iter()
            .find(|entry| entry["session_name"] == "child-a")
            .expect("child-a entry");
        assert_eq!(child_a["rlm_child_id"], running_child.to_string());
        assert_eq!(child_a["session_id"], running_child.to_string());
        assert_eq!(child_a["session_dir"], "/tmp/artifacts/child-a");
        assert_eq!(child_a["status"], "running");

        let child_b = entries
            .iter()
            .find(|entry| entry["session_name"] == "child-b")
            .expect("child-b entry");
        assert_eq!(child_b["status"], "error");
        assert!(
            child_b["session_dir"]
                .as_str()
                .is_some_and(|dir| !dir.is_empty()),
            "kernel parser requires a non-empty session_dir fallback"
        );

        Ok(())
    }

    #[tokio::test]
    async fn child_agent_turn_start_failure_clears_active_turn() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path())?;
        let value = runtime
            .start_session_with_registry(
                /*connection_id*/ 1,
                serde_json::json!(1),
                SessionStartParams {
                    cwd: data_root.path().to_path_buf(),
                    additional_directories: Vec::new(),
                    ephemeral: false,
                    title: None,
                    model: None,
                    model_binding_id: None,
                },
                None,
            )
            .await;
        let response: SuccessResponse<SessionStartResult> =
            serde_json::from_value(value).expect("session start response");
        let session_id = SessionId::from(response.result.session.id.as_str());
        let bad_rollout_path = data_root.path().join("rollout-dir");
        std::fs::create_dir(&bad_rollout_path)?;
        let session_handle = runtime.session(session_id).await.expect("session");
        session_handle.update_rollout_path(bad_rollout_path).await;

        let error = runtime
            .start_runtime_turn(
                session_id,
                "inspect this".to_string(),
                "inspect this".to_string(),
                /*queued_metadata*/ None,
            )
            .await
            .expect_err("append failure");
        let reservation = session_handle
            .turn_reservation_snapshot()
            .await
            .expect("turn reservation snapshot");
        let summary = session_handle.summary().await.expect("summary");

        assert!(matches!(error, ToolCallError::InternalError(_)));
        assert_eq!(reservation.active_turn, None);
        assert_eq!(
            summary.status,
            devo_protocol::native::session::SessionStatus::Idle
        );
        assert_eq!(reservation.latest_turn, None);

        Ok(())
    }
}
