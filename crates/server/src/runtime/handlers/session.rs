use super::super::*;

use devo_core::CanonicalHistory;
use devo_core::SessionSettingsField;
use devo_protocol::native::rpc_session::RollbackMode;

/// Default page size for canonical `session/list` when no limit is given.
const CANONICAL_SESSION_LIST_DEFAULT_LIMIT: usize = 50;

fn session_list_cwd_matches(cwds: &[std::path::PathBuf], cwd: &std::path::PathBuf) -> bool {
    if cwds.is_empty() {
        return true;
    }
    cwds.iter().any(|filter| {
        if filter == cwd {
            return true;
        }
        normalize_session_list_cwd(filter) == normalize_session_list_cwd(cwd)
    })
}

fn normalize_session_list_cwd(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

/// Native snapshot mapping for a stored permission preset.
///
/// `None` matches `new_session_state`: project config may override, otherwise
/// new sessions default to AutoReview ("Approve for me").
fn native_model_slug(binding: &devo_protocol::native::model::ModelBinding) -> String {
    let provider = binding.provider.trim();
    if provider.is_empty() || binding.model.starts_with(&format!("{provider}/")) {
        binding.model.clone()
    } else {
        format!("{provider}/{}", binding.model)
    }
}

fn native_permission_profile(
    preset: Option<devo_protocol::PermissionPreset>,
) -> devo_protocol::native::model::PermissionProfile {
    match preset {
        Some(devo_protocol::PermissionPreset::Default) => {
            devo_protocol::native::model::PermissionProfile::Default
        }
        Some(devo_protocol::PermissionPreset::FullAccess) => {
            devo_protocol::native::model::PermissionProfile::FullAccess
        }
        Some(devo_protocol::PermissionPreset::AutoReview) | None => {
            devo_protocol::native::model::PermissionProfile::AutoReview
        }
    }
}

pub(crate) struct RuntimeSessionTurnCutOptions {
    pub(crate) session_id: SessionId,
    pub(crate) user_turn_index: Option<u32>,
    pub(crate) rollback_mode: RollbackMode,
    pub(crate) cwd_override: Option<PathBuf>,
    pub(crate) title_override: Option<String>,
    pub(crate) created_at: chrono::DateTime<Utc>,
}

/// Resolve occupancy and latest-query usage for a history cut.
///
/// Prefers a compaction snapshot that still applies to the kept turns; otherwise
/// uses the last kept turn's stored occupancy / query usage.
pub(crate) fn resolve_cut_occupancy_and_usage(
    kept_turn_ids: &std::collections::HashSet<devo_core::TurnId>,
    last_kept_turn_id: Option<devo_core::TurnId>,
    turns_by_id: &std::collections::HashMap<devo_core::TurnId, crate::replay_hydrate::ReplayedTurn>,
    latest_compaction_snapshot: Option<&devo_core::CompactionSnapshotLine>,
) -> (
    Option<devo_protocol::native::item::ContextOccupancy>,
    Option<devo_protocol::native::usage::TurnUsage>,
    Option<devo_core::CompactionSnapshotLine>,
) {
    let cut_turn = last_kept_turn_id.and_then(|turn_id| turns_by_id.get(&turn_id));
    let applicable_compaction = latest_compaction_snapshot
        .filter(|snapshot| kept_turn_ids.contains(&snapshot.turn_id))
        .cloned();
    let occupancy = applicable_compaction
        .as_ref()
        .and_then(|snapshot| snapshot.context_occupancy.clone())
        .or_else(|| cut_turn.and_then(|turn| turn.extras.context_occupancy.clone()));
    let latest_query_usage = cut_turn.and_then(|turn| {
        turn.extras
            .latest_query_usage
            .clone()
            .or_else(|| turn.native.usage.clone())
    });
    (occupancy, latest_query_usage, applicable_compaction)
}

pub(crate) enum RuntimeSessionToolRegistryUpdate {
    KeepCurrent,
    ReplaceIfCwdMatches {
        cwd: PathBuf,
        tool_registry: Option<Arc<devo_core::tools::ToolRegistry>>,
    },
}

impl ServerRuntime {
    fn session_missing(&self, request_id: serde_json::Value) -> serde_json::Value {
        self.error_response(
            request_id,
            ProtocolErrorCode::SessionNotFound,
            "session does not exist",
        )
    }

    fn parse_rpc_params<T: serde::de::DeserializeOwned>(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
        method: &str,
    ) -> Result<T, serde_json::Value> {
        serde_json::from_value(params).map_err(|error| {
            self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("invalid canonical {method} params: {error}"),
            )
        })
    }

    async fn exported_runtime_session(
        &self,
        request_id: serde_json::Value,
        session_id: SessionId,
    ) -> Result<RuntimeSession, serde_json::Value> {
        let Some(handle) = self.session(session_id).await else {
            return Err(self.session_missing(request_id));
        };
        handle
            .export_runtime_session()
            .await
            .ok_or_else(|| self.session_missing(request_id))
    }

    pub(crate) async fn start_session_with_registry(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: SessionStartParams,
        tool_registry: Option<Arc<devo_core::tools::ToolRegistry>>,
    ) -> serde_json::Value {
        let now = Utc::now();
        let session_id = devo_protocol::native::ids::SessionId::new();
        let runtime_context = match self.deps.context_for_workspace(&params.cwd).await {
            Ok(context) => context,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to initialize session workspace: {error}"),
                );
            }
        };
        let requested_model = params
            .model_binding_id
            .as_deref()
            .or(params.model.as_deref());
        let initial_turn_config = runtime_context.resolve_turn_config(requested_model, None);
        let model = initial_turn_config.model.slug.clone();
        let model_binding_id = initial_turn_config.model_binding_id.clone();
        let initial_effort = initial_turn_config.reasoning_effort_selection.clone();
        let mut invented_persistence =
            (!params.ephemeral).then(|| self.rollout_store.invent_session_persistence(&session_id));
        let native_session = devo_protocol::native::session::Session {
            id: session_id,
            version: 1,
            cwd: params.cwd.clone(),
            additional_directories: params.additional_directories.clone(),
            parent: None,
            fork_from_id: None,
            at_turn_id: None,
            ephemeral: params.ephemeral,
            created_at: now,
            status: devo_protocol::native::session::SessionStatus::Idle,
            flags: Vec::new(),
            archived: false,
            activity: devo_protocol::native::session::SessionActivity::Idle,
            active_turn_id: None,
            queued_count: 0,
            title: params.title.clone(),
            title_state: params
                .title
                .as_ref()
                .map(|_| SessionTitleState::Final(SessionTitleFinalSource::ExplicitCreate))
                .unwrap_or(SessionTitleState::Unset),
            model: devo_protocol::native::model::ModelBinding {
                provider: model_binding_id.unwrap_or_else(|| "unknown".to_string()),
                model: model.clone(),
                variant: None,
                reasoning_effort: None,
            },
            settings: devo_protocol::native::session::SessionSettings {
                permission_profile: devo_protocol::native::model::PermissionProfile::AutoReview,
                reasoning_effort: initial_effort,
                mode: Some("build".to_string()),
                sandbox_profile: None,
                effective_context_window: None,
                auto_refine_enabled: None,
                auto_refine_turn_interval: None,
                python_cell_first_wait_ms: None,
            },
            git_info: None,
            preview: String::new(),
            last_activity_at: now,
            transcript_size_bytes: None,
            message_count: None,
            summary: None,
            task_state: None,
            usage: devo_protocol::native::usage::SessionUsage {
                total: devo_protocol::native::usage::UsageTotals::default(),
                by_purpose: Vec::new(),
                legacy: None,
                updated_at: now,
            },
        };
        let applied_compaction_limit = crate::runtime::context_occupancy::resolved_compaction_limit(
            &initial_turn_config.model,
        );
        let mut summary = crate::runtime_session_summary::RuntimeSessionSummary::new(
            native_session,
            now,
            Default::default(),
        );
        summary.settings.effective_context_window = Some(applied_compaction_limit);
        let mut core_session = runtime_context.new_session_state(
            session_id,
            params.cwd.clone(),
            params.additional_directories.clone(),
        );
        let permission_preset =
            protocol_preset_from_safety(core_session.config.permission_profile.preset);
        summary.set_permission_preset(permission_preset);
        if let Some(invented) = invented_persistence.as_mut() {
            invented.extras.permission_preset = Some(permission_preset);
        }
        if let Some(invented) = &invented_persistence
            && let Err(error) = self.rollout_store.append_session_meta_at(
                &invented.rollout_path,
                &summary.native,
                Some(invented.extras.clone()),
            )
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist session metadata: {error}"),
            );
        }
        crate::runtime::context_occupancy::apply_resolved_compaction_limit(
            &mut core_session.config,
            applied_compaction_limit as usize,
        );
        let config = core_session.config.clone();
        let pending_turn_queue = Arc::clone(&core_session.pending_turn_queue);
        let steer_input_queue = Arc::clone(&core_session.steer_input_queue);
        let rollout_path = invented_persistence
            .as_ref()
            .map(|entry| entry.rollout_path.clone());
        let rollout_path_for_db = rollout_path.clone();
        // Fresh sessions must hold their cross-process lock from creation:
        // a brand-new id cannot collide with an EXISTING session, but another
        // process can `devo resume` this id any time after this response, and
        // concurrent ownership would corrupt the rollout. A contention here
        // means another live process somehow created the same id — refuse.
        let session_file_lock = match rollout_path.as_deref() {
            Some(path) => match crate::runtime::session_cache::SessionFileLock::acquire(path) {
                Ok(lock) => Some(lock),
                Err(contention) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::SessionLocked,
                        format!(
                            "session rollout is already locked by another Devo process \
                             (pid {}); refusing to create a conflicting session",
                            contention.holder_pid
                        ),
                    );
                }
            },
            None => None,
        };
        let actor_state = SessionActorState {
            runtime_context,
            rollout_path,
            session_file_lock,
            summary: summary.clone(),
            config,
            core: core_session,
            stream: Arc::new(tokio::sync::Mutex::new(
                crate::runtime::session_actor::state::SessionStreamState::default(),
            )),
            active_turn: None,
            latest_turn: None,
            loaded_item_count: 0,
            history_items: Vec::new(),
            persisted_turn_items: Vec::new(),
            latest_compaction_snapshot: None,
            turns_by_id: std::collections::HashMap::new(),
            pending_turn_queue,
            steer_input_queue,
            agent_tool_policy: Default::default(),
            max_turns: None,
            next_item_seq: 1,
            first_user_input: None,
            tool_registry,
            file_read_ledger: Arc::new(devo_core::tools::FileReadLedger::new()),
            kernel: None,
            session_approval_cache: crate::execution::ApprovalGrantCache::default(),
            turn_approval_cache: crate::execution::ApprovalGrantCache::default(),
            session_context_recorded: false,
            transcript_leaf_id: None,
            leaf_epoch: 0,
        };
        let session_handle = self.insert_session_actor(actor_state).await;
        self.subscribe_connection_to_session(connection_id, session_id, None)
            .await;
        self.runtime_arc()
            .after_root_session_insert(session_id)
            .await;

        // Persist session metadata to SQLite (skip for ephemeral sessions)
        if !summary.ephemeral
            && let Err(err) = self
                .deps
                .db
                .upsert_session(&summary, rollout_path_for_db.as_deref())
        {
            tracing::warn!(
                session_id = %session_id,
                error = %err,
                "failed to persist session metadata to database"
            );
        }

        tracing::info!(
            connection_id,
            session_id = %session_id,
            cwd = %summary.cwd.display(),
            ephemeral = summary.ephemeral,
            model = ?summary.model,
            has_title = summary.title.is_some(),
            "started session"
        );
        let native_session = session_handle
            .native_session()
            .await
            .expect("newly inserted session actor has a Native session");
        self.broadcast_notification(
            devo_protocol::native::event::ServerNotification::SessionCreated {
                session: Box::new(native_session.clone()),
            },
        )
        .await;
        self.run_session_hook(
            session_id,
            devo_core::HookEvent::SessionStart,
            serde_json::Map::from_iter([
                ("source".to_string(), serde_json::json!("startup")),
                ("model".to_string(), serde_json::json!(model)),
            ]),
        )
        .await;

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionStartResult {
                session: native_session,
            },
        })
        .expect("serialize session/start response")
    }

    /// Lists first-party session snapshots in their canonical Native shape.
    ///
    /// SQLite rows are converted once at the persistence-index boundary and
    /// live actors replace those stale rows with actor-owned Native snapshots.
    pub(crate) async fn list_native_sessions(
        &self,
    ) -> Vec<devo_protocol::native::session::Session> {
        self.list_native_sessions_with_options(/*include_children*/ false)
            .await
    }

    pub(crate) async fn list_native_sessions_with_options(
        &self,
        include_children: bool,
    ) -> Vec<devo_protocol::native::session::Session> {
        let mut sessions_by_id = match if include_children {
            self.deps.db.list_sessions()
        } else {
            self.deps.db.list_root_sessions()
        } {
            Ok(sessions) => sessions
                .into_iter()
                .filter(|index_row| include_children || index_row.agent_path.is_none())
                .filter(|index_row| !index_row.ephemeral)
                .map(|index_row| {
                    let session = index_row.into_native_session();
                    (session.id, session)
                })
                .collect::<std::collections::HashMap<_, _>>(),
            Err(error) => {
                tracing::warn!(error = %error, "failed to list sessions from database");
                std::collections::HashMap::new()
            }
        };

        let handles = self.list_session_handles().await;
        let sessions = futures::future::join_all(
            handles
                .into_iter()
                .map(|handle| async move { handle.native_session().await }),
        )
        .await;
        for session in sessions.into_iter().flatten() {
            if session.ephemeral {
                continue;
            }
            if !include_children && session.parent.is_some() {
                continue;
            }
            sessions_by_id.insert(session.id, session);
        }

        let mut sessions = sessions_by_id.into_values().collect::<Vec<_>>();
        sessions.sort_by(|left, right| {
            right
                .last_activity_at
                .cmp(&left.last_activity_at)
                .then_with(|| right.usage.updated_at.cmp(&left.usage.updated_at))
        });
        sessions
    }

    /// Native `session/metadata/update` (L2-DES-APP-008 DD-4/DD-5,
    /// L2-DES-CONV-002 Phase 2): persist-first — settings field lines are
    /// written synchronously and the response is built from the rollout,
    /// never waiting on the session actor; the actor is notified best-effort
    /// (mailbox FIFO guarantees application before the next turn). Ephemeral
    /// sessions have no rollout: field lines are skipped (there is nothing
    /// to persist by design) and the snapshot is built from the SQLite
    /// index instead.
    pub(crate) async fn handle_native_session_metadata_update(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionMetadataUpdateParams =
            match self.parse_rpc_params(request_id.clone(), params, "session/metadata/update") {
                Ok(params) => params,
                Err(error) => return error,
            };
        let legacy_session_id = params.session_id;
        // Title patch: `Value` renames through the session actor and persists
        // the title update before the canonical snapshot is read below.
        match &params.title {
            devo_protocol::native::patch::PatchField::Missing => {}
            devo_protocol::native::patch::PatchField::Null => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "clearing the session title is not supported (titles cannot be empty)",
                );
            }
            devo_protocol::native::patch::PatchField::Value(title) => {
                let new_title = title.trim();
                if new_title.is_empty() {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        "session title cannot be empty",
                    );
                }
                let session_handle = match self.get_or_load_parent_session(legacy_session_id).await
                {
                    Ok(handle) => handle,
                    Err(crate::runtime::session_cache::LoadSessionError::SessionNotFound)
                    | Err(crate::runtime::session_cache::LoadSessionError::RolloutMissing) => {
                        return self.session_missing(request_id);
                    }
                    Err(crate::runtime::session_cache::LoadSessionError::SessionLocked {
                        holder_pid,
                    }) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::SessionLocked,
                            format!(
                                "session is already open in another Devo process (pid {holder_pid})"
                            ),
                        );
                    }
                    Err(crate::runtime::session_cache::LoadSessionError::RestoreFailed(
                        message,
                    )) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InternalError,
                            format!("failed to load session for metadata update: {message}"),
                        );
                    }
                };
                if let Some(summary) = session_handle.summary().await
                    && summary.agent_path.is_some()
                {
                    let parent = summary
                        .parent_session_id()
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "unknown".into());
                    return self.error_response(
                            request_id,
                            ProtocolErrorCode::InvalidParams,
                            format!(
                                "subagent sessions cannot be renamed directly; rename the parent session {parent} instead"
                            ),
                        );
                }
                {
                    self.cancel_auto_title_generation(legacy_session_id).await;
                    let _state_change_guard = session_handle.lock_state_change().await;
                    let previous_title = session_handle
                        .summary()
                        .await
                        .and_then(|summary| summary.title.clone());
                    let Some(mut summary) = session_handle
                        .set_session_title_user_rename(new_title.to_string())
                        .await
                    else {
                        return self.session_missing(request_id);
                    };
                    if let Some(rollout_path) = session_handle.rollout_path().await.flatten() {
                        if let Err(error) = self.rollout_store.append_title_update_at(
                            &rollout_path,
                            legacy_session_id,
                            new_title.to_string(),
                            previous_title,
                        ) {
                            return self.error_response(
                                request_id,
                                ProtocolErrorCode::InternalError,
                                format!("failed to persist session title update: {error}"),
                            );
                        }
                        summary = session_handle.summary().await.unwrap_or(summary);
                    }
                    self.persist_session_summary_if_persistent(legacy_session_id, &summary)
                        .await;
                    if let Some(session) = session_handle.native_session().await {
                        self.broadcast_notification(
                            devo_protocol::native::event::ServerNotification::SessionMetadataUpdated {
                                session: Box::new(session),
                            },
                        )
                        .await;
                    }
                }
            }
        }
        // Metadata updates target the durable session record: the live actor
        // is an implementation detail, not a precondition. Keep the actor
        // optional so a cold session can be updated before `session/resume`.
        let session_handle = self.session(legacy_session_id).await;
        let _metadata_write_permit = self
            .session_metadata_write_gate
            .acquire(legacy_session_id)
            .await;
        // Persist-first: never wait on the session actor, and never take
        // the state-change gate for a settings patch. Title generation and
        // finalize hold that gate across mailbox waits; taking it here
        // stalls the TUI's pre-turn `session/metadata/update` until the
        // 10s client timeout, after which `turn/start` hits the same gate.
        // Mailbox-free rollout resolution: SQLite index first, rollout scan
        // fallback (same sources as the subscription snapshot path). The
        // index metadata also supplies the current model/binding/effort
        // values, needed because the actor's metadata command overwrites
        // absent fields unless they are re-sent with their current values.
        let session_index = match self.deps.db.get_session_index(&legacy_session_id) {
            Ok(index) => index,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to read session index: {error}"),
                );
            }
        };
        let ephemeral_without_rollout = session_index
            .as_ref()
            .is_some_and(|index| index.session.ephemeral);
        let subagent_parent_session_id = session_index.as_ref().and_then(|index| {
            index
                .session
                .agent_path
                .as_ref()
                .and(index.session.parent_session_id)
        });
        let current_model_slug = session_index
            .as_ref()
            .and_then(|index| index.session.model.clone());
        let current_binding_id: Option<String> = None;
        let current_effort = session_index
            .as_ref()
            .and_then(|index| index.session.reasoning_effort_selection.clone());
        let mut index_session = session_index
            .as_ref()
            .map(|index| index.session.clone().into_native_session());
        let indexed_rollout_path = session_index.and_then(|index| index.rollout_path);
        let rollout_path = match self
            .resolve_existing_rollout_path(legacy_session_id, indexed_rollout_path)
        {
            Ok(path) => path,
            Err(error) => {
                return self.error_response(request_id, ProtocolErrorCode::InternalError, error);
            }
        };
        if let Some(parent_session_id) = subagent_parent_session_id {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!(
                    "subagent sessions cannot be updated directly; update the parent session {parent_session_id} instead"
                ),
            );
        }
        if rollout_path.is_none() && ephemeral_without_rollout && session_handle.is_none() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "ephemeral session is not live",
            );
        }
        // Ephemeral sessions have neither rollout nor an index row: the only
        // metadata source left is the actor summary (a mailbox read — the
        // blocking is scoped to the ephemeral degrade; durable paths never
        // wait on the actor).
        if index_session.is_none() && rollout_path.is_none() {
            let Some(handle) = session_handle.as_ref() else {
                return self.session_missing(request_id);
            };
            index_session = handle.summary().await.map(|summary| summary.native);
        }
        // Ephemeral degrade: no rollout → no field lines and an index-built
        // snapshot; durable → history-backed snapshot with version checks.
        let mut durable_rollout_session = None;
        let (
            session_version,
            session_model_slug,
            session_cwd,
            session_additional_dirs,
            mut current,
        ) = if let Some(rollout_path) = &rollout_path {
            let history = match devo_core::read_canonical_history(rollout_path) {
                Ok(history) => history,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to read session history: {error}"),
                    );
                }
            };
            durable_rollout_session = history.session;
            let Some(session) = durable_rollout_session.as_ref() else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionNotFound,
                    "session history has no metadata",
                );
            };
            if params.expected_version != 0 && params.expected_version != session.version {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::WorkspaceVersionConflict,
                    format!(
                        "session version drift: expected {}, current {}",
                        params.expected_version, session.version
                    ),
                );
            }
            (
                session.version,
                session.model.model.clone(),
                session.cwd.clone(),
                session.additional_directories.clone(),
                session.settings.clone(),
            )
        } else {
            let Some(index_session) = index_session.as_ref() else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionNotFound,
                    "session is not durable and has no index metadata",
                );
            };
            if params.expected_version > 1 {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::WorkspaceVersionConflict,
                    format!(
                        "session version drift: expected {}, current 1",
                        params.expected_version
                    ),
                );
            }
            (
                1,
                index_session.model.model.clone(),
                index_session.cwd.clone(),
                index_session.additional_directories.clone(),
                index_session.settings.clone(),
            )
        };
        let mut overlay_profile: Option<devo_safety::RuntimePermissionProfile> = None;
        let mut overlay_sandbox: Option<String> = None;
        let mut overlay_effort: Option<String> = None;
        let mut overlay_model: Option<String> = None;
        let mut overlay_compact_limit: Option<usize> = None;
        let mut overlay_mode: Option<devo_protocol::CollaborationMode> = None;
        let mut overlay_python_cell_first_wait_ms: Option<u64> = None;
        let mut overlay_auto_refine_enabled: Option<bool> = None;
        let mut overlay_auto_refine_turn_interval: Option<u32> = None;
        let mut applied_window: Option<u64> = None;
        let mut settings_changes = Vec::new();
        let live_permission_profile = if let Some(handle) = session_handle.as_ref() {
            native_permission_profile(
                handle
                    .summary()
                    .await
                    .and_then(|summary| summary.permission_preset()),
            )
        } else {
            index_session
                .as_ref()
                .map(|session| session.settings.permission_profile)
                .unwrap_or(devo_protocol::native::model::PermissionProfile::AutoReview)
        };
        if let Some(settings) = &params.settings {
            if let Some(profile) = settings.permission_profile
                && profile != live_permission_profile
            {
                let preset = match profile {
                    devo_protocol::native::model::PermissionProfile::Default => {
                        devo_protocol::PermissionPreset::Default
                    }
                    devo_protocol::native::model::PermissionProfile::AutoReview => {
                        devo_protocol::PermissionPreset::AutoReview
                    }
                    devo_protocol::native::model::PermissionProfile::FullAccess => {
                        devo_protocol::PermissionPreset::FullAccess
                    }
                };
                if rollout_path.is_some() {
                    settings_changes.push((
                        SessionSettingsField::PermissionPreset,
                        serde_json::to_value(preset).expect("serialize permission preset setting"),
                    ));
                }
                let profile = safety_profile_from_protocol(
                    preset,
                    session_cwd.clone(),
                    session_additional_dirs.clone(),
                );
                overlay_profile = Some(profile);
            }
            if settings.sandbox_profile != current.sandbox_profile
                && let Some(name) = &settings.sandbox_profile
            {
                let native_name = match crate::sandbox_profile::normalize_sandbox_profile_name(
                    name,
                    &session_cwd,
                ) {
                    Ok(name) => name,
                    Err(error) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InvalidParams,
                            format!("invalid sandbox profile '{name}': {error}"),
                        );
                    }
                };
                if rollout_path.is_some() {
                    settings_changes.push((
                        SessionSettingsField::SandboxProfile,
                        serde_json::Value::String(native_name.clone()),
                    ));
                }
                overlay_sandbox = Some(native_name);
            }
            let current_effort = current.reasoning_effort.clone();
            if let Some(effort) = &settings.reasoning_effort {
                let requested_model = params
                    .model
                    .as_ref()
                    .map(native_model_slug)
                    .unwrap_or_else(|| session_model_slug.clone());
                let normalized = match self.deps.context_for_workspace(&session_cwd).await {
                    Ok(runtime_context) => runtime_context
                        .resolve_turn_config(Some(&requested_model), Some(effort.clone()))
                        .reasoning_effort_selection
                        .unwrap_or_else(|| {
                            devo_protocol::normalize_reasoning_effort_literal(effort)
                        }),
                    Err(_) => self
                        .deps
                        .model_catalog
                        .get(&requested_model)
                        .and_then(|model| model.normalize_reasoning_effort_selection(Some(effort)))
                        .unwrap_or_else(|| {
                            devo_protocol::normalize_reasoning_effort_literal(effort)
                        }),
                };
                if current_effort.as_ref() != Some(&normalized) {
                    if rollout_path.is_some() {
                        settings_changes.push((
                            SessionSettingsField::ReasoningEffortSelection,
                            serde_json::to_value(Some(normalized.clone()))
                                .expect("serialize reasoning effort setting"),
                        ));
                    }
                    overlay_effort = Some(normalized);
                }
            }
            if settings.mode != current.mode
                && let Some(mode_id) = &settings.mode
            {
                let mode = match serde_json::from_value::<devo_protocol::CollaborationMode>(
                    serde_json::Value::String(mode_id.clone()),
                ) {
                    Ok(mode) => mode,
                    Err(_) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InvalidParams,
                            format!("invalid collaboration mode '{mode_id}'"),
                        );
                    }
                };
                if rollout_path.is_some() {
                    settings_changes.push((
                        SessionSettingsField::CollaborationMode,
                        serde_json::to_value(mode).expect("serialize collaboration mode setting"),
                    ));
                }
                overlay_mode = Some(mode);
            }
            if settings.effective_context_window != current.effective_context_window
                && settings.effective_context_window.is_some()
            {
                // Product: auto-compact threshold is removed. Ignore patches that
                // try to set a global/session absolute limit; echo the model
                // effective window for older clients.
                let workspace_catalog = self
                    .deps
                    .context_for_workspace(&session_cwd)
                    .await
                    .ok()
                    .map(|context| Arc::clone(&context.model_catalog));
                let model = workspace_catalog
                    .as_ref()
                    .and_then(|catalog| catalog.get(&session_model_slug).cloned())
                    .or_else(|| self.deps.model_catalog.get(&session_model_slug).cloned());
                if let Some(model) = model {
                    let applied =
                        crate::runtime::context_occupancy::resolved_compaction_limit(&model);
                    if let Some(handle) = session_handle.as_ref() {
                        handle.notify_effective_context_window(applied as usize);
                    }
                    overlay_compact_limit = Some(applied as usize);
                    applied_window = Some(applied);
                }
            }
            if let Some(enabled) = settings.auto_refine_enabled {
                current.auto_refine_enabled = Some(enabled);
                overlay_auto_refine_enabled = Some(enabled);
                if rollout_path.is_some() {
                    settings_changes.push((
                        SessionSettingsField::AutoRefineEnabled,
                        serde_json::Value::Bool(enabled),
                    ));
                }
            }
            if let Some(interval) = settings.auto_refine_turn_interval {
                let interval = interval.max(1);
                current.auto_refine_turn_interval = Some(interval);
                overlay_auto_refine_turn_interval = Some(interval);
                if rollout_path.is_some() {
                    settings_changes.push((
                        SessionSettingsField::AutoRefineTurnInterval,
                        serde_json::json!(interval),
                    ));
                }
            }
            if let Some(ms) = settings.python_cell_first_wait_ms {
                current.python_cell_first_wait_ms = Some(ms);
                if rollout_path.is_some() {
                    settings_changes.push((
                        SessionSettingsField::PythonCellFirstWaitMs,
                        serde_json::json!(ms),
                    ));
                }
                overlay_python_cell_first_wait_ms = Some(ms);
            }
        }
        if let Some(binding) = &params.model {
            let mut model_selection = native_model_slug(binding);
            if let Some(variant) = binding.variant.as_deref()
                && !model_selection.ends_with(&format!("/{variant}"))
            {
                model_selection = format!("{model_selection}/{variant}");
            }
            if model_selection != session_model_slug {
                if rollout_path.is_some() {
                    settings_changes.push((
                        SessionSettingsField::Model,
                        serde_json::to_value(Some(model_selection.clone()))
                            .expect("serialize model setting"),
                    ));
                }
                overlay_model = Some(model_selection);
            }
        }

        // A model slug and its provider binding are one logical selection. A
        // slug-only update must clear the previous binding; otherwise the
        // next turn's binding-first resolution keeps selecting the old model.
        // An explicitly supplied binding remains authoritative when both are
        // present in the same update.
        let model_binding_update = if overlay_model.is_some() {
            Some(params.model_binding_id.clone())
        } else {
            params.model_binding_id.clone().map(Some)
        };
        if let Some(model_binding_id) = &model_binding_update
            && rollout_path.is_some()
        {
            settings_changes.push((
                SessionSettingsField::ModelBindingId,
                serde_json::to_value(model_binding_id).expect("serialize model binding setting"),
            ));
        }
        if let Some(path) = rollout_path.as_ref()
            && !settings_changes.is_empty()
            && let Err(error) = self.rollout_store.append_session_settings_batch_at(
                path,
                legacy_session_id,
                session_version,
                &settings_changes,
            )
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist session settings: {error}"),
            );
        }
        // Mirror session model / effort into user defaults (config.toml).
        // Provider is not a separate preference — it is the `provider/` prefix
        // of the model slug.
        if overlay_model.is_some() || overlay_effort.is_some() {
            {
                let mut store = self
                    .deps
                    .config_store
                    .lock()
                    .expect("app config store mutex should not be poisoned");
                if let Some(model) = overlay_model.as_ref()
                    && let Err(error) = store.set_model_config_option("model", model)
                {
                    tracing::warn!(%error, model = %model, "failed to persist global default model");
                }
                if let Some(effort) = overlay_effort.as_ref()
                    && let Err(error) = store.set_model_config_option("thought_level", effort)
                {
                    tracing::warn!(%error, effort = %effort, "failed to persist global default effort");
                }
            }
            self.deps.invalidate_workspace_contexts();
        }
        if let Some(handle) = session_handle.as_ref() {
            if let Some(profile) = &overlay_profile {
                handle.notify_permission_profile(profile.clone());
            }
            if let Some(name) = &overlay_sandbox {
                handle.notify_sandbox_profile(name.clone());
            }
        }
        // Auto-refine / python-wait settings are durable above, but the live
        // actor's native session must also observe them — the post-turn
        // auto-refine interval check and next-turn python wait read the
        // actor snapshot, not the rollout.
        if (overlay_auto_refine_enabled.is_some()
            || overlay_auto_refine_turn_interval.is_some()
            || overlay_python_cell_first_wait_ms.is_some())
            && let Some(handle) = session_handle.as_ref()
        {
            handle.notify_native_session_settings(
                overlay_auto_refine_enabled,
                overlay_auto_refine_turn_interval,
                overlay_python_cell_first_wait_ms,
            );
        }
        // One consolidated metadata notification carrying every field's new
        // or current value: the actor overwrites absent fields on non-
        // mode-only updates, so partial notifications would wipe them.
        if (overlay_model.is_some()
            || overlay_effort.is_some()
            || overlay_mode.is_some()
            || model_binding_update.is_some())
            && let Some(handle) = session_handle.as_ref()
        {
            handle.notify_session_model_settings(
                Some(
                    overlay_model
                        .clone()
                        .or(current_model_slug)
                        .unwrap_or_else(|| session_model_slug.clone()),
                ),
                model_binding_update
                    .clone()
                    .unwrap_or_else(|| current_binding_id.clone()),
                overlay_effort.clone().or(current_effort),
                overlay_mode,
            );
        }

        // Phase 3: deliver the override to the running turn's inline state,
        // if a turn is active. Admission reads the inline config on every
        // authorization and the tool router reads the live sandbox handle on
        // every spawn, so the change applies at the next decision point.
        let mut applied_to_active_turn = false;
        if (overlay_profile.is_some()
            || overlay_sandbox.is_some()
            || overlay_effort.is_some()
            || overlay_model.is_some()
            || overlay_compact_limit.is_some()
            || overlay_python_cell_first_wait_ms.is_some())
            && let Some(stream) = self.active_stream_state(legacy_session_id).await
        {
            let mut stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_mut() {
                if let Some(profile) = &overlay_profile {
                    inline.hook_context.config.permission_mode = profile.permission_mode();
                    inline.hook_context.config.permission_profile = profile.clone();
                    let implied = Some(profile.implied_sandbox_profile().to_string());
                    inline.hook_context.config.sandbox_profile = implied.clone();
                    *inline
                        .sandbox_profile_live
                        .lock()
                        .expect("sandbox profile live mutex poisoned") = implied;
                    // A new policy invalidates implicit cached approvals,
                    // matching the actor-side behavior for idle updates.
                    inline.session_approval_cache = Default::default();
                    inline.turn_approval_cache = Default::default();
                }
                if let Some(name) = &overlay_sandbox {
                    inline.hook_context.config.sandbox_profile = Some(name.clone());
                    *inline
                        .sandbox_profile_live
                        .lock()
                        .expect("sandbox profile live mutex poisoned") = Some(name.clone());
                }
                // Phase 4: model/effort changes replace the live turn config
                // (re-resolved so provider routing follows the new model);
                // compaction-limit changes move the next budget check.
                if overlay_effort.is_some()
                    || overlay_model.is_some()
                    || overlay_compact_limit.is_some()
                {
                    let mut live = inline
                        .live_turn_settings
                        .lock()
                        .expect("live settings mutex poisoned");
                    if let Some(model_slug) = &overlay_model {
                        let effort = overlay_effort.clone().or_else(|| {
                            live.turn_config
                                .as_ref()
                                .and_then(|config| config.reasoning_effort_selection.clone())
                        });
                        live.turn_config = Some(
                            inline
                                .hook_context
                                .runtime_context
                                .resolve_turn_config(Some(model_slug), effort),
                        );
                    } else if let Some(effort) = &overlay_effort {
                        // Base the override on the seeded live config; if the
                        // turn runner has not seeded yet, resolve a fresh base
                        // from the session's current model so the overlay is
                        // never silently dropped.
                        let base = live.turn_config.clone().unwrap_or_else(|| {
                            inline
                                .hook_context
                                .runtime_context
                                .resolve_turn_config(inline.summary.model_name(), None)
                        });
                        let mut config = base;
                        config.reasoning_effort_selection = Some(effort.clone());
                        live.turn_config = Some(config);
                    }
                    if let Some(limit) = overlay_compact_limit {
                        live.auto_compact_token_limit = Some(limit);
                    }
                    if let Some(ms) = overlay_python_cell_first_wait_ms {
                        live.python_cell_first_wait_ms = Some(ms);
                    }
                    live.generation = live.generation.saturating_add(1);
                } else if let Some(ms) = overlay_python_cell_first_wait_ms {
                    let mut live = inline
                        .live_turn_settings
                        .lock()
                        .expect("live settings mutex poisoned");
                    live.python_cell_first_wait_ms = Some(ms);
                    live.generation = live.generation.saturating_add(1);
                }
                applied_to_active_turn = true;
            }
        }

        // Keep the SQLite session index in step with the settings write so
        // the session list reflects new values without waiting for turn
        // activity. Built from index metadata + the applied patch values
        // (no actor round-trip).
        if let Some(index_session) = index_session.as_mut() {
            let mut touched = false;
            if let Some(profile) = &overlay_profile {
                index_session.settings.permission_profile =
                    native_permission_profile(Some(protocol_preset_from_safety(profile.preset)));
                touched = true;
            }
            if let Some(model) = &overlay_model {
                index_session.model.model = model.clone();
                touched = true;
            }
            if let Some(binding_id) = model_binding_update.as_ref() {
                index_session.model.provider =
                    binding_id.clone().unwrap_or_else(|| "unknown".into());
                touched = true;
            }
            if let Some(effort) = &overlay_effort {
                index_session.settings.reasoning_effort = Some(effort.clone());
                touched = true;
            }
            if let Some(mode) = overlay_mode {
                index_session.settings.mode = serde_json::to_value(mode)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string));
                touched = true;
            }
            if touched {
                index_session.usage.updated_at = Utc::now();
                if let Err(error) = self.deps.db.upsert_session(&*index_session, None) {
                    tracing::warn!(
                        session_id = %legacy_session_id,
                        error = %error,
                        "failed to refresh session index after settings update"
                    );
                }
            }
        }

        // The response reflects the persisted state: for durable sessions
        // it is rebuilt from the rollout (field lines fold into the canonical
        // snapshot); for ephemeral sessions it is built from the SQLite
        // index with the applied patch.
        let mut session = if let Some(path) = rollout_path.as_ref()
            && !settings_changes.is_empty()
            && let Ok(history) = devo_core::read_canonical_history(path)
            && let Some(persisted) = history.session
        {
            // Post-write re-read so `version` includes the settings-epoch bump
            // (L2-DES-CONV-002). Returning the pre-write snapshot leaves clients
            // on a stale expectedVersion and the next metadata/update fails.
            *persisted
        } else if let Some(mut session) = durable_rollout_session {
            if let Some(profile) = &overlay_profile {
                session.settings.permission_profile =
                    native_permission_profile(Some(protocol_preset_from_safety(profile.preset)));
            }
            if let Some(name) = &overlay_sandbox {
                session.settings.sandbox_profile = Some(name.clone());
            }
            if let Some(model) = &overlay_model {
                session.model.model = model.clone();
            }
            if let Some(binding_id) = model_binding_update.as_ref() {
                session.model.provider = binding_id.clone().unwrap_or_else(|| "unknown".into());
            }
            if let Some(effort) = &overlay_effort {
                session.settings.reasoning_effort = Some(effort.clone());
            }
            if let Some(mode) = overlay_mode {
                session.settings.mode = serde_json::to_value(mode)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string));
            }
            *session
        } else {
            let Some(index_session) = index_session.as_ref() else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionNotFound,
                    "session is not durable and has no index metadata",
                );
            };
            index_session.clone()
        };
        // Echo applied model effective window for older clients. Do not fan out
        // a global compaction preference — that product surface is removed.
        if let Some(applied) = applied_window {
            session.settings.effective_context_window = Some(applied);
        }
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionMetadataUpdateResult {
                session,
                applied_to_active_turn,
            },
        })
        .expect("serialize canonical session/metadata/update response")
    }

    /// Native `session/new` (L2-DES-APP-008 Phase B): creates a durable
    /// session in `cwd` with idempotency-key replay, returning the canonical
    /// session snapshot built from the rollout (single source of truth).
    pub(crate) async fn handle_native_session_new(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionNewParams =
            match self.parse_rpc_params(request_id.clone(), params, "session/new") {
                Ok(params) => params,
                Err(error) => return error,
            };
        if let Some(existing) = self
            .session_new_idempotency
            .lock()
            .await
            .get(&params.idempotency_key)
            .cloned()
            && let Some(result) = self
                .native_session_snapshot_response(request_id.clone(), existing)
                .await
        {
            return result;
        }
        let response = self
            .start_session_with_registry(
                connection_id,
                request_id.clone(),
                SessionStartParams {
                    cwd: params.cwd.clone(),
                    additional_directories: Vec::new(),
                    ephemeral: false,
                    title: None,
                    model: None,
                    model_binding_id: None,
                },
                None,
            )
            .await;
        if let Ok(success) =
            serde_json::from_value::<SuccessResponse<SessionStartResult>>(response.clone())
        {
            let session_id = SessionId::from(success.result.session.id.as_str());
            self.subscribe_connection_to_session(connection_id, session_id, None)
                .await;
            self.session_new_idempotency
                .lock()
                .await
                .insert(params.idempotency_key, session_id);
            if let Some(result) = self
                .native_session_snapshot_response(request_id, session_id)
                .await
            {
                return result;
            }
        }
        response
    }

    /// Builds a `session/new`-shaped response from the rollout-backed
    /// canonical session snapshot; `None` when the rollout is not readable.
    async fn native_session_snapshot_response(
        &self,
        request_id: serde_json::Value,
        session_id: SessionId,
    ) -> Option<serde_json::Value> {
        let session = self.native_session_snapshot(session_id).await?;
        Some(
            serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_session::SessionNewResult { session },
            })
            .expect("serialize canonical session/new response"),
        )
    }

    fn indexed_rollout_path(&self, session_id: SessionId) -> Option<std::path::PathBuf> {
        self.deps
            .db
            .get_session_index(&session_id)
            .ok()
            .flatten()
            .and_then(|index| index.rollout_path)
    }

    fn resolve_existing_rollout_path(
        &self,
        session_id: SessionId,
        indexed_rollout_path: Option<std::path::PathBuf>,
    ) -> Result<Option<std::path::PathBuf>, String> {
        Ok(match indexed_rollout_path.filter(|path| path.exists()) {
            Some(path) => Some(path),
            _ => self
                .rollout_store
                .find_rollout_by_session_id(&session_id)
                .map_err(|error| format!("failed to locate session rollout: {error}"))?
                .filter(|path| path.exists()),
        })
    }

    /// Reads the rollout-backed canonical session snapshot; `None` when the
    /// rollout is missing or unreadable.
    pub(crate) async fn native_session_snapshot(
        &self,
        session_id: SessionId,
    ) -> Option<devo_protocol::native::session::Session> {
        let rollout_path = self
            .resolve_existing_rollout_path(session_id, self.indexed_rollout_path(session_id))
            .ok()
            .flatten();
        // Only the metadata header is needed here; the filtered meta reader
        // skips the typed parse of every non-meta line, which dominates a
        // full canonical-history read on large journals.
        let history_session = rollout_path.as_ref().and_then(|path| {
            devo_core::read_rollout_session_meta(path)
                .ok()?
                .map(|session| *session)
        });
        if let Some(handle) = self.session(session_id).await
            && let Some(mut session) = handle.native_session().await
        {
            // Persist-first settings writes bump the rollout settings epoch
            // before the actor observes them. Prefer the durable version so
            // session/read refresh does not under-report expectedVersion.
            if let Some(history) = history_session.as_ref() {
                session.version = session.version.max(history.version);
            }
            return Some(session);
        }
        let mut session = history_session?;
        if let Some(model) = self.deps.model_catalog.get(&session.model.model) {
            session.settings.effective_context_window =
                Some(crate::runtime::context_occupancy::resolved_compaction_limit(model));
        }
        Some(session)
    }

    /// Overlays live runtime pointers onto a durable session snapshot.
    ///
    /// Rollout / index snapshots almost always report `Idle`; in-flight turns
    /// live in `ActiveTurnRegistry`. List/read must project that truth so
    /// clients (e.g. delete-refill) do not treat a working session as idle.
    async fn apply_live_session_runtime_fields(
        &self,
        session_id: SessionId,
        session: &mut devo_protocol::native::session::Session,
    ) {
        match self.active_turns.active_turn(session_id).await {
            Some(turn) => {
                session.status = devo_protocol::native::session::SessionStatus::Active;
                session.active_turn_id = Some(turn.id);
            }
            None => {
                session.status = devo_protocol::native::session::SessionStatus::Idle;
                session.active_turn_id = None;
            }
        }
        session.sync_activity();
    }

    /// Native `session/read` (L2-DES-APP-008): one session's
    /// rollout-backed canonical snapshot.
    pub(crate) async fn handle_native_session_read(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionReadParams =
            match self.parse_rpc_params(request_id.clone(), params, "session/read") {
                Ok(params) => params,
                Err(error) => return error,
            };
        let session_id = params.session_id;
        let Some(mut session) = self.native_session_snapshot(session_id).await else {
            return self.session_missing(request_id);
        };
        self.apply_live_session_runtime_fields(session_id, &mut session)
            .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionReadResult { session },
        })
        .expect("serialize canonical session/read response")
    }

    /// Native `session/systemPrompt/read`: exact system prompt the server would
    /// send on the next model call for this session.
    pub(crate) async fn handle_native_session_system_prompt_read(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionSystemPromptReadParams =
            match self.parse_rpc_params(request_id.clone(), params, "session/systemPrompt/read") {
                Ok(params) => params,
                Err(error) => return error,
            };
        let session_id = params.session_id;
        let session_handle = match self
            .runtime_arc()
            .get_or_load_parent_session(session_id)
            .await
        {
            Ok(handle) => handle,
            Err(_) => return self.session_missing(request_id),
        };
        let Some(runtime) = session_handle.export_runtime_session().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                "failed to read session state for system prompt".to_string(),
            );
        };
        let prompt = {
            let core = runtime.core_session.lock().await;
            if let Some(context) = core.session_context.as_ref() {
                context.build_system_prompt(core.collaboration_mode)
            } else {
                let summary = session_handle.summary().await;
                let cwd = summary
                    .as_ref()
                    .map(|s| s.cwd.clone())
                    .unwrap_or_else(|| core.cwd.clone());
                let Some(model) = summary
                    .as_ref()
                    .and_then(|s| s.model_name())
                    .and_then(|slug| self.deps.model_catalog.get(slug))
                    .or_else(|| {
                        summary
                            .as_ref()
                            .and_then(|s| s.model_binding_id())
                            .and_then(|binding| self.deps.model_catalog.get(binding))
                    })
                    .cloned()
                    .or_else(|| {
                        core.latest_turn_context
                            .as_ref()
                            .map(|ctx| ctx.model.clone())
                    })
                else {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        "session has no model binding to build a system prompt".to_string(),
                    );
                };
                let agents_md_manager =
                    devo_core::AgentsMdManager::new(core.config.agents_md.clone());
                let locked_agents_snapshot =
                    devo_core::load_workspace_instructions(&cwd, &agents_md_manager);
                let reasoning = summary
                    .as_ref()
                    .and_then(|s| s.native.settings.reasoning_effort.clone());
                let context = devo_core::SessionContext::capture(
                    &model,
                    reasoning.as_deref(),
                    &cwd,
                    locked_agents_snapshot,
                    core.config.available_skills_instructions.clone(),
                );
                context.build_system_prompt(core.collaboration_mode)
            }
        };
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionSystemPromptReadResult { prompt },
        })
        .expect("serialize session/systemPrompt/read response")
    }

    /// Native `session/list` (L2-DES-APP-008): offset-paged canonical
    /// session snapshots, newest activity first. Sessions whose rollout
    /// snapshot is unreadable are skipped.
    pub(crate) async fn handle_native_session_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionListParams =
            match self.parse_rpc_params(request_id.clone(), params, "session/list") {
                Ok(params) => params,
                Err(error) => return error,
            };
        let start: usize = match params.cursor.as_deref() {
            None => 0,
            Some(cursor) => match cursor.parse() {
                Ok(start) => start,
                Err(_) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/list cursor: {cursor}"),
                    );
                }
            },
        };
        let limit = params
            .limit
            .map_or(CANONICAL_SESSION_LIST_DEFAULT_LIMIT, |limit| limit as usize);
        let search = params.search.as_deref().map(str::to_lowercase);
        let include_children = params.include_children.unwrap_or(false);
        let mut sessions = Vec::new();
        for mut session in self
            .list_native_sessions_with_options(include_children)
            .await
        {
            if !session_list_cwd_matches(&params.cwds, &session.cwd) {
                continue;
            }
            if let Some(search) = search.as_ref()
                && !session
                    .title
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(search)
            {
                continue;
            }
            if session.summary.is_none() && !session.preview.is_empty() {
                session.summary = Some(session.preview.clone());
            }
            if session.task_state.is_none() {
                session.task_state = Some(match session.activity {
                    devo_protocol::native::session::SessionActivity::Working => {
                        "working".to_string()
                    }
                    devo_protocol::native::session::SessionActivity::Idle => "idle".to_string(),
                });
            }
            sessions.push(session);
        }
        let next_start = start.saturating_add(limit);
        let next_cursor = (next_start < sessions.len()).then(|| next_start.to_string());
        // Paginate before enrichment so list cost scales with page size, not
        // total indexed sessions (stress corpora / large Agents View).
        let mut data: Vec<_> = sessions.into_iter().skip(start).take(limit).collect();
        let active_turn_ids = self.active_turns.active_turn_ids().await;
        let rollout_paths = self.deps.db.list_rollout_paths().unwrap_or_default();
        let page_ids = data.iter().map(|session| session.id).collect::<Vec<_>>();
        let turn_counts = self.deps.db.list_turn_counts(&page_ids).unwrap_or_default();
        for session in &mut data {
            let session_id = session.id;
            if let Some(turn_id) = active_turn_ids.get(&session_id).copied() {
                session.status = devo_protocol::native::session::SessionStatus::Active;
                session.active_turn_id = Some(turn_id);
            } else {
                session.status = devo_protocol::native::session::SessionStatus::Idle;
                session.active_turn_id = None;
            }
            session.sync_activity();
            // Indexed path only — no suffix walk. Metadata is best-effort; a
            // missing file leaves the list-cache field unset.
            if let Some(path) = rollout_paths.get(&session_id) {
                session.transcript_size_bytes =
                    std::fs::metadata(path).ok().map(|metadata| metadata.len());
            }
            if session.message_count.is_none() {
                session.message_count = turn_counts.get(&session_id).copied();
            }
            session.task_state = Some(match session.activity {
                devo_protocol::native::session::SessionActivity::Working => "working".to_string(),
                devo_protocol::native::session::SessionActivity::Idle => "idle".to_string(),
            });
        }
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::page::Page { data, next_cursor },
        })
        .expect("serialize canonical session/list response")
    }

    /// Native `session/delete` (L2-DES-APP-008): deletes the session tree
    /// and broadcasts the session-deleted event, same side effects as the
    /// ACP adapter path.
    pub(crate) async fn handle_native_session_delete(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionDeleteParams =
            match self.parse_rpc_params(request_id.clone(), params, "session/delete") {
                Ok(params) => params,
                Err(error) => return error,
            };
        let session_id = params.session_id;
        let loaded_session_ids: Vec<SessionId> =
            self.sessions.lock().await.keys().cloned().collect();
        let mut native_session_ids = std::collections::HashMap::new();
        for legacy_id in loaded_session_ids {
            if let Some(handle) = self.session(legacy_id).await
                && let Some(native_session) = handle.native_session().await
            {
                native_session_ids.insert(legacy_id, native_session.id);
            }
        }
        if let Some(snapshot) = self.native_session_snapshot(session_id).await {
            native_session_ids.insert(session_id, snapshot.id);
        }
        let deleted_session_ids = match self.delete_session_tree(session_id).await {
            Ok(deleted_session_ids) => deleted_session_ids,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to delete session: {error}"),
                );
            }
        };
        if !deleted_session_ids.is_empty() {
            let native_session_id = native_session_ids.get(&session_id).cloned().unwrap_or({
                // boundary: session summary unavailable before delete
                session_id
            });
            self.broadcast_notification(
                devo_protocol::native::event::ServerNotification::SessionDeleted {
                    session_id: native_session_id,
                    deleted_session_ids: deleted_session_ids
                        .iter()
                        .map(|id| {
                            native_session_ids.get(id).cloned().unwrap_or({
                                // boundary: child session summary unavailable before delete
                                *id
                            })
                        })
                        .collect(),
                },
            )
            .await;
        }
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionDeleteResult {},
        })
        .expect("serialize canonical session/delete response")
    }

    /// Native `session/resume` (L2-DES-APP-008 Phase B): hydrates the
    /// session actor and answers with the rollout-backed
    /// canonical session snapshot. Transcript restore is intentionally not
    /// part of this result — canonical clients page `session/items/list` or
    /// use `subscription/*` snapshots (Phase C rework of the TUI restore
    /// flow).
    pub(crate) async fn handle_native_session_resume(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionResumeParams =
            match self.parse_rpc_params(request_id.clone(), params, "session/resume") {
                Ok(params) => params,
                Err(error) => return error,
            };
        let legacy_session_id = params.session_id;
        let response = self
            .restore_existing_session_with_tool_registry_update(
                connection_id,
                request_id.clone(),
                SessionResumeParams {
                    session_id: legacy_session_id,
                },
                RuntimeSessionToolRegistryUpdate::KeepCurrent,
            )
            .await;
        if response.get("error").is_some() {
            return response;
        }
        // Children hydrate before the reissue so a child approval parked at
        // a checkpoint when the process died re-registers its lane and gets
        // re-delivered to this client (lazy hydration would leave it stuck).
        self.runtime_arc()
            .hydrate_child_sessions_for_approval_recovery(params.session_id)
            .await;
        self.runtime_arc()
            .reissue_pending_controls_if_subscribed(connection_id, &params.session_id)
            .await;
        self.native_session_resume_response(request_id, legacy_session_id)
            .await
            .unwrap_or(response)
    }

    async fn native_session_resume_response(
        &self,
        request_id: serde_json::Value,
        session_id: SessionId,
    ) -> Option<serde_json::Value> {
        let session = self.native_session_snapshot(session_id).await?;
        let stats = self.deps.db.get_stats(&session_id).ok().flatten();
        // Occupancy preference: persisted stats first, then the resident
        // actor summary, and only then the rollout — the rollout scan is a
        // full-file parse that dominates resume on large journals and is
        // pure waste whenever either in-memory source already has a value.
        let mut last_context_occupancy = stats
            .as_ref()
            .and_then(|stats| stats.last_context_occupancy.clone());
        if last_context_occupancy.is_none() {
            last_context_occupancy = self
                .session_summary_snapshot(session_id)
                .await
                .and_then(|summary| summary.last_context_occupancy);
        }
        if last_context_occupancy.is_none() {
            last_context_occupancy = self.native_rollout_context_occupancy(session_id).await;
        }
        let last_query_total_tokens = last_context_occupancy
            .as_ref()
            .map(|occupancy| occupancy.total_tokens)
            .filter(|tokens| *tokens > 0)
            .or_else(|| {
                stats
                    .as_ref()
                    .map(|stats| stats.prompt_token_estimate as u64)
                    .filter(|tokens| *tokens > 0)
            });
        Some(
            serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_session::SessionResumeResult {
                    recovery: self.turn_recovery(session_id).await.ok().flatten(),
                    session,
                    last_context_occupancy,
                    last_query_total_tokens,
                },
            })
            .expect("serialize canonical session/resume response"),
        )
    }

    async fn native_rollout_context_occupancy(
        &self,
        session_id: SessionId,
    ) -> Option<devo_protocol::native::item::ContextOccupancy> {
        let rollout_path = self
            .resolve_existing_rollout_path(session_id, self.indexed_rollout_path(session_id))
            .ok()
            .flatten()?;
        devo_core::read_canonical_history(&rollout_path)
            .ok()
            .and_then(|history| history.latest_context_occupancy)
    }

    pub(crate) async fn restore_existing_session_with_tool_registry_update(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: SessionResumeParams,
        tool_registry_update: RuntimeSessionToolRegistryUpdate,
    ) -> serde_json::Value {
        let session_handle = match self
            .runtime_arc()
            .get_or_load_parent_session(params.session_id)
            .await
        {
            Ok(handle) => handle,
            Err(crate::runtime::session_cache::LoadSessionError::SessionNotFound) => {
                return self.session_missing(request_id);
            }
            Err(crate::runtime::session_cache::LoadSessionError::RolloutMissing) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    "session metadata exists but rollout file is missing; session cannot be restored",
                );
            }
            Err(crate::runtime::session_cache::LoadSessionError::SessionLocked { holder_pid }) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionLocked,
                    format!(
                        "session is already open in another Devo process (pid {holder_pid}); \
                         close it there before resuming it here — running the same session in \
                         two processes corrupts its history"
                    ),
                );
            }
            Err(crate::runtime::session_cache::LoadSessionError::RestoreFailed(message)) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to restore session: {message}"),
                );
            }
        };
        let _state_change_guard = session_handle.lock_state_change().await;
        match tool_registry_update {
            RuntimeSessionToolRegistryUpdate::KeepCurrent => {}
            RuntimeSessionToolRegistryUpdate::ReplaceIfCwdMatches { cwd, tool_registry } => {
                let summary = session_handle.summary().await;
                if summary.as_ref().is_none_or(|summary| summary.cwd != cwd) {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        "session cwd does not match the stored session cwd",
                    );
                }
                if !session_handle.set_tool_registry(tool_registry).await {
                    return self.session_missing(request_id);
                }
            }
        }
        let Some(resume_snapshot) = session_handle.resume_snapshot().await else {
            return self.session_missing(request_id);
        };
        let session = resume_snapshot.summary.native;
        let latest_turn = resume_snapshot.latest_turn.map(|turn| turn.native);
        let loaded_item_count = resume_snapshot.loaded_item_count;
        let history_items = resume_snapshot.history_items;
        let pending_texts = resume_snapshot.pending_texts;
        self.subscribe_connection_to_session(connection_id, params.session_id, None)
            .await;
        self.run_session_hook(
            params.session_id,
            devo_core::HookEvent::SessionStart,
            serde_json::Map::from_iter([("source".to_string(), serde_json::json!("resume"))]),
        )
        .await;
        tracing::info!(
            connection_id,
            session_id = %params.session_id,
            loaded_item_count,
            has_latest_turn = latest_turn.is_some(),
            pending_count = pending_texts.len(),
            "resumed session"
        );
        self.runtime_arc()
            .resume_pending_queue_if_idle(params.session_id)
            .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionResumeResult {
                session,
                latest_turn,
                loaded_item_count,
                history_items,
                pending_texts,
            },
        })
        .expect("serialize session/resume response")
    }

    /// Native `session/fork` (L2-DES-APP-008 Phase B): forks at
    /// `atTurnId` (or the session tip when absent). The turn id is mapped to
    /// the legacy user-turn index with the same rule the fork machinery
    /// uses (turns containing a `UserMessage` item, in order).
    pub(crate) async fn handle_native_session_fork(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionForkParams =
            match self.parse_rpc_params(request_id.clone(), params, "session/fork") {
                Ok(params) => params,
                Err(error) => return error,
            };
        let legacy_session_id = params.session_id;
        let cut = params
            .cut
            .unwrap_or(devo_protocol::native::rpc_session::SessionForkCut::Through);
        let fork_at_turn_id = params.at_turn_id;

        // Tip fork while a turn is running: interrupt first so we copy only
        // completed history (Codex tip-fork semantics).
        if fork_at_turn_id.is_none()
            && self
                .runtime_active_turn_id(legacy_session_id)
                .await
                .is_some()
        {
            self.await_session_turn_interrupt_before_delete(legacy_session_id)
                .await;
        }

        let source = match self
            .exported_runtime_session(request_id.clone(), legacy_session_id)
            .await
        {
            Ok(source) => source,
            Err(error) => return error,
        };

        if let Some(legacy_turn_id) = fork_at_turn_id
            && self.runtime_active_turn_id(legacy_session_id).await == Some(legacy_turn_id)
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ForkTurnNotStable,
                "atTurnId names an in-progress turn",
            );
        }

        let user_turn_index = match fork_at_turn_id {
            None => None,
            Some(legacy_turn_id) => {
                let user_turn_ids = user_turn_ids_from_items(&source.persisted_turn_items);
                let Some(index) = user_turn_ids
                    .iter()
                    .position(|turn_id| *turn_id == legacy_turn_id)
                else {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::ForkTurnNotFound,
                        "atTurnId does not name a user turn in this session",
                    );
                };
                Some(u32::try_from(index).unwrap_or(u32::MAX))
            }
        };

        let forked_runtime = match self
            .create_durable_user_fork(
                &source,
                super::session_fork::DurableForkOptions {
                    source_session_id: legacy_session_id,
                    fork_at_turn_id,
                    user_turn_index,
                    cut,
                    title_override: None,
                    cwd_override: None,
                },
            )
            .await
        {
            Ok(runtime) => runtime,
            Err(message) => {
                let code = if message.contains("selected turn") {
                    ProtocolErrorCode::InvalidParams
                } else {
                    ProtocolErrorCode::InternalError
                };
                return self.error_response(request_id, code, message);
            }
        };
        let forked_id = forked_runtime.summary.session_id();
        let summary = forked_runtime.summary.clone();
        let rollout_path_for_db = forked_runtime.rollout_path.clone();
        let forked_handle = self
            .insert_session_actor(SessionActorState::from_runtime_session(forked_runtime))
            .await;
        self.subscribe_connection_to_session(connection_id, forked_id, None)
            .await;
        self.runtime_arc()
            .after_root_session_insert(forked_id)
            .await;
        if !summary.ephemeral {
            if let Err(err) = self
                .deps
                .db
                .upsert_session(&summary, rollout_path_for_db.as_deref())
            {
                tracing::warn!(
                    session_id = %forked_id,
                    error = %err,
                    "failed to persist forked session metadata to database"
                );
            }
            let stats = crate::db::SessionStats {
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_tokens: 0,
                total_cache_creation_tokens: 0,
                total_cache_read_tokens: 0,
                last_input_tokens: 0,
                turn_count: 0,
                prompt_token_estimate: summary.prompt_token_estimate,
                last_context_occupancy: summary.last_context_occupancy.clone(),
            };
            if let Err(err) = self.deps.db.update_stats(&forked_id, &stats) {
                tracing::warn!(
                    session_id = %forked_id,
                    error = %err,
                    "failed to persist forked session token stats to database"
                );
            }
        }
        tracing::info!(
            connection_id,
            source_session_id = %legacy_session_id,
            forked_session_id = %forked_id,
            cwd = %summary.cwd.display(),
            ephemeral = summary.ephemeral,
            model = ?summary.model,
            "forked session"
        );
        if let Some(session) = forked_handle.native_session().await {
            self.broadcast_notification(
                devo_protocol::native::event::ServerNotification::SessionCreated {
                    session: Box::new(session),
                },
            )
            .await;
        }
        self.native_session_snapshot_response(request_id.clone(), forked_id)
            .await
            .unwrap_or_else(|| {
                serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: SessionForkResult {
                        session: summary.native.clone(),
                        forked_from_session_id: legacy_session_id,
                    },
                })
                .expect("serialize session/fork response")
            })
    }

    pub(crate) async fn build_runtime_session_from_user_turn_cut(
        &self,
        source: &RuntimeSession,
        options: RuntimeSessionTurnCutOptions,
    ) -> Result<RuntimeSession, String> {
        let RuntimeSessionTurnCutOptions {
            session_id,
            user_turn_index,
            rollback_mode,
            cwd_override,
            title_override,
            created_at,
        } = options;
        let source_core_session = source.core_session.lock().await;

        // Resolve the client-supplied index against the linear journal first —
        // that is the list interactive clients enumerate — then cut on the
        // active tree path so dead-branch items never leak into the copy.
        let selected_turn_id = match user_turn_index {
            None => None,
            Some(index) => {
                let user_turn_ids = user_turn_ids_from_items(&source.persisted_turn_items);
                let index = usize::try_from(index)
                    .map_err(|_| "selected turn index is invalid".to_string())?;
                Some(
                    user_turn_ids
                        .get(index)
                        .cloned()
                        .ok_or_else(|| "selected turn does not exist".to_string())?,
                )
            }
        };
        let path_scoped_items = source.rollout_path.as_deref().and_then(|rollout_path| {
            match devo_core::read_canonical_history(rollout_path) {
                Ok(history) => filter_items_to_active_path(
                    &source.persisted_turn_items,
                    &history,
                    ActivePathTip::JournalTip,
                ),
                Err(error) => {
                    tracing::warn!(
                        session_id = %source.summary.session_id(),
                        %error,
                        "fork/rollback active-path lookup failed; cutting linear history"
                    );
                    None
                }
            }
        });
        if let (Some(selected), Some(path_items)) = (&selected_turn_id, &path_scoped_items)
            && !path_items.iter().any(|item| item.turn_id == *selected)
        {
            return Err("selected turn is not on the active path".to_string());
        }
        let cut_source = path_scoped_items
            .as_ref()
            .unwrap_or(&source.persisted_turn_items);
        let kept_items = kept_items_for_user_turn_cut(cut_source, selected_turn_id, rollback_mode)?;

        let cwd = cwd_override.unwrap_or_else(|| source.summary.cwd.clone());
        let additional_directories = source.summary.additional_directories.clone();
        let runtime_context = if cwd == source.summary.cwd {
            Arc::clone(&source.runtime_context)
        } else {
            self.deps
                .context_for_workspace(&cwd)
                .await
                .map_err(|error| format!("failed to initialize session workspace: {error}"))?
        };
        let mut core_session = runtime_context.new_session_state(
            session_id,
            cwd.clone(),
            additional_directories.clone(),
        );
        core_session.config = source_core_session.config.clone();
        core_session.session_context = source_core_session.session_context.clone();
        core_session.collaboration_mode = source_core_session.collaboration_mode;
        core_session.latest_turn_context = None;
        // Fork/rollback starts a new cumulative ledger at the cut point.
        core_session.total_input_tokens = 0;
        core_session.total_output_tokens = 0;
        core_session.total_tokens = 0;
        core_session.total_cache_creation_tokens = 0;
        core_session.total_cache_read_tokens = 0;
        core_session.last_input_tokens = 0;
        core_session.last_turn_tokens = 0;

        let mut rebuilt_history_items = Vec::new();
        let mut rebuilt_messages = Vec::new();
        let mut tool_names_by_id = HashMap::new();
        for item in &kept_items {
            crate::prompt_from_native_item::apply_native_item(
                &mut rebuilt_messages,
                &mut rebuilt_history_items,
                &mut tool_names_by_id,
                item.item.clone(),
            );
        }
        core_session.messages = rebuilt_messages;
        core_session.turn_count = kept_items
            .iter()
            .filter(|item| crate::persisted_native_item::is_user_message(&item.item))
            .count();

        let last_kept_turn_id = kept_items.last().and_then(|item| item.legacy_turn_id());
        let kept_turn_ids: HashSet<_> = kept_items
            .iter()
            .filter_map(|item| item.legacy_turn_id())
            .collect();
        let (cut_occupancy, latest_query_usage, applicable_compaction) =
            resolve_cut_occupancy_and_usage(
                &kept_turn_ids,
                last_kept_turn_id,
                &source.turns_by_id,
                source.latest_compaction_snapshot.as_ref(),
            );
        // A cut that keeps the compaction turn keeps the compacted prompt:
        // the snapshot rebuild anchors its summary inside the kept items, so
        // forks and shallow rollbacks of compacted sessions stay compacted.
        // Cuts that drop the summary resolve to None and fall back to the
        // linear `messages` rebuilt above.
        core_session.prompt_messages = applicable_compaction.as_ref().and_then(|snapshot| {
            crate::persistence::build_prompt_messages_from_snapshot(&kept_items, snapshot)
        });
        let cut_turn = last_kept_turn_id
            .as_ref()
            .and_then(|turn_id| source.turns_by_id.get(turn_id));
        let prompt_token_estimate = cut_occupancy
            .as_ref()
            .map(|occupancy| occupancy.total_tokens as usize)
            .or_else(|| {
                latest_query_usage
                    .as_ref()
                    .map(|usage| usage.display_total_tokens() as usize)
            })
            .unwrap_or(0);
        core_session.prompt_token_estimate = prompt_token_estimate;

        let latest_turn = if let Some(last_turn_id) = last_kept_turn_id {
            if let Some(turn) = source
                .latest_turn
                .as_ref()
                .filter(|turn| turn.native.id == last_turn_id)
            {
                Some(turn.clone())
            } else if let Some(turn) = cut_turn.cloned() {
                let mut runtime = turn.into_runtime_turn();
                runtime.native.session_id = session_id;
                Some(runtime)
            } else {
                let model = source
                    .summary
                    .model_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| runtime_context.default_model.clone());
                // Synthetic fork metadata follows normal turn semantics:
                // `model` remains the catalog slug, while `request_model`
                // is recomputed from the active provider binding.
                let request_model = runtime_context
                    .resolve_turn_config(
                        source.summary.model_binding_id().or(Some(model.as_str())),
                        source.summary.settings.reasoning_effort.clone(),
                    )
                    .request_model;
                let sequence = kept_items
                    .iter()
                    .filter(|item| crate::persisted_native_item::is_user_message(&item.item))
                    .count() as u32;
                Some(crate::turn::RuntimeTurn {
                    native: devo_protocol::native::turn::Turn {
                        id: last_turn_id,
                        session_id,
                        sequence,
                        kind: devo_protocol::native::turn::TurnKind::Regular,
                        status: devo_protocol::native::turn::TurnStatus::Completed,
                        model: devo_protocol::native::model::ModelBinding {
                            provider: source
                                .summary
                                .model_binding_id()
                                .unwrap_or("unknown")
                                .to_string(),
                            model: request_model,
                            variant: None,
                            reasoning_effort: source.summary.model.reasoning_effort,
                        },
                        collaboration_mode: Some(source.summary.collaboration_mode),
                        started_at: source.summary.created_at,
                        completed_at: Some(source.summary.updated_at),
                        error: None,
                        usage: None,
                    },
                    extras: crate::turn::RuntimeTurnExtras {
                        request_thinking: source.summary.settings.reasoning_effort.clone(),
                        stop_reason: None,
                        failure_reason: None,
                    },
                })
            }
        } else {
            None
        };

        let updated_at = Utc::now();
        let mut native_session = source.summary.native.clone();
        native_session.id = session_id;
        native_session.version = 1;
        native_session.cwd = cwd.clone();
        native_session.additional_directories = additional_directories;
        native_session.parent = None;
        native_session.fork_from_id = None;
        native_session.at_turn_id = None;
        native_session.created_at = created_at;
        native_session.status = devo_protocol::native::session::SessionStatus::Idle;
        native_session.flags.clear();
        native_session.archived = false;
        native_session.activity = devo_protocol::native::session::SessionActivity::Idle;
        native_session.active_turn_id = None;
        native_session.queued_count = 0;
        native_session.title = title_override.or_else(|| source.summary.title.clone());
        native_session.last_activity_at = updated_at;
        native_session.transcript_size_bytes = None;
        native_session.settings.effective_context_window = None;
        native_session.usage = devo_protocol::native::usage::SessionUsage {
            total: devo_protocol::native::usage::UsageTotals::default(),
            by_purpose: Vec::new(),
            legacy: None,
            updated_at,
        };
        let mut summary = crate::runtime_session_summary::RuntimeSessionSummary::new(
            native_session,
            updated_at,
            core_session.collaboration_mode,
        );
        summary.prompt_token_estimate = prompt_token_estimate;
        summary.last_query_usage = latest_query_usage.clone();
        summary.last_query_total_tokens = cut_occupancy
            .as_ref()
            .map(|occupancy| occupancy.total_tokens as usize)
            .or_else(|| {
                latest_query_usage
                    .as_ref()
                    .map(|usage| usage.display_total_tokens() as usize)
            })
            .unwrap_or(0);
        summary.last_context_occupancy = cut_occupancy.clone();
        drop(source_core_session);

        let turns_by_id = source
            .turns_by_id
            .iter()
            .filter(|(turn_id, _)| kept_turn_ids.contains(turn_id))
            .map(|(turn_id, turn)| {
                let mut turn = turn.clone();
                turn.native.session_id = session_id;
                (*turn_id, turn)
            })
            .collect();

        core_session.pending_turn_queue = Arc::clone(&source.pending_turn_queue);
        core_session.steer_input_queue = Arc::clone(&source.steer_input_queue);
        let config = core_session.config.clone();
        let pending_turn_queue = Arc::clone(&source.pending_turn_queue);
        let steer_input_queue = Arc::clone(&source.steer_input_queue);
        Ok(RuntimeSession {
            runtime_context,
            rollout_path: None,
            summary,
            config,
            core_session: Arc::new(Mutex::new(core_session)),
            active_turn: None,
            latest_turn,
            loaded_item_count: u64::try_from(kept_items.len()).unwrap_or(u64::MAX),
            history_items: rebuilt_history_items,
            persisted_turn_items: kept_items,
            latest_compaction_snapshot: applicable_compaction,
            turns_by_id,
            pending_turn_queue,
            steer_input_queue,
            agent_tool_policy: source.agent_tool_policy,
            max_turns: source.max_turns,
            deferred_assistant: None,
            deferred_reasoning: None,
            next_item_seq: u64::try_from(source.persisted_turn_items.len().saturating_add(1))
                .unwrap_or(u64::MAX),
            first_user_input: source.first_user_input.clone(),
            tool_registry: source.tool_registry.clone(),
            file_read_ledger: Arc::clone(&source.file_read_ledger),
            session_approval_cache: source.session_approval_cache.clone(),
            turn_approval_cache: source.turn_approval_cache.clone(),
            session_context_recorded: source.session_context_recorded,
        })
    }
}

