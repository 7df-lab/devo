use std::collections::HashMap;
use std::time::Duration;

use super::*;

mod coordinator;
pub(in crate::runtime) mod handlers;
mod lifecycle;

const AGENT_NAME_ADJECTIVES: &[&str] = &[
    "brave", "clever", "silent", "happy", "gentle", "swift", "bright", "lazy", "wild", "calm",
    "fuzzy", "tiny", "bold", "lucky", "mighty",
];
const AGENT_NAME_NOUNS: &[&str] = &[
    "apple", "banana", "orange", "peach", "mango", "tiger", "panda", "fox", "rabbit", "eagle",
    "koala", "lion", "whale", "otter", "wolf",
];

impl ServerRuntime {
    async fn spawn_agent_inner(
        self: &Arc<Self>,
        params: devo_protocol::SpawnAgentParams,
    ) -> Result<devo_protocol::SpawnAgentResult, ToolCallError> {
        let parent_session_id = params.session_id;
        let child_session_id = devo_protocol::native::ids::SessionId::new();
        let now = Utc::now();
        if let Some(fork_turns) = params.fork_turns.as_deref()
            && !matches!(fork_turns, "none" | "all")
        {
            return Err(ToolCallError::InvalidInput(
                "fork_turns must be \"none\" or \"all\"".to_string(),
            ));
        }
        let effective_tool_policy = params.tool_policy;
        let fork_turns = params.fork_turns.as_deref().unwrap_or("all");
        if params.max_turns == Some(0) {
            return Err(ToolCallError::InvalidInput(
                "max_turns must be positive when provided".to_string(),
            ));
        }

        let parent_handle = self.session(parent_session_id).await.ok_or_else(|| {
            ToolCallError::InvalidInput(format!("session not found: {parent_session_id}"))
        })?;
        let parent_snapshot = if let Some(snapshot) = self
            .active_spawn_snapshot_for_session(parent_session_id)
            .await
        {
            snapshot
        } else {
            parent_handle.spawn_snapshot().await.ok_or_else(|| {
                ToolCallError::InvalidInput(format!(
                    "failed to snapshot parent session: {parent_session_id}"
                ))
            })?
        };
        let stable_items = if fork_turns == "all" {
            parent_snapshot.stable_items
        } else {
            Vec::new()
        };

        let parent_summary = parent_snapshot.parent_summary;
        let parent_config = parent_snapshot.parent_config;
        let parent_latest_turn = parent_snapshot.parent_latest_turn;
        let parent_active_turn_id = parent_snapshot.parent_active_turn_id;
        let parent_tool_registry = parent_snapshot.parent_tool_registry;
        let runtime_context = parent_snapshot.runtime_context;
        let parent_usage_turn_id = parent_active_turn_id
            .or_else(|| parent_latest_turn.as_ref().map(|turn| turn.turn_id()));

        let nickname = match params
            .nickname
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            // RLM `spawn(name=…)` contract: the requested name IS the child's
            // address (agent_message/observe/delete all match on it), so honor
            // it verbatim and only reject duplicates among siblings.
            Some(requested) => {
                let taken = {
                    let registries = self.agent_registries.lock().await;
                    registries
                        .get(&parent_session_id)
                        .map(|registry| {
                            registry
                                .children_of(&parent_session_id)
                                .into_iter()
                                .filter_map(|child_id| registry.get(&child_id))
                                .any(|meta| meta.nickname == requested)
                        })
                        .unwrap_or(false)
                };
                if taken {
                    return Err(ToolCallError::InvalidInput(format!(
                        "agent name already in use: {requested}"
                    )));
                }
                requested.to_string()
            }
            None => {
                self.generate_unique_agent_name(parent_session_id, child_session_id)
                    .await?
            }
        };
        let role = "default".to_string();
        let parent_path = parent_summary
            .agent_path
            .clone()
            .unwrap_or_else(|| "root".to_string());
        let agent_path = AgentPath::new(parent_path).join(&nickname).0;
        let model = parent_summary.model_name().map(str::to_string);
        let model_binding_id = parent_summary.model_binding_id().map(str::to_string);
        let reasoning_effort_selection = parent_summary.settings.reasoning_effort.clone();

        let mut native_session = parent_summary.native.clone();
        native_session.id = child_session_id;
        native_session.version = 1;
        native_session.parent = Some(devo_protocol::native::session::SessionParent::Agent {
            session_id: parent_summary.native.id,
            role: Some(role.clone()),
        });
        native_session.fork_from_id = None;
        native_session.at_turn_id = None;
        native_session.ephemeral = params.ephemeral;
        native_session.created_at = now;
        native_session.status = devo_protocol::native::session::SessionStatus::Idle;
        native_session.flags.clear();
        native_session.archived = false;
        native_session.activity = devo_protocol::native::session::SessionActivity::Idle;
        native_session.active_turn_id = None;
        native_session.queued_count = 0;
        native_session.title = Some(nickname.clone());
        native_session.title_state =
            SessionTitleState::Final(SessionTitleFinalSource::ExplicitCreate);
        native_session.model.provider = model_binding_id.unwrap_or_else(|| "unknown".to_string());
        native_session.model.model = model.clone().unwrap_or_default();
        native_session.model.reasoning_effort = None;
        native_session.settings.reasoning_effort = reasoning_effort_selection;
        native_session.settings.mode = Some("build".to_string());
        native_session.settings.effective_context_window = parent_summary
            .settings
            .effective_context_window
            .or_else(|| {
                parent_config
                    .effective_context_window_override
                    .map(|limit| limit as u64)
            });
        native_session.preview = params.message.clone();
        native_session.last_activity_at = now;
        native_session.transcript_size_bytes = None;
        native_session.usage = devo_protocol::native::usage::SessionUsage {
            total: devo_protocol::native::usage::UsageTotals::default(),
            by_purpose: Vec::new(),
            legacy: None,
            updated_at: now,
        };

        let rollout_path = if params.ephemeral {
            None
        } else {
            let parent_rollout = parent_handle.rollout_path().await.flatten();
            let mut invented = self
                .rollout_store
                .invent_child_session_persistence(
                    parent_rollout.as_deref(),
                    &parent_session_id,
                    &child_session_id,
                )
                .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
            invented.extras.collaboration_mode = Some(parent_summary.collaboration_mode);
            invented.extras.permission_preset = parent_summary.permission_preset();
            self.rollout_store
                .append_session_meta_at(
                    &invented.rollout_path,
                    &native_session,
                    Some(invented.extras),
                )
                .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
            Some(invented.rollout_path)
        };