fn user_turn_ids_from_items(
    persisted_turn_items: &[crate::execution::PersistedTurnItem],
) -> Vec<devo_protocol::native::ids::TurnId> {
    let mut user_turn_ids = Vec::new();
    for item in persisted_turn_items {
        if crate::persisted_native_item::is_user_message(&item.item)
            && user_turn_ids.last() != Some(&item.turn_id)
        {
            user_turn_ids.push(item.turn_id);
        }
    }
    user_turn_ids
}

/// Restricts persisted items to the session's active root→leaf path.
///
/// Forks and rollbacks must copy the conversation the model actually sees:
/// after a rewind, the journal still holds the abandoned branch linearly, so
/// a purely positional cut leaks dead-branch turns into the copy. Tree-
/// visible rows are kept only when they sit on the path; rows that never
/// render in the tree (SubAgent, Warning, …) follow their turn.
///
/// Returns `None` when the path cannot be trusted — empty path, or the
/// journal lags the in-memory items (the filter would silently drop live
/// turns) — leaving callers on the legacy linear cut.
/// Which tip defines the active path for [`filter_items_to_active_path`].
pub(super) enum ActivePathTip {
    /// The journal's current leaf (fork / rollback cuts).
    JournalTip,
    /// An explicit tip item (`session/tree/navigate` target).
    At(devo_core::ItemId),
    /// Explicit rewind before the first item (`session/tree/navigate` to the
    /// session's first user message): the scope is empty, not the linear
    /// fallback — the whole journal is an abandoned branch.
    Root,
}

pub(super) fn filter_items_to_active_path(
    persisted: &[crate::execution::PersistedTurnItem],
    history: &CanonicalHistory,
    tip: ActivePathTip,
) -> Option<Vec<crate::execution::PersistedTurnItem>> {
    let parents = devo_core::resolve_parent_map(history);
    let leaf = match tip {
        ActivePathTip::JournalTip => devo_core::resolve_leaf_id(history, &parents),
        ActivePathTip::At(item_id) => Some(item_id),
        ActivePathTip::Root => None,
    };
    let active: std::collections::HashSet<devo_core::ItemId> = match tip {
        ActivePathTip::Root => std::collections::HashSet::new(),
        _ => devo_core::path_root_to_leaf(leaf.as_ref(), &parents)
            .into_iter()
            .collect(),
    };
    if active.is_empty() && !matches!(tip, ActivePathTip::Root) {
        return None;
    }
    let journal_ids: std::collections::HashSet<devo_core::ItemId> =
        history.items.iter().map(|envelope| envelope.id).collect();
    let journal_lags_memory = persisted.iter().any(|item| {
        devo_core::is_tree_visible_item(&item.item) && !journal_ids.contains(&item.item_id)
    });
    if journal_lags_memory {
        tracing::debug!("journal is behind in-memory items; fork/rollback cuts linear history");
        return None;
    }
    let active_turn_ids: std::collections::HashSet<devo_protocol::native::ids::TurnId> = persisted
        .iter()
        .filter(|item| active.contains(&item.item_id))
        .map(|item| item.turn_id)
        .collect();
    Some(
        persisted
            .iter()
            .filter(|item| {
                active.contains(&item.item_id)
                    || (!devo_core::is_tree_visible_item(&item.item)
                        && active_turn_ids.contains(&item.turn_id))
            })
            .cloned()
            .collect(),
    )
}