        // Child artifact directory (rollout parent dir): surfaced on the spawn
        // result and subagent registry so RLM handles can address child files.
        let child_session_dir = rollout_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|dir| dir.display().to_string());
        let rollout_path_for_db = rollout_path.clone();
        let mut core_session = runtime_context.new_session_state(
            child_session_id,
            parent_summary.cwd.clone(),
            parent_summary.additional_directories.clone(),
        );
        core_session.config = parent_config.clone();
        let mut rebuilt_history_items = Vec::new();
        let mut rebuilt_messages = Vec::new();
        let mut tool_names_by_id = HashMap::new();
        for item in &stable_items {
            crate::prompt_from_native_item::apply_native_item(
                &mut rebuilt_messages,
                &mut rebuilt_history_items,
                &mut tool_names_by_id,
                item.item.clone(),
            );
        }
        core_session.messages = rebuilt_messages;
        core_session.turn_count = stable_items
            .iter()
            .filter(|item| crate::persisted_native_item::is_user_message(&item.item))
            .count();
        let pending_turn_queue = Arc::clone(&core_session.pending_turn_queue);
        let steer_input_queue = Arc::clone(&core_session.steer_input_queue);
        let latest_turn = if stable_items.is_empty() {
            None
        } else {
            parent_latest_turn.map(|mut turn| {
                turn.native.session_id = native_session.id;
                turn
            })
        };
        let mut summary = crate::runtime_session_summary::RuntimeSessionSummary::new(
            native_session,
            now,
            Default::default(),
        );
        summary.agent_path = Some(agent_path.clone());
        summary.agent_nickname = Some(nickname.clone());
        summary.agent_role = Some(role.clone());
        summary.prompt_token_estimate = core_session.prompt_token_estimate;
        let child_session = RuntimeSession {
            runtime_context,
            rollout_path,
            summary: summary.clone(),
            config: parent_config,
            core_session: Arc::new(Mutex::new(core_session)),
            active_turn: None,
            latest_turn,
            loaded_item_count: u64::try_from(stable_items.len()).unwrap_or(u64::MAX),
            history_items: rebuilt_history_items,
            persisted_turn_items: stable_items,
            latest_compaction_snapshot: None,
            turns_by_id: std::collections::HashMap::new(),
            pending_turn_queue,
            steer_input_queue,
            agent_tool_policy: effective_tool_policy,
            max_turns: params.max_turns,
            deferred_assistant: None,
            deferred_reasoning: None,
            next_item_seq: 1,
            first_user_input: Some(params.message.clone()),
            tool_registry: parent_tool_registry,
            file_read_ledger: std::sync::Arc::new(devo_core::tools::FileReadLedger::new()),
            session_approval_cache: crate::execution::ApprovalGrantCache::default(),
            turn_approval_cache: crate::execution::ApprovalGrantCache::default(),
            session_context_recorded: false,
        };
        let child_state = SessionActorState::from_runtime_session(child_session);
        let child_handle = self.insert_session_actor(child_state).await;
        self.agent_mailboxes
            .lock()
            .await
            .entry(parent_session_id)
            .or_default();
        self.agent_mailboxes
            .lock()
            .await
            .entry(child_session_id)
            .or_default();
        self.agent_output_buffers
            .lock()
            .await
            .entry(parent_session_id)
            .or_default();
        self.register_child_agent(
            parent_session_id,
            child_session_id,
            SubagentMetadata {
                session_id: child_session_id,
                parent_session_id,
                agent_path: agent_path.clone(),
                nickname: nickname.clone(),
                role: role.clone(),
                status: SubagentStatus::Spawning,
                spawned_at: now,
                closed_at: None,
                last_task_message: Some(params.message.clone()),
                close_requested: false,
                session_dir: child_session_dir.clone(),
            },
        )
        .await;
        if let Some(parent_turn_id) = parent_usage_turn_id {
            self.record_subagent_status_event(
                parent_session_id,
                child_session_id,
                SubagentStatus::Spawning,
                parent_turn_id,
            )
            .await;
        }
        self.register_subagent_usage_owner(
            parent_session_id,
            child_session_id,
            parent_usage_turn_id,
        )
        .await;
        if !summary.ephemeral
            && let Err(error) = self.deps.db.upsert_session(
                &summary,
                rollout_path_for_db.as_deref().map(std::path::Path::new),
            )
        {
            tracing::warn!(
                session_id = %child_session_id,
                error = %error,
                "failed to persist child session metadata to database"
            );
        }
        let start_runtime = Arc::clone(self);
        let start_session = child_handle
            .native_session()
            .await
            .expect("newly inserted child session actor has a Native session");
        let start_message = params.message.clone();
        tracing::debug!(
            parent_session_id = %parent_session_id,
            child_session_id = %child_session_id,
            agent_path = %agent_path,
            "subagent startup task spawned"
        );
        let start_parent_session_id = parent_session_id;
        let start_child_session_id = child_session_id;
        tokio::spawn(async move {
            start_runtime
                .broadcast_notification(
                    devo_protocol::native::event::ServerNotification::SessionCreated {
                        session: Box::new(start_session),
                    },
                )
                .await;
            start_runtime
                .run_subagent_start_hook(start_child_session_id)
                .await;
            if start_runtime
                .agent_close_requested(start_parent_session_id, start_child_session_id)
                .await
            {
                return;
            }
            match start_runtime
                .start_runtime_turn(
                    start_child_session_id,
                    start_message.clone(),
                    start_message,
                    /*queued_metadata*/ None,
                )
                .await
            {
                Ok(_) => {
                    if start_runtime
                        .agent_close_requested(start_parent_session_id, start_child_session_id)
                        .await
                    {
                        let _ = start_runtime
                            .close_child_agent(start_parent_session_id, start_child_session_id)
                            .await;
                        return;
                    }
                    start_runtime
                        .set_agent_status(
                            start_parent_session_id,
                            start_child_session_id,
                            SubagentStatus::Running,
                        )
                        .await;
                }
                Err(error) => {
                    let error_message = error.to_string();
                    tracing::warn!(
                        parent_session_id = %start_parent_session_id,
                        child_session_id = %start_child_session_id,
                        error = %error_message,
                        "failed to start child agent turn"
                    );
                    if start_runtime
                        .agent_close_requested(start_parent_session_id, start_child_session_id)
                        .await
                    {
                        return;
                    }
                    start_runtime
                        .fail_child_agent_startup(
                            start_parent_session_id,
                            start_child_session_id,
                            error_message,
                        )
                        .await;
                }
            }
        });

        let legacy_child_session_id = child_session_id;
        Ok(devo_protocol::SpawnAgentResult {
            task_id: devo_protocol::TaskId::from(legacy_child_session_id),
            child_session_id: legacy_child_session_id,
            agent_path,
            agent_nickname: nickname,
            status: SubagentStatus::Spawning.as_str().to_string(),
            session_dir: child_session_dir,
            model,
        })
    }

    pub(super) async fn mailbox(&self, session_id: SessionId) -> SubagentMailbox {
        self.agent_mailboxes
            .lock()
            .await
            .entry(session_id)
            .or_default()
            .clone()
    }

    async fn output_buffer(&self, parent_session_id: SessionId) -> SubagentOutputBuffer {
        self.agent_output_buffers
            .lock()
            .await
            .entry(parent_session_id)
            .or_default()
            .clone()
    }

    async fn register_child_agent(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
        metadata: SubagentMetadata,
    ) {
        self.agent_registries
            .lock()
            .await
            .entry(parent_session_id)
            .or_insert_with(AgentRegistry::new)
            .register(parent_session_id, child_session_id, metadata);
    }

    /// Re-registers a hydrated subagent session into its parent's agent
    /// registry.
    ///
    /// Registry linkage is spawn-time, in-process state: after a server
    /// restart the parent's registry starts empty, and a child hydrated
    /// lazily (resume, approval recovery, Agents View) has no parent
    /// linkage — `agent_message.send(receiver_role="parent")` then fails
    /// with "no parent for this session", the child's report is lost, and
    /// any parent flow awaiting the child hangs forever. Identity is
    /// rebuilt from durable state: `Session.parent`, the spawn convention
    /// that the child's title IS its nickname, the agent path joined onto
    /// the parent's path, and the latest turn status. A child the registry
    /// already knows (spawned in this process) is left untouched so live
    /// status is never clobbered.
    pub(super) async fn restore_agent_registry_entry_for_hydrated_child(
        &self,
        runtime_session: &crate::execution::RuntimeSession,
    ) {
        let summary = &runtime_session.summary;
        let Some(parent_session_id) = summary.parent_session_id() else {
            return;
        };
        let child_session_id = summary.session_id();
        if self.child_parent_and_path(child_session_id).await.is_some() {
            return;
        }
        let nickname = summary
            .agent_nickname
            .clone()
            .or_else(|| summary.native.title.clone())
            .unwrap_or_else(|| child_session_id.to_string());
        let parent_path = self.session_agent_path(parent_session_id).await;
        let agent_path = AgentPath::new(parent_path).join(&nickname).0;
        let status = runtime_session
            .latest_turn
            .as_ref()
            .map(|turn| terminal_subagent_status_from_turn(turn.native.status, false))
            .unwrap_or(SubagentStatus::Running);
        let session_dir = runtime_session
            .rollout_path
            .as_deref()
            .and_then(|path| path.parent())
            .map(|dir| dir.display().to_string());
        self.register_child_agent(
            parent_session_id,
            child_session_id,
            SubagentMetadata {
                session_id: child_session_id,
                parent_session_id,
                agent_path,
                nickname,
                role: summary
                    .agent_role
                    .clone()
                    .unwrap_or_else(|| "default".to_string()),
                status,
                spawned_at: summary.native.created_at,
                closed_at: None,
                last_task_message: None,
                close_requested: false,
                session_dir,
            },
        )
        .await;
    }

    async fn set_agent_status(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
        status: SubagentStatus,
    ) {
        if let Some(registry) = self
            .agent_registries
            .lock()
            .await
            .get_mut(&parent_session_id)
        {
            registry.update_status(child_session_id, status);
        }
    }

    /// Drain bash/python async completion notices into a short follow-up turn
    /// when the session is idle. Shared by parked Python cells and `bash.completed`.
    pub(crate) async fn drain_async_tool_completion_notices(
        self: &Arc<Self>,
        session_id: SessionId,
    ) {
        let sid = session_id.to_string();
        let notices = crate::runtime::kernel_host::take_bash_notices(&sid);
        if notices.is_empty() {
            return;
        }

        let mut completion_notices = Vec::new();
        let mut deferred = Vec::new();
        for notice in notices {
            match notice.get("kind").and_then(|k| k.as_str()) {
                Some("async_python_completion") | Some("async_bash_completion") => {
                    completion_notices.push(notice);
                }
                _ => deferred.push(notice),
            }
        }
        for notice in deferred {
            crate::runtime::kernel_host::push_bash_notice(&sid, notice);
        }
        if completion_notices.is_empty() {
            return;
        }

        let busy = self
            .session_turn_reservation_snapshot(session_id)
            .await
            .is_some_and(|r| r.active_turn.is_some())
            || self.active_stream_state(session_id).await.is_some();
        if busy {
            for notice in completion_notices {
                crate::runtime::kernel_host::push_bash_notice(&sid, notice);
            }
            return;
        }

        let text = completion_notices
            .iter()
            .map(|notice| match notice.get("kind").and_then(|k| k.as_str()) {
                Some("async_python_completion") => format_python_completion_notice(notice),
                _ => format_bash_completion_notice(notice),
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if let Err(error) = self
            .start_runtime_turn(session_id, text.clone(), text, None)
            .await
        {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "failed to start follow-up turn for async tool completion"
            );
            for notice in completion_notices {
                crate::runtime::kernel_host::push_bash_notice(&sid, notice);
            }
        }
    }

    pub(super) async fn start_runtime_turn(
        self: &Arc<Self>,
        session_id: SessionId,
        display_input: String,
        input_text: String,
        queued_metadata: Option<serde_json::Value>,
    ) -> Result<crate::turn::RuntimeTurn, ToolCallError> {
        let session_handle = self.session(session_id).await.ok_or_else(|| {
            ToolCallError::InvalidInput(format!("session not found: {session_id}"))
        })?;
        let _state_change_guard = session_handle.lock_state_change().await;

        let reservation = self
            .session_turn_reservation_snapshot(session_id)
            .await
            .ok_or_else(|| {
                ToolCallError::InvalidInput(format!(
                    "failed to snapshot session reservation: {session_id}"
                ))
            })?;

        if reservation.max_turns.is_some_and(|max_turns| {
            max_turns == 0
                || reservation
                    .active_turn
                    .as_ref()
                    .is_some_and(|turn| turn.native.sequence >= max_turns)
                || reservation
                    .latest_turn
                    .as_ref()
                    .is_some_and(|turn| turn.native.sequence >= max_turns)
        }) {
            return Err(ToolCallError::InvalidInput(
                "agent maximum turn count reached".to_string(),
            ));
        }

        if let Some(active_turn) = reservation.active_turn.clone() {
            let item = devo_protocol::PendingInputItem::new(
                devo_protocol::PendingInputKind::UserText { text: input_text },
                queued_metadata,
                Utc::now(),
            );
            session_handle
                .enqueue_pending_turn_input(item.clone())
                .await;
            if !reservation.ephemeral
                && let Err(error) = self
                    .deps
                    .db
                    .push_pending(&session_id, QueueType::Turn, &item)
            {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "failed to persist agent follow-up pending message"
                );
            }
            return Ok(active_turn);
        }

        let turn_config = reservation.runtime_context.resolve_turn_config(
            session_model_selection(&reservation.summary),
            reservation.summary.settings.reasoning_effort.clone(),
        );
        let resolved_request = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());

        let request_model = turn_config.provider_request_model(&resolved_request.request_model);
        let now = Utc::now();
        let sequence = reservation
            .latest_turn
            .as_ref()
            .map_or(1, |turn| turn.native.sequence + 1);
        let native_turn_id = devo_protocol::native::ids::TurnId::new();
        let turn_id = native_turn_id;
        let native_session_id = reservation.summary.native.id;
        let runtime_turn = crate::turn::RuntimeTurn {
            native: devo_protocol::native::turn::Turn {
                id: native_turn_id,
                session_id: native_session_id,
                sequence,
                kind: devo_protocol::native::turn::TurnKind::Regular,
                status: devo_protocol::native::turn::TurnStatus::InProgress,
                model: devo_protocol::native::model::ModelBinding {
                    provider: turn_config
                        .model_binding_id
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    model: request_model,
                    variant: None,
                    reasoning_effort: resolved_request.effective_reasoning_effort,
                },
                collaboration_mode: Some(devo_protocol::CollaborationMode::Build),
                started_at: now,
                completed_at: None,
                error: None,
                usage: None,
            },
            extras: crate::turn::RuntimeTurnExtras {
                request_thinking: resolved_request.request_thinking.clone(),
                stop_reason: None,
                failure_reason: None,
            },
        };

        session_handle
            .begin_runtime_turn(runtime_turn.clone(), turn_config.clone())
            .await;

        if let Err(error) = self.append_turn_start(session_id, &runtime_turn).await {
            let _ = session_handle.clear_active_turn_if_matches(turn_id).await;
            return Err(error);
        }

        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::session_status_changed(
                session_id,
                SessionStatus::Active,
                /*active_turn_id*/ None,
            ),
        )
        .await;
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::TurnStarted {
                turn: Box::new(runtime_turn.native.clone()),
            },
        )
        .await;

        self.add_active_turn_user_text(session_id, turn_id, display_input.clone())
            .await;
        let runtime = Arc::clone(self);
        let turn_for_task = runtime_turn.clone();
        let turn_config_for_task = turn_config.clone();
        if let Some(parent_session_id) = reservation.parent_session_id {
            self.active_turns
                .copy_connection_from_parent(session_id, parent_session_id)
                .await;
        }
        self.spawn_active_runtime_turn_task(session_id, runtime_turn.clone(), None, async move {
            runtime
                .execute_turn(ExecuteTurnRequest {
                    session_id,
                    turn: turn_for_task,
                    turn_config: turn_config_for_task,
                    display_input,
                    client_user_message_id: None,
                    input: input_text,
                    input_messages: Vec::new(),
                    input_images: Vec::new(),
                    input_image_paths: Vec::new(),
                    collaboration_mode: devo_protocol::CollaborationMode::Build,
                    input_mode: TurnInputMode::VisibleUserMessage,
                    user_message_already_emitted: false,
                })
                .await;
        })
        .await;
        Ok(runtime_turn)
    }

    async fn append_turn_start(
        self: &Arc<Self>,
        session_id: SessionId,
        turn: &crate::turn::RuntimeTurn,
    ) -> Result<(), ToolCallError> {
        let session_handle = self.session(session_id).await.ok_or_else(|| {
            ToolCallError::InvalidInput(format!("session not found: {session_id}"))
        })?;
        let persistence_snapshot = session_handle
            .turn_persistence_snapshot()
            .await
            .ok_or_else(|| {
                ToolCallError::InvalidInput(format!(
                    "failed to snapshot turn persistence: {session_id}"
                ))
            })?;
        // Child agents can be the first durable write for their rollout; route through
        // actor-owned dedupe so SessionContextUpdated is recorded even if the process
        // crashes before terminal turn finalization.
        if persistence_snapshot.rollout_path.is_some() {
            self.persist_turn_line_deduped(session_id, turn)
                .await
                .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
        }
        Ok(())
    }

    async fn queue_agent_message(
        &self,
        from_session_id: SessionId,
        target: &str,
        content: String,
    ) -> Result<AgentRoute, ToolCallError> {
        let route = self.resolve_agent_route(from_session_id, target).await?;
        let message = devo_protocol::AgentMailboxMessage {
            message_id: String::new(),
            from_session_id,
            to_session_id: route.to_session_id,
            from_agent_path: route.from_agent_path.clone(),
            to_agent_path: route.to_agent_path.clone(),
            content,
            sequence: 0,
            created_at: Utc::now(),
        };
        self.mailbox(route.to_session_id)
            .await
            .send(message)
            .await
            .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
        Ok(route.clone())
    }

    pub(in crate::runtime) async fn child_can_accept_next_turn(
        &self,
        session_id: SessionId,
    ) -> bool {
        let Some(reservation) = self.session_turn_reservation_snapshot(session_id).await else {
            return false;
        };
        !reservation.max_turns.is_some_and(|max_turns| {
            max_turns == 0
                || reservation
                    .active_turn
                    .as_ref()
                    .is_some_and(|turn| turn.native.sequence >= max_turns)
                || reservation
                    .latest_turn
                    .as_ref()
                    .is_some_and(|turn| turn.native.sequence >= max_turns)
        })
    }
    pub(in crate::runtime) async fn drain_child_mailbox_into_user_turns(
        self: &Arc<Self>,
        child_session_id: SessionId,
    ) -> Result<(), ToolCallError> {
        let messages = self.mailbox(child_session_id).await.drain().await;
        for message in messages {
            let parent_turn_id = self
                .active_turn_id_for_session(message.from_session_id)
                .await;
            if self
                .active_turn_id_for_session(child_session_id)
                .await
                .is_none()
            {
                self.register_subagent_usage_owner(
                    message.from_session_id,
                    child_session_id,
                    parent_turn_id,
                )
                .await;
            }
            self.start_runtime_turn(
                child_session_id,
                message.content.clone(),
                message.content,
                Some(subagent_usage_owner_pending_metadata(
                    message.from_session_id,
                    parent_turn_id,
                )),
            )
            .await?;
            if let Some((parent_session_id, _)) = self.child_parent_and_path(child_session_id).await
            {
                self.set_agent_status(parent_session_id, child_session_id, SubagentStatus::Running)
                    .await;
            }
        }
        Ok(())
    }

    pub(in crate::runtime) async fn resolve_child_agent(
        &self,
        parent_session_id: SessionId,
        target: &str,
    ) -> Result<SubagentMetadata, ToolCallError> {
        let registries = self.agent_registries.lock().await;
        let Some(registry) = registries.get(&parent_session_id) else {
            return Err(ToolCallError::InvalidInput(format!(
                "agent not found: {target}"
            )));
        };
        let Some(child_session_id) = registry.find_child(&parent_session_id, target) else {
            return Err(ToolCallError::InvalidInput(format!(
                "agent not found: {target}"
            )));
        };
        registry
            .get(&child_session_id)
            .cloned()
            .ok_or_else(|| ToolCallError::InvalidInput(format!("agent not found: {target}")))
    }

    pub(in crate::runtime) async fn agent_info(
        &self,
        parent_session_id: SessionId,
        target: &str,
    ) -> Result<devo_protocol::AgentInfo, ToolCallError> {
        Ok(self
            .resolve_child_agent(parent_session_id, target)
            .await?
            .to_agent_info())
    }

    async fn resolve_agent_route(
        &self,
        from_session_id: SessionId,
        target: &str,
    ) -> Result<AgentRoute, ToolCallError> {
        if let Ok(child) = self.resolve_child_agent(from_session_id, target).await {
            let from_path = self.session_agent_path(from_session_id).await;
            return Ok(AgentRoute {
                to_session_id: child.session_id,
                from_agent_path: from_path,
                to_agent_path: child.agent_path,
            });
        }
        Err(ToolCallError::InvalidInput(format!(
            "agent not found: {target}"
        )))
    }

    async fn resolve_wait_agent_targets(
        &self,
        parent_session_id: SessionId,
        target: Option<&str>,
    ) -> Result<Vec<SessionId>, ToolCallError> {
        let registries = self.agent_registries.lock().await;
        let Some(registry) = registries.get(&parent_session_id) else {
            return Ok(Vec::new());
        };
        if let Some(target) = target {
            let Some(child_session_id) = registry.find_child(&parent_session_id, target) else {
                return Err(ToolCallError::InvalidInput(format!(
                    "agent not found: {target}"
                )));
            };
            return Ok(vec![child_session_id]);
        }
        Ok(registry.children_of(&parent_session_id))
    }

    pub(super) async fn session_agent_path(&self, session_id: SessionId) -> String {
        let Some(session_handle) = self.session(session_id).await else {
            return "root".to_string();
        };
        let Some(summary) = session_handle.summary().await else {
            return "root".to_string();
        };
        summary.agent_path.unwrap_or_else(|| "root".to_string())
    }

    pub(super) async fn handle_subagent_turn_completed(
        &self,
        child_session_id: SessionId,
        turn: &crate::turn::RuntimeTurn,
    ) {
        let Some((parent_session_id, status)) = self
            .resolve_terminal_subagent_status(child_session_id, turn)
            .await
        else {
            return;
        };
        let detail = self
            .subagent_terminal_status_detail(child_session_id, turn.native.id, status)
            .await;
        self.finish_subagent_turn_completion(
            parent_session_id,
            child_session_id,
            turn,
            status,
            detail,
        )
        .await;
        if subagent_stop_hook_applies(status) {
            self.run_subagent_stop_hook(child_session_id).await;
        }
    }

    /// Same as `handle_subagent_turn_completed`, but reads any data owned by
    /// the currently-executing session actor directly from `state` instead of
    /// round-tripping through the session actor mailbox.
    ///
    /// Must be used when `child_session_id` is the session actor currently
    /// executing this code: that actor's mailbox is not being polled until
    /// the in-flight turn finishes, so a mailbox round-trip here would
    /// deadlock forever waiting on itself.
    pub(super) async fn handle_subagent_turn_completed_for_actor_state(
        &self,
        state: &SessionActorState,
        child_session_id: SessionId,
        turn: &crate::turn::RuntimeTurn,
    ) {
        let Some((parent_session_id, _agent_path)) =
            self.child_parent_and_path(child_session_id).await
        else {
            return;
        };
        let status = terminal_subagent_status_from_turn(
            turn.native.status,
            self.agent_close_requested(parent_session_id, child_session_id)
                .await,
        );
        let detail = subagent_terminal_status_detail_from_stable_items(
            &state.persisted_turn_items,
            turn.native.id,
            status,
        );
        self.set_agent_status(parent_session_id, child_session_id, status)
            .await;
        self.record_subagent_status_event_with_text(
            parent_session_id,
            child_session_id,
            status,
            turn.turn_id(),
            detail,
        )
        .await;
        if subagent_stop_hook_applies(status) {
            self.run_subagent_stop_hook_for_actor_state(state, child_session_id)
                .await;
        }
    }

    async fn resolve_terminal_subagent_status(
        &self,
        child_session_id: SessionId,
        turn: &crate::turn::RuntimeTurn,
    ) -> Option<(SessionId, SubagentStatus)> {
        let (parent_session_id, _agent_path) = self.child_parent_and_path(child_session_id).await?;
        let status = terminal_subagent_status_from_turn(
            turn.native.status,
            self.agent_close_requested(parent_session_id, child_session_id)
                .await,
        );
        Some((parent_session_id, status))
    }

    async fn finish_subagent_turn_completion(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
        turn: &crate::turn::RuntimeTurn,
        status: SubagentStatus,
        detail: Option<String>,
    ) {
        self.set_agent_status(parent_session_id, child_session_id, status)
            .await;
        self.record_subagent_status_event_with_text(
            parent_session_id,
            child_session_id,
            status,
            turn.turn_id(),
            detail,
        )
        .await;
    }

    async fn subagent_terminal_status_detail(
        &self,
        child_session_id: SessionId,
        turn_id: devo_protocol::native::ids::TurnId,
        status: SubagentStatus,
    ) -> Option<String> {
        if status != SubagentStatus::Failed {
            return None;
        }
        let session_handle = self.sessions.lock().await.get(&child_session_id).cloned()?;
        let snapshot = session_handle.spawn_snapshot().await?;
        subagent_terminal_status_detail_from_stable_items(&snapshot.stable_items, turn_id, status)
    }

    pub(super) async fn child_parent_and_path(
        &self,
        child_session_id: SessionId,
    ) -> Option<(SessionId, String)> {
        let registries = self.agent_registries.lock().await;
        registries.values().find_map(|registry| {
            let parent_session_id = registry.child_to_parent.get(&child_session_id).cloned()?;
            let agent_path = registry.get(&child_session_id)?.agent_path.clone();
            Some((parent_session_id, agent_path))
        })
    }

    pub(super) async fn record_subagent_output_notification(
        &self,
        notification: &devo_protocol::native::event::ServerNotification,
    ) {
        let devo_protocol::native::event::ServerNotification::ItemAssistantMessageDelta(delta) =
            notification
        else {
            return;
        };
        if delta.delta.is_empty() {
            return;
        }
        let child_session_id = delta.session_id;
        let Some((parent_session_id, agent_path)) =
            self.child_parent_and_path(child_session_id).await
        else {
            return;
        };
        let buffer = self.output_buffer(parent_session_id).await;
        let output_event = devo_protocol::AgentOutputEvent {
            sequence: 0,
            child_session_id,
            agent_path,
            turn_id: None,
            kind: devo_protocol::AgentOutputEventKind::AssistantMessage,
            text: Some(delta.delta.clone()),
            status: None,
            created_at: Utc::now(),
        };
        // Streaming text must not block behind wait_agent's buffer lock. If the
        // lock is busy, skip this delta for the wait buffer; the TUI already
        // received it via outbound notifications, and a later delta/status will
        // refresh the coalesced text.
        let _ = buffer.try_push_text_delta(output_event);
    }

    async fn record_subagent_status_event(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
        status: SubagentStatus,
        turn_id: TurnId,
    ) {
        self.record_subagent_status_event_with_text(
            parent_session_id,
            child_session_id,
            status,
            turn_id,
            None,
        )
        .await;
    }

    async fn record_subagent_status_event_with_text(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
        status: SubagentStatus,
        turn_id: TurnId,
        text: Option<String>,
    ) {
        let agent_path = match self.child_parent_and_path(child_session_id).await {
            Some((event_parent_session_id, agent_path))
                if event_parent_session_id == parent_session_id =>
            {
                agent_path
            }
            _ => self.session_agent_path(child_session_id).await,
        };
        self.output_buffer(parent_session_id)
            .await
            .push(devo_protocol::AgentOutputEvent {
                sequence: 0,
                child_session_id,
                agent_path,
                turn_id: Some(turn_id),
                kind: devo_protocol::AgentOutputEventKind::Status,
                text,
                status: Some(status.as_str().to_string()),
                created_at: Utc::now(),
            })
            .await;
    }

    async fn agent_close_requested(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
    ) -> bool {
        self.agent_registries
            .lock()
            .await
            .get(&parent_session_id)
            .and_then(|registry| registry.get(&child_session_id))
            .is_some_and(|metadata| metadata.close_requested)
    }

    pub(super) async fn interrupt_all_child_agents(self: Arc<Self>, parent_session_id: SessionId) {
        let child_session_ids = {
            let registries = self.agent_registries.lock().await;
            registries
                .get(&parent_session_id)
                .map(|registry| registry.children_of(&parent_session_id))
                .unwrap_or_default()
        };
        for child_session_id in child_session_ids {
            let _ = self.interrupt_child_runtime_work(child_session_id).await;
            self.set_agent_status(
                parent_session_id,
                child_session_id,
                SubagentStatus::Interrupted,
            )
            .await;
        }
    }

    async fn close_child_agent(
        self: &Arc<Self>,
        parent_session_id: SessionId,
        child_session_id: SessionId,
    ) -> Result<String, ToolCallError> {
        let already_terminal = {
            let mut registries = self.agent_registries.lock().await;
            let Some(registry) = registries.get_mut(&parent_session_id) else {
                return Err(ToolCallError::InvalidInput(format!(
                    "agent not found: {child_session_id}"
                )));
            };
            let Some(metadata) = registry.agents.get_mut(&child_session_id) else {
                return Err(ToolCallError::InvalidInput(format!(
                    "agent not found: {child_session_id}"
                )));
            };
            let terminal = matches!(
                metadata.status,
                SubagentStatus::Completed
                    | SubagentStatus::Failed
                    | SubagentStatus::Interrupted
                    | SubagentStatus::Canceled
                    | SubagentStatus::Closed
            );
            metadata.close_requested = true;
            if !terminal {
                metadata.status = SubagentStatus::Closed;
                metadata.closed_at = Some(Utc::now());
            }
            terminal
        };
        // A running turn's cancellation is now observed by the session actor's
        // own turn-completion handling (`handle_subagent_turn_completed_for_actor_state`),
        // which independently resolves and records the terminal "closed" status
        // once it sees `close_requested`. Track whether a turn was actually in
        // flight so we don't also send a duplicate closed notification below.
        let had_active_turn = self.active_turns.has_session(child_session_id).await;
        let interrupted_turn = self.interrupt_child_runtime_work(child_session_id).await;
        if already_terminal && interrupted_turn.is_none() {
            let status = self
                .resolve_child_agent(parent_session_id, child_session_id.as_ref())
                .await?
                .status
                .as_str()
                .to_string();
            return Ok(status);
        }

        if let Some(turn) = interrupted_turn {
            self.broadcast_notification(
                devo_protocol::native::event::ServerNotification::TurnCompleted {
                    turn: Box::new(turn.native.clone()),
                },
            )
            .await;
            self.handle_subagent_turn_completed(child_session_id, &turn)
                .await;
        } else if !had_active_turn {
            self.send_closed_notification(parent_session_id, child_session_id)
                .await;
        }
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::session_status_changed(
                child_session_id,
                SessionStatus::Idle,
                /*active_turn_id*/ None,
            ),
        )
        .await;
        Ok(SubagentStatus::Closed.as_str().to_string())
    }

    async fn send_closed_notification(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
    ) {
        self.record_subagent_status_event(
            parent_session_id,
            child_session_id,
            SubagentStatus::Closed,
            TurnId::new(),
        )
        .await;
        self.run_subagent_stop_hook(child_session_id).await;
    }

    async fn generate_unique_agent_name(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
    ) -> Result<String, ToolCallError> {
        let used_names = {
            let registries = self.agent_registries.lock().await;
            registries
                .get(&parent_session_id)
                .map(|registry| {
                    registry
                        .children_of(&parent_session_id)
                        .into_iter()
                        .filter_map(|child_id| registry.get(&child_id))
                        .map(|metadata| metadata.nickname.clone())
                        .collect::<std::collections::HashSet<_>>()
                })
                .unwrap_or_default()
        };
        let max_count = AGENT_NAME_ADJECTIVES.len() * AGENT_NAME_NOUNS.len();
        if used_names.len() >= max_count {
            return Err(ToolCallError::InvalidInput(
                "no unique generated agent names available".to_string(),
            ));
        }
        let start = generated_name_start_index(child_session_id, max_count);
        for offset in 0..max_count {
            let index = (start + offset) % max_count;
            let adjective = AGENT_NAME_ADJECTIVES[index / AGENT_NAME_NOUNS.len()];
            let noun = AGENT_NAME_NOUNS[index % AGENT_NAME_NOUNS.len()];
            let candidate = format!("{adjective}-{noun}");
            if !used_names.contains(&candidate) {
                return Ok(candidate);
            }
        }
        Err(ToolCallError::InvalidInput(
            "no unique generated agent names available".to_string(),
        ))
    }

    /// Kernel `host_request("agent_observe.list")` — nuclear family roster.
    pub(crate) async fn host_agent_observe_list(&self, session_id: &str) -> serde_json::Value {
        let sid = SessionId::from_string(session_id.to_owned());
        let family = self.observe_family_summaries(sid).await;
        let current = family
            .iter()
            .find(|a| a.get("sessionId").and_then(|v| v.as_str()) == Some(session_id))
            .cloned();
        serde_json::json!({
            "status": "ok",
            "current": current,
            "agents": family,
        })
    }

    /// Kernel `host_request("agent_observe.get")`.
    pub(crate) async fn host_agent_observe_get(
        &self,
        session_id: &str,
        params: &serde_json::Value,
    ) -> serde_json::Value {
        let sid = SessionId::from_string(session_id.to_owned());
        let target = params
            .get("target")
            .or_else(|| params.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if target.is_empty() {
            return serde_json::json!({
                "status": "error",
                "error": "agent_observe.get requires target"
            });
        }
        let family = self.observe_family_summaries(sid).await;
        let matched: Vec<_> = family
            .into_iter()
            .filter(|a| observe_target_matches(a, target))
            .collect();
        if matched.len() != 1 {
            return serde_json::json!({
                "status": "error",
                "error": format!(
                    "agent_observe.get: expected exactly one match for '{target}', found {}",
                    matched.len()
                ),
            });
        }
        serde_json::json!({ "status": "ok", "agent": matched[0] })
    }

    /// Kernel `host_request("agent_observe.recent")`.
    pub(crate) async fn host_agent_observe_recent(
        &self,
        session_id: &str,
        params: &serde_json::Value,
    ) -> serde_json::Value {
        let get = self.host_agent_observe_get(session_id, params).await;
        if get.get("status").and_then(|v| v.as_str()) != Some("ok") {
            return get;
        }
        let agent = get.get("agent").cloned().unwrap_or(serde_json::Value::Null);
        let Some(target_sid) = agent
            .get("sessionId")
            .and_then(|v| v.as_str())
            .map(|s| SessionId::from_string(s.to_owned()))
        else {
            return serde_json::json!({
                "status": "error",
                "error": "agent_observe.recent: missing sessionId"
            });
        };
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(8)
            .clamp(1, 50) as usize;
        let max_chars = params
            .get("max_chars")
            .or_else(|| params.get("maxChars"))
            .and_then(|v| v.as_u64())
            .unwrap_or(800)
            .clamp(80, 2000) as usize;
        let messages = self
            .observe_recent_messages(target_sid, limit, max_chars)
            .await;
        serde_json::json!({
            "status": "ok",
            "agent": agent,
            "messages": messages,
            "limit": limit,
            "maxChars": max_chars,
            "truncated": messages.len() >= limit,
        })
    }

    pub(super) async fn observe_family_summaries(
        &self,
        session_id: SessionId,
    ) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        let parent = self.child_parent_and_path(session_id).await.map(|(p, _)| p);
        let root = parent.unwrap_or(session_id);

        out.push(self.observe_summary_for(session_id, "self").await);

        if let Some(parent_id) = parent {
            out.push(self.observe_summary_for(parent_id, "parent").await);
            let siblings: Vec<SessionId> = {
                let registries = self.agent_registries.lock().await;
                registries
                    .get(&parent_id)
                    .map(|r| {
                        r.children_of(&parent_id)
                            .into_iter()
                            .filter(|c| *c != session_id)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            for sib in siblings {
                out.push(self.observe_summary_for(sib, "sibling").await);
            }
        }

        let children: Vec<SessionId> = {
            let registries = self.agent_registries.lock().await;
            registries
                .get(&root)
                .map(|r| r.children_of(&session_id))
                .or_else(|| {
                    registries
                        .get(&session_id)
                        .map(|r| r.children_of(&session_id))
                })
                .unwrap_or_default()
        };
        for child in children {
            out.push(self.observe_summary_for(child, "child").await);
        }
        out
    }

    async fn observe_summary_for(
        &self,
        session_id: SessionId,
        relationship: &str,
    ) -> serde_json::Value {
        let handle = self.session(session_id).await;
        let summary = if let Some(ref h) = handle {
            h.summary().await
        } else {
            None
        };
        let nickname = summary
            .as_ref()
            .and_then(|s| s.agent_nickname.clone())
            .or_else(|| summary.as_ref().and_then(|s| s.title.clone()));
        let path = summary
            .as_ref()
            .and_then(|s| s.agent_path.clone())
            .unwrap_or_else(|| "root".to_string());
        let active = self.runtime_active_turn_id(session_id).await.is_some();
        serde_json::json!({
            "sessionId": session_id.to_string(),
            "sessionName": nickname,
            "agentPath": path,
            "relationship": relationship,
            "status": if active { "running" } else { "idle" },
            "isSessionActive": active,
        })
    }

    async fn observe_recent_messages(
        &self,
        session_id: SessionId,
        limit: usize,
        max_chars: usize,
    ) -> Vec<serde_json::Value> {
        let Some(handle) = self.session(session_id).await else {
            return Vec::new();
        };
        // Live transcript INCLUDING the active turn's items: observing a child
        // mid-turn must show the messages it has already produced, not an
        // empty fork snapshot (spawn_snapshot filters the in-flight turn out).
        let Some(items) = handle.transcript_items().await else {
            return Vec::new();
        };
        let mut messages = Vec::new();
        for item in items.iter().rev() {
            let text = match &item.item {
                devo_protocol::native::item::Item::UserMessage { content, .. } => {
                    let text = content
                        .iter()
                        .filter_map(|part| match part {
                            devo_protocol::native::item::UserInput::Text { text } => {
                                Some(text.as_str())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some(format!("user: {text}"))
                }
                devo_protocol::native::item::Item::AssistantMessage { text, .. } => {
                    Some(format!("assistant: {text}"))
                }
                devo_protocol::native::item::Item::Reasoning { text, .. } => {
                    Some(format!("reasoning: {text}"))
                }
                _ => None,
            };
            let Some(mut text) = text else { continue };
            if text.chars().count() > max_chars {
                text = text.chars().take(max_chars).collect::<String>() + "…";
            }
            messages.push(serde_json::json!({
                "role": "message",
                "text": text,
            }));
            if messages.len() >= limit {
                break;
            }
        }
        messages.reverse();
        messages
    }

    /// Map a registry status onto the RLM subagent wire statuses
    /// (`running | completed | error` — the kernel-side contract has three).
    fn rlm_subagent_status(status: SubagentStatus) -> &'static str {
        match status {
            SubagentStatus::Spawning | SubagentStatus::Running => "running",
            SubagentStatus::WaitingForInput
            | SubagentStatus::Completed
            | SubagentStatus::Interrupted
            | SubagentStatus::Canceled
            | SubagentStatus::Closed => "completed",
            SubagentStatus::Failed => "error",
        }
    }

    fn rlm_subagent_entry(meta: &SubagentMetadata) -> serde_json::Value {
        // Kernel-side `session_dir` must be a non-empty string; fall back to the
        // canonical per-session artifacts dir when the registry has no recorded
        // rollout dir (e.g. children registered by older builds).
        let session_dir = meta.session_dir.clone().or_else(|| {
            devo_util_paths::find_devo_home()
                .ok()
                .map(|home| {
                    home.join("session-artifacts")
                        .join(meta.session_id.as_str())
                })
                .map(|dir| dir.display().to_string())
        });
        serde_json::json!({
            "rlm_child_id": meta.session_id.to_string(),
            "session_id": meta.session_id.to_string(),
            "active_session_id": meta.session_id.to_string(),
            "session_name": meta.nickname,
            "session_dir": session_dir.unwrap_or_else(|| ".".to_string()),
            "status": Self::rlm_subagent_status(meta.status),
        })
    }

    /// Kernel `host_request("rlm.list_subagents")` — direct children retained
    /// by this session, in the payload shape `rlm.list_subagents()` parses.
    pub(crate) async fn host_rlm_list_subagents(&self, session_id: &str) -> serde_json::Value {
        let sid = SessionId::from_string(session_id.to_owned());
        let entries: Vec<serde_json::Value> = {
            let registries = self.agent_registries.lock().await;
            registries
                .get(&sid)
                .map(|registry| {
                    registry
                        .children_of(&sid)
                        .into_iter()
                        .filter_map(|child_id| registry.get(&child_id))
                        .map(Self::rlm_subagent_entry)
                        .collect()
                })
                .unwrap_or_default()
        };
        serde_json::json!({ "status": "ok", "subagents": entries })
    }

    /// Kernel `host_request("rlm.delete_subagent")` — close (stop) one direct
    /// child: an in-flight turn is interrupted, the child is marked closed, and
    /// the final registry entry is returned in the `subagent` slot.
    pub(crate) async fn host_rlm_delete_subagent(
        self: &Arc<Self>,
        session_id: &str,
        params: &serde_json::Value,
    ) -> serde_json::Value {
        let sid = SessionId::from_string(session_id.to_owned());
        let Some(target) = params.get("target").and_then(|v| v.as_str()).map(str::trim) else {
            return serde_json::json!({
                "status": "error",
                "error": "rlm.delete_subagent requires target"
            });
        };
        if target.is_empty() {
            return serde_json::json!({
                "status": "error",
                "error": "rlm.delete_subagent requires target"
            });
        }
        let child = match self.resolve_child_agent(sid, target).await {
            Ok(child) => child,
            Err(error) => {
                return serde_json::json!({
                    "status": "error",
                    "error": error.to_string(),
                });
            }
        };
        if let Err(error) = self.close_child_agent(sid, child.session_id).await {
            return serde_json::json!({ "status": "error", "error": error.to_string() });
        }
        let entry = {
            let registries = self.agent_registries.lock().await;
            registries
                .get(&sid)
                .and_then(|registry| registry.get(&child.session_id))
                .map(Self::rlm_subagent_entry)
        };
        match entry {
            Some(entry) => serde_json::json!({ "status": "ok", "subagent": entry }),
            None => serde_json::json!({
                "status": "error",
                "error": format!("agent not found after close: {target}")
            }),
        }
    }
}

fn observe_target_matches(agent: &serde_json::Value, target: &str) -> bool {
    observe_target_matches_for_host(agent, target)
}

pub(super) fn observe_target_matches_for_host(agent: &serde_json::Value, target: &str) -> bool {
    let t = target.to_ascii_lowercase();
    agent
        .get("sessionId")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.eq_ignore_ascii_case(target) || s.to_ascii_lowercase().ends_with(&t))
        || agent
            .get("sessionName")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.eq_ignore_ascii_case(target))
        || agent
            .get("agentPath")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.eq_ignore_ascii_case(target) || s.ends_with(target))
}

#[derive(Clone)]
struct AgentRoute {
    to_session_id: SessionId,
    from_agent_path: String,
    to_agent_path: String,
}

fn terminal_subagent_status_from_turn(
    turn_status: devo_protocol::native::turn::TurnStatus,
    close_requested: bool,
) -> SubagentStatus {
    let status = match turn_status {
        devo_protocol::native::turn::TurnStatus::Completed => SubagentStatus::Completed,
        devo_protocol::native::turn::TurnStatus::Interrupted => SubagentStatus::Interrupted,
        devo_protocol::native::turn::TurnStatus::Failed => SubagentStatus::Failed,
        devo_protocol::native::turn::TurnStatus::InProgress
        | devo_protocol::native::turn::TurnStatus::WaitingApproval => SubagentStatus::Running,
    };
    if close_requested {
        SubagentStatus::Closed
    } else {
        status
    }
}

fn format_python_completion_notice(notice: &serde_json::Value) -> String {
    let params = notice
        .get("params")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    if let Some(text) = params.get("text").and_then(|v| v.as_str()) {
        return text.to_string();
    }
    let cell_id = params
        .get("cellId")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let status = params
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let stdout = params
        .get("stdoutTail")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let stderr = params
        .get("stderrTail")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let error = params.get("error").and_then(|v| v.as_str());
    let mut body =
        format!("[system] Background Python cell `{cell_id}` finished with status `{status}`.");
    if !stdout.is_empty() {
        body.push_str("\nStdout tail:\n");
        body.push_str(stdout);
    }
    if !stderr.is_empty() {
        body.push_str("\nStderr tail:\n");
        body.push_str(stderr);
    }
    if let Some(error) = error {
        body.push_str("\nError: ");
        body.push_str(error);
    }
    body.push_str("\nContinue from this result.");
    body
}

fn format_bash_completion_notice(notice: &serde_json::Value) -> String {
    let params = notice
        .get("params")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    if let Some(text) = params.get("text").and_then(|v| v.as_str()) {
        return text.to_string();
    }
    format!(
        "[system] Background bash handle completed: {}\nInspect the saved handle and continue.",
        params
    )
}

fn subagent_stop_hook_applies(status: SubagentStatus) -> bool {
    matches!(
        status,
        SubagentStatus::Completed
            | SubagentStatus::Failed
            | SubagentStatus::Interrupted
            | SubagentStatus::Canceled
            | SubagentStatus::Closed
    )
}

fn subagent_terminal_status_detail_from_stable_items(
    stable_items: &[crate::execution::PersistedTurnItem],
    turn_id: devo_protocol::native::ids::TurnId,
    status: SubagentStatus,
) -> Option<String> {
    if status != SubagentStatus::Failed {
        return None;
    }
    stable_items.iter().rev().find_map(|item| {
        if item.turn_id != turn_id {
            return None;
        }
        match &item.item {
            devo_protocol::native::item::Item::AssistantMessage { text, .. }
                if !text.trim().is_empty() =>
            {
                Some(text.trim().to_string())
            }
            _ => None,
        }
    })
}

fn generated_name_start_index(child_session_id: SessionId, max_count: usize) -> usize {
    child_session_id
        .to_string()
        .bytes()
        .fold(0usize, |acc, byte| {
            acc.wrapping_mul(31).wrapping_add(usize::from(byte))
        })
        % max_count
}