fn kept_items_for_user_turn_cut(
    persisted_turn_items: &[crate::execution::PersistedTurnItem],
    selected_turn_id: Option<devo_protocol::native::ids::TurnId>,
    rollback_mode: RollbackMode,
) -> Result<Vec<crate::execution::PersistedTurnItem>, String> {
    let Some(selected_turn_id) = selected_turn_id else {
        return Ok(persisted_turn_items.to_vec());
    };
    if !persisted_turn_items
        .iter()
        .any(|item| item.turn_id == selected_turn_id)
    {
        return Err("selected turn does not exist".to_string());
    }

    match rollback_mode {
        RollbackMode::ThroughUserTurn => Ok(persisted_turn_items
            .iter()
            .take_while(|item| item.turn_id != selected_turn_id)
            .cloned()
            .chain(
                persisted_turn_items
                    .iter()
                    .skip_while(|item| item.turn_id != selected_turn_id)
                    .take_while(|item| item.turn_id == selected_turn_id)
                    .cloned(),
            )
            .collect()),
        RollbackMode::BeforeUserTurn => Ok(persisted_turn_items
            .iter()
            .take_while(|item| item.turn_id != selected_turn_id)
            .cloned()
            .collect()),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use devo_core::ItemId;

    fn user_item(
        turn_id: devo_protocol::native::ids::TurnId,
        text: &str,
    ) -> crate::execution::PersistedTurnItem {
        crate::persisted_native_item::PersistedNativeItem::new(
            turn_id,
            devo_protocol::native::turn::TurnKind::Regular,
            devo_protocol::native::ids::ItemId::new(),
            crate::persisted_native_item::user_message_item(
                text,
                &[],
                devo_protocol::native::item::UserMessageEntry::TurnStart,
            ),
        )
    }

    fn assistant_item(
        turn_id: devo_protocol::native::ids::TurnId,
        text: &str,
    ) -> crate::execution::PersistedTurnItem {
        crate::persisted_native_item::PersistedNativeItem::new(
            turn_id,
            devo_protocol::native::turn::TurnKind::Regular,
            devo_protocol::native::ids::ItemId::new(),
            devo_protocol::native::item::Item::AssistantMessage {
                text: text.to_string(),
            },
        )
    }

    fn two_turn_items() -> (
        Vec<crate::execution::PersistedTurnItem>,
        Vec<crate::execution::PersistedTurnItem>,
    ) {
        let first_turn_id = devo_protocol::native::ids::TurnId::new();
        let second_turn_id = devo_protocol::native::ids::TurnId::new();
        let items = vec![
            user_item(first_turn_id, "first user"),
            assistant_item(first_turn_id, "first answer"),
            user_item(second_turn_id, "second user"),
            assistant_item(second_turn_id, "second answer"),
        ];
        let prefix = items[..2].to_vec();
        (items, prefix)
    }

    #[test]
    fn kept_items_for_user_turn_cut_modes() {
        let (items, prefix) = two_turn_items();
        let second_turn_id = items[2].turn_id;
        assert_eq!(
            kept_items_for_user_turn_cut(
                &items,
                Some(second_turn_id),
                RollbackMode::ThroughUserTurn,
            )
            .expect("keep selected turn"),
            items
        );
        assert_eq!(
            kept_items_for_user_turn_cut(
                &items,
                Some(second_turn_id),
                RollbackMode::BeforeUserTurn,
            )
            .expect("drop selected turn"),
            prefix
        );
        let turn_id = devo_protocol::native::ids::TurnId::new();
        let single_turn = vec![
            user_item(turn_id, "first user"),
            assistant_item(turn_id, "first answer"),
        ];
        assert_eq!(
            kept_items_for_user_turn_cut(
                &single_turn,
                Some(turn_id),
                RollbackMode::BeforeUserTurn,
            )
            .expect("drop first turn"),
            Vec::new()
        );
        let unknown_turn = devo_protocol::native::ids::TurnId::new();
        assert_eq!(
            kept_items_for_user_turn_cut(
                &single_turn,
                Some(unknown_turn),
                RollbackMode::ThroughUserTurn,
            )
            .expect_err("selected turn must exist"),
            "selected turn does not exist"
        );
    }

    /// Envelope with explicit parent, matching how the navigate handler
    /// journals rewound branches.
    fn envelope(
        id: devo_core::ItemId,
        seq: u64,
        parent_id: Option<devo_core::ItemId>,
        item: devo_protocol::native::item::Item,
    ) -> devo_protocol::native::item::ItemEnvelope {
        devo_protocol::native::item::ItemEnvelope {
            id,
            session_id: devo_core::SessionId::new(),
            turn_id: devo_core::TurnId::new(),
            seq,
            revision: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            state: devo_protocol::native::item::ItemState::Completed,
            item,
            parent_id,
        }
    }

    /// Linear journal of a session that was rewound: turn 1 kept, turn 2
    /// abandoned, turn 3 the new branch (explicit parent edge back to turn
    /// 1's user message), leaf pointing at turn 3's assistant reply.
    fn rewound_history(
        _turn_ids: [devo_core::TurnId; 3],
        item_ids: [devo_core::ItemId; 6],
    ) -> CanonicalHistory {
        let envelopes = vec![
            envelope(item_ids[0], 1, None, user_message("first user")),
            envelope(
                item_ids[1],
                2,
                Some(item_ids[0]),
                assistant_message("first answer"),
            ),
            envelope(
                item_ids[2],
                3,
                Some(item_ids[1]),
                user_message("dead branch user"),
            ),
            envelope(
                item_ids[3],
                4,
                Some(item_ids[2]),
                assistant_message("dead branch answer"),
            ),
            envelope(
                item_ids[4],
                5,
                Some(item_ids[1]),
                user_message("rewound user"),
            ),
            envelope(
                item_ids[5],
                6,
                Some(item_ids[4]),
                assistant_message("rewound answer"),
            ),
        ];
        CanonicalHistory {
            items: envelopes,
            tree_edges: std::collections::HashMap::new(),
            leaf_id: Some(item_ids[5]),
            ..CanonicalHistory::default()
        }
    }

    fn user_message(text: &str) -> devo_protocol::native::item::Item {
        crate::persisted_native_item::user_message_item(
            text,
            &[],
            devo_protocol::native::item::UserMessageEntry::TurnStart,
        )
    }

    fn assistant_message(text: &str) -> devo_protocol::native::item::Item {
        devo_protocol::native::item::Item::AssistantMessage {
            text: text.to_string(),
        }
    }

    fn persisted_with_ids(
        turn_id: devo_core::TurnId,
        item_id: devo_core::ItemId,
        item: devo_protocol::native::item::Item,
    ) -> crate::execution::PersistedTurnItem {
        crate::persisted_native_item::PersistedNativeItem::new(
            turn_id,
            devo_protocol::native::turn::TurnKind::Regular,
            item_id,
            item,
        )
    }

    #[test]
    fn filter_items_to_active_path_drops_dead_branch() {
        let turn_ids = [
            devo_core::TurnId::new(),
            devo_core::TurnId::new(),
            devo_core::TurnId::new(),
        ];
        let item_ids: Vec<devo_core::ItemId> = (0..6).map(|_| devo_core::ItemId::new()).collect();
        let history = rewound_history(turn_ids, item_ids.clone().try_into().unwrap());

        // In-memory linear journal: all three turns, dead branch in the middle.
        let persisted = vec![
            persisted_with_ids(turn_ids[0], item_ids[0], user_message("first user")),
            persisted_with_ids(turn_ids[0], item_ids[1], assistant_message("first answer")),
            persisted_with_ids(turn_ids[1], item_ids[2], user_message("dead branch user")),
            persisted_with_ids(
                turn_ids[1],
                item_ids[3],
                assistant_message("dead branch answer"),
            ),
            persisted_with_ids(turn_ids[2], item_ids[4], user_message("rewound user")),
            persisted_with_ids(
                turn_ids[2],
                item_ids[5],
                assistant_message("rewound answer"),
            ),
        ];

        let filtered = filter_items_to_active_path(&persisted, &history, ActivePathTip::JournalTip)
            .expect("active path resolves for a rewound session");
        let kept_texts: Vec<&str> = filtered
            .iter()
            .map(|item| match &item.item {
                devo_protocol::native::item::Item::UserMessage { content, .. } => content
                    .iter()
                    .find_map(|part| match part {
                        devo_protocol::native::item::UserInput::Text { text } => {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .unwrap_or_default(),
                devo_protocol::native::item::Item::AssistantMessage { text } => text.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(
            kept_texts,
            vec![
                "first user",
                "first answer",
                "rewound user",
                "rewound answer"
            ],
            "dead-branch rows must not leak onto the active path"
        );
        assert_eq!(filtered.len(), 4);

        // A fork through the rewound turn copies only the live prefix.
        let kept = kept_items_for_user_turn_cut(
            &filtered,
            Some(turn_ids[2]),
            RollbackMode::ThroughUserTurn,
        )
        .expect("cut through rewound turn");
        assert_eq!(kept.len(), 4);
    }

    #[test]
    fn filter_items_to_active_path_falls_back_when_journal_lags() {
        let turn_id = devo_core::TurnId::new();
        let journal_item_id = devo_core::ItemId::new();
        let live_item_id = devo_core::ItemId::new();
        let history = CanonicalHistory {
            items: vec![envelope(
                journal_item_id,
                1,
                None,
                user_message("journaled user"),
            )],
            ..CanonicalHistory::default()
        };
        // Memory has a visible item the journal has never seen.
        let persisted = vec![
            persisted_with_ids(turn_id, journal_item_id, user_message("journaled user")),
            persisted_with_ids(
                turn_id,
                live_item_id,
                assistant_message("unjournaled answer"),
            ),
        ];
        assert!(
            filter_items_to_active_path(&persisted, &history, ActivePathTip::JournalTip).is_none(),
            "a stale journal must not silently drop live turns"
        );
    }

    #[test]
    fn filter_items_to_active_path_keeps_unlisted_rows_with_their_turn() {
        let turn_ids = [devo_core::TurnId::new(), devo_core::TurnId::new()];
        let item_ids: Vec<devo_core::ItemId> = (0..4).map(|_| devo_core::ItemId::new()).collect();
        let envelopes = vec![
            envelope(item_ids[0], 1, None, user_message("live user")),
            envelope(
                item_ids[1],
                2,
                Some(item_ids[0]),
                assistant_message("live answer"),
            ),
            envelope(item_ids[2], 3, Some(item_ids[1]), user_message("dead user")),
            envelope(
                item_ids[3],
                4,
                Some(item_ids[2]),
                assistant_message("dead answer"),
            ),
        ];
        let history = CanonicalHistory {
            items: envelopes,
            leaf_id: Some(item_ids[1]),
            ..CanonicalHistory::default()
        };
        let subagent_row = |turn_id: devo_core::TurnId| {
            persisted_with_ids(
                turn_id,
                devo_core::ItemId::new(),
                devo_protocol::native::item::Item::SubAgent {
                    origin_call_id: None,
                    agent_session_id: devo_core::SessionId::new(),
                    parent_session_id: devo_core::SessionId::new(),
                    role: Some("probe".to_string()),
                    task: "probe task".to_string(),
                    state: devo_protocol::native::item::SpawnedWorkState::Completed,
                },
            )
        };
        // SubAgent rows ride along with their turn, alive or dead.
        let persisted = vec![
            subagent_row(turn_ids[0]),
            persisted_with_ids(turn_ids[0], item_ids[0], user_message("live user")),
            persisted_with_ids(turn_ids[0], item_ids[1], assistant_message("live answer")),
            subagent_row(turn_ids[1]),
            persisted_with_ids(turn_ids[1], item_ids[2], user_message("dead user")),
            persisted_with_ids(turn_ids[1], item_ids[3], assistant_message("dead answer")),
        ];
        let filtered = filter_items_to_active_path(&persisted, &history, ActivePathTip::JournalTip)
            .expect("leaf resolves without a sessionLeaf record");
        assert_eq!(filtered.len(), 3);
        assert!(filtered.iter().any(|item| matches!(
            item.item,
            devo_protocol::native::item::Item::SubAgent { .. }
        )));
    }

    #[test]
    fn filter_items_to_active_path_scopes_to_an_explicit_tip() {
        let turn_ids = [
            devo_core::TurnId::new(),
            devo_core::TurnId::new(),
            devo_core::TurnId::new(),
        ];
        let item_ids: Vec<devo_core::ItemId> = (0..6).map(|_| devo_core::ItemId::new()).collect();
        let history = rewound_history(turn_ids, item_ids.clone().try_into().unwrap());
        let persisted = vec![
            persisted_with_ids(turn_ids[0], item_ids[0], user_message("first user")),
            persisted_with_ids(turn_ids[0], item_ids[1], assistant_message("first answer")),
            persisted_with_ids(turn_ids[1], item_ids[2], user_message("dead branch user")),
            persisted_with_ids(
                turn_ids[1],
                item_ids[3],
                assistant_message("dead branch answer"),
            ),
            persisted_with_ids(turn_ids[2], item_ids[4], user_message("rewound user")),
            persisted_with_ids(
                turn_ids[2],
                item_ids[5],
                assistant_message("rewound answer"),
            ),
        ];

        // Navigating onto the abandoned branch swaps the roles: the "dead"
        // rows are the path and the journal-tip rows fall off it.
        let filtered =
            filter_items_to_active_path(&persisted, &history, ActivePathTip::At(item_ids[3]))
                .expect("explicit tip resolves");
        let kept_ids: Vec<devo_core::ItemId> = filtered.iter().map(|item| item.item_id).collect();
        assert_eq!(
            kept_ids,
            vec![item_ids[0], item_ids[1], item_ids[2], item_ids[3]],
            "the explicit dead-branch tip must scope to that branch"
        );

        // Navigating before the first item abandons the entire journal.
        let emptied = filter_items_to_active_path(&persisted, &history, ActivePathTip::Root)
            .expect("root tip is an explicit empty scope");
        assert!(
            emptied.is_empty(),
            "root tip must drop every tree-visible row, got {emptied:?}"
        );
    }

    #[test]
    fn cut_occupancy_uses_cut_turn_not_tip() {
        use pretty_assertions::assert_eq;

        let turn_a = devo_core::TurnId::new();
        let turn_b = devo_core::TurnId::new();
        let session_id = devo_core::SessionId::new();
        let occupancy = |conversation: u64| {
            devo_protocol::native::item::ContextOccupancy::from_category_tokens(
                /*context_window_tokens*/ 100_000,
                /*base*/ 10_000,
                /*skills*/ 0,
                /*tools_builtin*/ 0,
                /*tools_mcp*/ 0,
                conversation,
            )
        };
        let occupancy_a = occupancy(20_000);
        let occupancy_b = occupancy(80_000);
        let usage = |input: u64, output: u64| devo_protocol::native::usage::TurnUsage {
            query: devo_protocol::native::usage::UsageTotals {
                total_tokens: input + output,
                input_tokens: input,
                output_tokens: output,
                metered_call_count: 1,
                ..devo_protocol::native::usage::UsageTotals::default()
            },
            overhead: devo_protocol::native::usage::UsageTotals::default(),
        };
        let usage_a = usage(30, 5);
        let usage_b = usage(90, 10);
        let replayed =
            |turn_id: devo_core::TurnId,
             sequence: u32,
             occupancy: devo_protocol::native::item::ContextOccupancy,
             latest_query_usage: devo_protocol::native::usage::TurnUsage| {
                crate::replay_hydrate::ReplayedTurn::from_native(
                    devo_protocol::native::turn::Turn {
                        id: turn_id,
                        session_id,
                        sequence,
                        kind: devo_protocol::native::turn::TurnKind::Regular,
                        status: devo_protocol::native::turn::TurnStatus::Completed,
                        model: devo_protocol::native::model::ModelBinding {
                            provider: "unknown".into(),
                            model: "m".into(),
                            variant: None,
                            reasoning_effort: None,
                        },
                        collaboration_mode: None,
                        started_at: Utc::now(),
                        completed_at: Some(Utc::now()),
                        error: None,
                        usage: None,
                    },
                    Some(devo_core::TurnPersistenceExtras {
                        session_context: None,
                        turn_context: None,
                        request_thinking: None,
                        input_token_estimate: None,
                        latest_query_usage: Some(latest_query_usage),
                        context_occupancy: Some(occupancy),
                        stop_reason: None,
                        failure_reason: None,
                    }),
                )
            };
        let mut turns = std::collections::HashMap::new();
        turns.insert(turn_a, replayed(turn_a, 1, occupancy_a.clone(), usage_a));
        turns.insert(turn_b, replayed(turn_b, 2, occupancy_b.clone(), usage_b));

        let kept = [turn_a].into_iter().collect();
        let (got, usage, compact) =
            resolve_cut_occupancy_and_usage(&kept, Some(turn_a), &turns, None);
        assert_eq!(got, Some(occupancy_a.clone()));
        assert_eq!(usage.map(|u| u.query.input_tokens), Some(30));
        assert!(compact.is_none());

        let tip_only_snapshot = devo_core::CompactionSnapshotLine {
            timestamp: Utc::now(),
            session_id,
            turn_id: turn_b,
            summary_item_id: ItemId::new(),
            preserved_item_ids: Vec::new(),
            context_occupancy: Some(occupancy_b.clone()),
        };
        let (got, usage, compact) =
            resolve_cut_occupancy_and_usage(&kept, Some(turn_a), &turns, Some(&tip_only_snapshot));
        assert_eq!(got, Some(occupancy_a));
        assert_eq!(usage.map(|u| u.query.input_tokens), Some(30));
        assert!(compact.is_none());

        let kept_all = [turn_a, turn_b].into_iter().collect();
        let compact_occupancy = occupancy(5_000);
        let snapshot = devo_core::CompactionSnapshotLine {
            timestamp: Utc::now(),
            session_id,
            turn_id: turn_b,
            summary_item_id: ItemId::new(),
            preserved_item_ids: Vec::new(),
            context_occupancy: Some(compact_occupancy.clone()),
        };
        let (got, usage, compact) =
            resolve_cut_occupancy_and_usage(&kept_all, Some(turn_b), &turns, Some(&snapshot));
        assert_eq!(got, Some(compact_occupancy));
        assert_eq!(usage.map(|u| u.query.input_tokens), Some(90));
        assert_eq!(compact.as_ref().map(|s| s.turn_id), Some(turn_b));
    }
}
