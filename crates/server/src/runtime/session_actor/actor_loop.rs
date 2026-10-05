use std::sync::Arc;

use anyhow::Context;
use chrono::Utc;
use devo_core::SessionTitleFinalSource;
use devo_core::SessionTitleState;
use devo_core::TurnConfig;
use devo_protocol::native::turn::TurnStatus;
use tokio::sync::mpsc;

use super::approval_scope::{
    apply_approval_scope_to_state, apply_path_scope_to_permission_profile, credential_delivery_root,
};
use super::commands::{ApprovalCheckpointSnapshot, SessionCommand};
use super::snapshots::{
    HookContextSnapshot, PendingQueueSnapshot, QueuedTurnInputData, ShellExecContextSnapshot,
    TitleGenerationContext, TurnPersistenceSnapshot, TurnReservationSnapshot,
};
use super::state::SessionActorState;
use crate::runtime::protocol_preset_from_safety;
use crate::runtime::session_model_selection;
use devo_protocol::native::session::SessionStatus;

pub(super) async fn run_session_actor(
    mut state: SessionActorState,
    mut mailbox: mpsc::Receiver<SessionCommand>,
    _runtime: Arc<crate::runtime::ServerRuntime>,
) {
    while let Some(command) = mailbox.recv().await {
        match command {
            SessionCommand::CheckoutTurnWorkingSet { turn, reply } => {
                let working = state.checkout_turn_working_set(&turn);
                {
                    let mut stream = working.state.stream.lock().await;
                    stream.turn_inline = Some(super::turn_inline::TurnInlineState::new(
                        &working.state,
                        &turn,
                    ));
                }
                let _ = reply.send(working);
            }
            SessionCommand::MergeTurn { working, reply } => {
                state.merge_turn_working_set(*working);
                let _ = reply.send(());
            }
            SessionCommand::GetSummary { reply } => {
                let _ = reply.send(state.summary.clone());
            }
            SessionCommand::GetNativeSession { reply } => {
                let _ = reply.send(state.summary.native.clone());
            }
            SessionCommand::GetSpawnSnapshot { reply } => {
                let snapshot = state.spawn_snapshot();
                let _ = reply.send(snapshot);
            }
            SessionCommand::GetTranscriptItems { reply } => {
                // Include the active turn's in-flight items: while a turn is
                // running they live in the turn-inline working set and only
                // merge into the actor copy at turn end, so the merged copy
                // alone would read as empty for a live child.
                let mut items = state.persisted_turn_items.clone();
                let inline_items = {
                    let stream = state.stream.lock().await;
                    stream
                        .turn_inline
                        .as_ref()
                        .map(|inline| inline.persisted_turn_items.clone())
                        .unwrap_or_default()
                };
                let existing: std::collections::HashSet<_> =
                    items.iter().map(|item| item.item_id).collect();
                items.extend(
                    inline_items
                        .into_iter()
                        .filter(|item| !existing.contains(&item.item_id)),
                );
                let _ = reply.send(items);
            }
            SessionCommand::GetApprovalCacheSnapshot { reply } => {
                let _ = reply.send(state.approval_cache_snapshot());
            }
            SessionCommand::GetCollaborationMode { reply } => {
                let _ = reply.send(state.core.collaboration_mode);
            }
            SessionCommand::GetParentSessionId { reply } => {
                let _ = reply.send(state.parent_session_id());
            }
            SessionCommand::GetTurnReservationSnapshot { reply } => {
                let _ = reply.send(TurnReservationSnapshot {
                    max_turns: state.max_turns,
                    active_turn: state.active_turn.clone(),
                    latest_turn: state.latest_turn.clone(),
                    ephemeral: state.summary.ephemeral,
                    parent_session_id: state.parent_session_id(),
                    summary: state.summary.clone(),
                    runtime_context: Arc::clone(&state.runtime_context),
                    pending_turn_queue: Arc::clone(&state.pending_turn_queue),
                    steer_input_queue: Arc::clone(&state.steer_input_queue),
                });
            }
            SessionCommand::GetHookContextSnapshot { reply } => {
                let _ = reply.send(HookContextSnapshot {
                    runtime_context: Arc::clone(&state.runtime_context),
                    rollout_path: state.rollout_path.clone(),
                    summary: state.summary.clone(),
                    config: state.config.clone(),
                });
            }
            SessionCommand::GetTurnPersistenceSnapshot { reply } => {
                let _ = reply.send(TurnPersistenceSnapshot {
                    rollout_path: state.rollout_path.clone(),
                });
            }
            SessionCommand::GetShellExecContext { cwd, reply } => {
                let _ = &cwd;
                let _ = reply.send(ShellExecContextSnapshot {
                    sandbox_profile: state.core.config.sandbox_profile.clone(),
                });
            }
            SessionCommand::GetTitleGenerationContext { reply } => {
                let _ = reply.send(TitleGenerationContext {
                    model_selection: session_model_selection(&state.summary).map(str::to_string),
                    reasoning_effort_selection: state.summary.settings.reasoning_effort.clone(),
                    title_state: state.summary.title_state.clone(),
                    runtime_context: Arc::clone(&state.runtime_context),
                });
            }
            SessionCommand::GetPendingQueueSnapshot { reply } => {
                let queue = state
                    .pending_turn_queue
                    .lock()
                    .expect("pending turn queue mutex should not be poisoned");
                let pending_count = queue
                    .iter()
                    .filter(|item| {
                        matches!(
                            &item.kind,
                            devo_core::PendingInputKind::UserText { .. }
                                | devo_core::PendingInputKind::UserInput { .. }
                        )
                    })
                    .count();
                let _ = reply.send(PendingQueueSnapshot { pending_count });
            }
            SessionCommand::PopQueuedTurnInput {
                require_idle_session,
                reply,
            } => {
                if require_idle_session && state.active_turn.is_some() {
                    let _ = reply.send(None);
                    continue;
                }
                let mut queue = state
                    .pending_turn_queue
                    .lock()
                    .expect("pending turn queue mutex should not be poisoned");
                tracing::debug!(
                    session_id = %state.session_id(),
                    queue_len = queue.len(),
                    require_idle_session,
                    "PopQueuedTurnInput"
                );
                let popped = queue.pop_front().and_then(pop_queued_turn_input_data);
                let _ = reply.send(popped);
            }
            SessionCommand::EnqueuePendingTurnInput { item } => {
                state
                    .pending_turn_queue
                    .lock()
                    .expect("pending turn queue mutex should not be poisoned")
                    .push_back(item);
            }
            SessionCommand::GetActiveTurnId { reply } => {
                let _ = reply.send(state.active_turn.as_ref().map(|turn| turn.turn_id()));
            }
            SessionCommand::GetApprovalCheckpointSnapshot { reply } => {
                let snapshot = if state.active_turn.is_some() {
                    let stream = state.stream.lock().await;
                    let turn_config = stream
                        .turn_inline
                        .as_ref()
                        .and_then(|inline| {
                            inline
                                .live_turn_settings
                                .lock()
                                .ok()
                                .and_then(|live| live.turn_config.clone())
                        })
                        .or_else(|| {
                            Some(
                                state.runtime_context.resolve_turn_config(
                                    state
                                        .summary
                                        .model_binding_id()
                                        .or(state.summary.model_name()),
                                    state.summary.settings.reasoning_effort.clone(),
                                ),
                            )
                        });
                    turn_config.map(|turn_config| ApprovalCheckpointSnapshot {
                        messages: state.core.messages.clone(),
                        turn_config,
                        collaboration_mode: state.core.collaboration_mode,
                    })
                } else {
                    None
                };
                let _ = reply.send(snapshot);
            }
            SessionCommand::MarkActiveTurnWaitingApproval { turn_id, reply } => {
                let turn = state
                    .active_turn
                    .as_mut()
                    .filter(|turn| turn.turn_id() == turn_id)
                    .map(|turn| {
                        turn.native.status = TurnStatus::WaitingApproval;
                        turn.clone()
                    });
                if let Some(turn) = &turn {
                    state.latest_turn = Some(turn.clone());
                }
                let _ = reply.send(turn);
            }
            SessionCommand::GetRolloutPath { reply } => {
                let _ = reply.send(state.rollout_path.clone());
            }
            SessionCommand::PreparePersistItem { turn_id, reply } => {
                let turn_kind = state
                    .active_turn
                    .as_ref()
                    .filter(|turn| turn.turn_id() == turn_id)
                    .map(|turn| turn.native.kind)
                    .or_else(|| {
                        state
                            .latest_turn
                            .as_ref()
                            .filter(|turn| turn.turn_id() == turn_id)
                            .map(|turn| turn.native.kind)
                    })
                    .unwrap_or_default();
                let _ = reply.send(super::snapshots::PersistItemPrep {
                    turn_kind,
                    rollout_path: state.rollout_path.clone(),
                    transcript_leaf_id: state.transcript_leaf_id,
                    leaf_epoch: state.leaf_epoch,
                });
            }
            SessionCommand::TakeShutdownDeferredSnapshot { reply } => {
                let stream = state.stream.lock().await;
                let _ = reply.send(super::snapshots::ShutdownDeferredSnapshot {
                    deferred_assistant: stream.deferred_assistant.clone(),
                    deferred_reasoning: stream.deferred_reasoning.clone(),
                    active_turn: state.active_turn.clone(),
                    active_turn_id: state.active_turn.as_ref().map(|turn| turn.turn_id()),
                    rollout_path: state.rollout_path.clone(),
                });
            }
            SessionCommand::AllocateItemSeq { reply } => {
                let item_seq = state.next_item_seq;
                state.next_item_seq = state.next_item_seq.saturating_add(1);
                state.loaded_item_count = state.loaded_item_count.saturating_add(1);
                let _ = reply.send(item_seq);
            }
            SessionCommand::AppendPersistedItem { item } => {
                state.persisted_turn_items.push(item);
            }
            SessionCommand::AppendHistoryItem { item } => {
                state.history_items.push(item);
            }
            SessionCommand::TakeDeferredItems { reply } => {
                let _ = reply.send(state.stream.lock().await.take_deferred_items());
            }
            SessionCommand::TouchLastActivity => {
                state.summary.last_activity_at = state.summary.last_activity_at.max(Utc::now());
            }
            SessionCommand::ApplyApprovalScope { scope, pending } => {
                apply_approval_scope_to_state(
                    &mut state.session_approval_cache,
                    &mut state.turn_approval_cache,
                    &scope,
                    &pending,
                );
                apply_path_scope_to_permission_profile(
                    &mut state.core.config.permission_profile,
                    &scope,
                    &pending,
                );
                apply_path_scope_to_permission_profile(
                    &mut state.config.permission_profile,
                    &scope,
                    &pending,
                );
                // P2 (design doc §8/§9): a PathPrefix write approval is not
                // just a cache/profile entry — deliver it as a credential to
                // the fenced kernel so native I/O works from now on, with no
                // kernel restart. Scope selection lives in
                // `credential_delivery_root` (tested); Session-scope
                // exact-file approvals deliberately stay mediated.
                // Windows: session-SID ACE. Unix: dirfd over SCM_RIGHTS.
                if let Some((root, access)) = credential_delivery_root(&scope, &pending)
                    && let Some(kernel) = state.kernel.as_ref()
                {
                    #[cfg(windows)]
                    if let Some(authority) = kernel.fence_credentials() {
                        let delivered = match access {
                            super::approval_scope::CredentialAccess::Write => {
                                authority.grant_write_root(&root)
                            }
                            super::approval_scope::CredentialAccess::Read => {
                                authority.grant_read_root(&root)
                            }
                        };
                        match delivered {
                            Ok(_) => tracing::info!(
                                root = %root.display(),
                                "session credential delivered: ACE for the fenced kernel \
                                 (no restart)"
                            ),
                            Err(err) => tracing::warn!(
                                %err,
                                root = %root.display(),
                                "session credential ACE delivery failed; \
                                 the mediated fallback remains active"
                            ),
                        }
                    }
                    #[cfg(unix)]
                    if let Some(channel) = kernel.grant_channel() {
                        let access_str = match access {
                            super::approval_scope::CredentialAccess::Write => "write",
                            super::approval_scope::CredentialAccess::Read => "read",
                        };
                        match channel.grant(&root, access_str) {
                            Ok(_) => tracing::info!(
                                root = %root.display(),
                                access = access_str,
                                "session credential delivered: dirfd for the fenced kernel \
                                 (no restart)"
                            ),
                            Err(err) => tracing::warn!(
                                %err,
                                root = %root.display(),
                                "session credential dirfd delivery failed; \
                                 the mediated fallback remains active"
                            ),
                        }
                    }
                }
            }
            SessionCommand::UpdateSummary { summary } => {
                state.summary = summary;
            }
            SessionCommand::SetFirstUserInputIfUnset { text, reply } => {
                if state.first_user_input.is_none() {
                    state.first_user_input = Some(text.clone());
                }
                let _ = reply.send(state.first_user_input.clone());
            }
            SessionCommand::UpdateTitle {
                title,
                title_state,
                reply,
            } => {
                let allow = match (&state.summary.title_state, &title_state) {
                    (
                        SessionTitleState::Final(SessionTitleFinalSource::Heuristic),
                        SessionTitleState::Final(SessionTitleFinalSource::ModelGenerated),
                    ) => true,
                    (SessionTitleState::Final(_), _) => false,
                    _ => true,
                };
                if !allow {
                    let _ = reply.send(None);
                    continue;
                }
                let updated_at = Utc::now();
                state.summary.title = Some(title.clone());
                state.summary.title_state = title_state.clone();
                state.summary.updated_at = updated_at;
                let _ = reply.send(Some(state.summary.clone()));
            }
            SessionCommand::BeginActiveTurn { turn, turn_config } => {
                let now = Utc::now();
                apply_turn_config_to_session_summary(&mut state.summary, &turn_config);
                ensure_session_context_locked(&mut state, &turn_config);
                state.summary.set_status(SessionStatus::Active);
                state.summary.updated_at = now;
                state.summary.last_activity_at = now;
                state.active_turn = Some(turn);
            }
            SessionCommand::ClearActiveTurnIfMatches { turn_id, reply } => {
                let cleared = state
                    .active_turn
                    .as_ref()
                    .is_some_and(|active| active.turn_id() == turn_id);
                if cleared {
                    state.active_turn = None;
                    state.summary.set_status(SessionStatus::Idle);
                    state.summary.updated_at = Utc::now();
                    state.summary.last_activity_at = state.summary.updated_at;
                }
                let _ = reply.send(cleared);
            }
            SessionCommand::SetSessionIdle { latest_turn } => {
                let now = Utc::now();
                if let Some(latest_turn) = latest_turn {
                    state.latest_turn = Some(latest_turn);
                }
                state.active_turn = None;
                state.summary.set_status(SessionStatus::Idle);
                state.summary.updated_at = now;
                state.summary.last_activity_at = now;
            }
            SessionCommand::SetActiveGoal { goal } => match goal {
                Some(goal) => state.core.set_active_goal(goal),
                None => state.core.clear_active_goal(),
            },
            SessionCommand::ActivateQueuedTurn { turn, turn_config } => {
                let now = Utc::now();
                apply_turn_config_to_session_summary(&mut state.summary, &turn_config);
                ensure_session_context_locked(&mut state, &turn_config);
                state.summary.set_status(SessionStatus::Active);
                state.summary.updated_at = now;
                state.summary.last_activity_at = now;
                state.active_turn = Some(turn);
            }
            SessionCommand::UpdateCorePermissionMode { permission_mode } => {
                state.core.config.permission_mode = permission_mode;
                state.config.permission_mode = permission_mode;
            }
            SessionCommand::UpdateRolloutPath { rollout_path } => {
                state.rollout_path = Some(rollout_path);
            }
            SessionCommand::SetTranscriptLeaf {
                leaf_id,
                epoch,
                scoped_item_ids,
                compaction_snapshot,
            } => {
                state.transcript_leaf_id = leaf_id;
                state.leaf_epoch = epoch;
                if let Some(ids) = scoped_item_ids {
                    // Mirror load_session_from_rollout's path scoping for the
                    // live actor: tree-visible rows survive only on the
                    // navigated branch, unlisted rows (SubAgent, Warning, …)
                    // follow their turn, and the prompt/history rebuild drops
                    // the abandoned branch from the next turn's context.
                    let keep: std::collections::HashSet<devo_protocol::native::ids::ItemId> =
                        ids.into_iter().collect();
                    let active_turn_ids: std::collections::HashSet<
                        devo_protocol::native::ids::TurnId,
                    > = state
                        .persisted_turn_items
                        .iter()
                        .filter(|item| keep.contains(&item.item_id))
                        .map(|item| item.turn_id)
                        .collect();
                    state.persisted_turn_items.retain(|item| {
                        keep.contains(&item.item_id)
                            || (!devo_core::is_tree_visible_item(&item.item)
                                && active_turn_ids.contains(&item.turn_id))
                    });
                    let mut rebuilt_messages = Vec::new();
                    let mut rebuilt_history = Vec::new();
                    let mut tool_names = std::collections::HashMap::new();
                    for item in &state.persisted_turn_items {
                        crate::prompt_from_native_item::apply_native_item(
                            &mut rebuilt_messages,
                            &mut rebuilt_history,
                            &mut tool_names,
                            item.item.clone(),
                        );
                    }
                    state.history_items = rebuilt_history;
                    // The re-scope must not undo compaction: when the
                    // snapshot's summary item survives the scope, rebuild the
                    // prompt from the snapshot (compacted shape); only a
                    // navigate to a leaf before the summary falls back to the
                    // full scoped history, where the summary is gone.
                    let prompt_messages = compaction_snapshot
                        .as_ref()
                        .and_then(|snapshot| {
                            crate::persistence::build_prompt_messages_from_snapshot(
                                &state.persisted_turn_items,
                                snapshot,
                            )
                        })
                        .unwrap_or(rebuilt_messages);
                    state.core.set_prompt_messages(prompt_messages);
                }
            }
            SessionCommand::ApplyParentUsageSnapshot { snapshot } => {
                snapshot.apply_to_actor_state(&mut state);
            }
            SessionCommand::InterruptActiveTurn { reply } => {
                let now = Utc::now();
                state.summary.set_status(SessionStatus::Idle);
                state.summary.updated_at = now;
                state.summary.last_activity_at = now;
                state.summary.set_cumulative_usage(
                    state.core.total_input_tokens,
                    state.core.total_output_tokens,
                    state.core.total_tokens,
                    state.core.total_cache_creation_tokens,
                    state.core.total_cache_read_tokens,
                );
                state.summary.prompt_token_estimate = state.core.prompt_token_estimate;
                let interrupted = state.active_turn.take().map(|mut turn| {
                    turn.native.status = TurnStatus::Interrupted;
                    turn.native.completed_at = Some(now);
                    state.latest_turn = Some(turn.clone());
                    turn
                });
                if interrupted.is_some() {
                    state.core.mark_last_turn_interrupted();
                }
                let _ = reply.send(interrupted);
            }
            SessionCommand::ExportRuntimeSession { reply } => {
                let stream = state.stream.lock().await;
                let _ = reply.send(state.to_runtime_session_from_stream(&stream));
            }
            SessionCommand::UpdateSessionWorkspace {
                cwd,
                runtime_context,
            } => {
                state.runtime_context = runtime_context;
                state.core.cwd = cwd.clone();
                state.summary.cwd = cwd;
                state.summary.version = state.summary.version.saturating_add(1);
            }
            SessionCommand::SetArchived { archived, reply } => {
                state.summary.archived = archived;
                state.summary.version = state.summary.version.saturating_add(1);
                let _ = reply.send(state.summary.native.clone());
            }
            SessionCommand::UpdateSessionModelSettings {
                model,
                model_binding_id,
                reasoning_effort_selection,
                collaboration_mode,
                reply,
            } => {
                let updated_at = Utc::now();
                // Mode-only updates omit model fields as null; do not wipe them.
                let mode_only_update = model.is_none()
                    && model_binding_id.is_none()
                    && reasoning_effort_selection.is_none()
                    && collaboration_mode.is_some();
                if !mode_only_update {
                    state.summary.model.model = model.clone().unwrap_or_default();
                    state.summary.model.provider =
                        model_binding_id.clone().unwrap_or_else(|| "unknown".into());
                    state.summary.settings.reasoning_effort = reasoning_effort_selection.clone();
                }
                state.summary.updated_at = updated_at;
                if let Some(mode) = collaboration_mode {
                    state.core.collaboration_mode = mode;
                    state.summary.collaboration_mode = mode;
                }
                state.summary.version = state.summary.version.saturating_add(1);
                let _ = reply.send(state.summary.clone());
            }
            SessionCommand::UpdateNativeSessionSettings {
                auto_refine_enabled,
                auto_refine_turn_interval,
                python_cell_first_wait_ms,
                reply,
            } => {
                if let Some(enabled) = auto_refine_enabled {
                    state.summary.settings.auto_refine_enabled = Some(enabled);
                }
                if let Some(interval) = auto_refine_turn_interval {
                    state.summary.settings.auto_refine_turn_interval = Some(interval.max(1));
                }
                if let Some(ms) = python_cell_first_wait_ms {
                    state.summary.settings.python_cell_first_wait_ms = Some(ms);
                }
                state.summary.updated_at = Utc::now();
                // Keep the actor version aligned with the settings-epoch bump
                // the durable write already made, mirroring
                // UpdateSessionModelSettings.
                state.summary.version = state.summary.version.saturating_add(1);
                let _ = reply.send(());
            }
            SessionCommand::ApplyPermissionProfile { profile, reply } => {
                let sandbox = Some(profile.implied_sandbox_profile().to_string());
                state.core.config.permission_mode = profile.permission_mode();
                state.core.config.permission_profile = profile.clone();
                state.core.config.sandbox_profile = sandbox.clone();
                state.config.permission_mode = profile.permission_mode();
                state.config.permission_profile = profile.clone();
                state.config.sandbox_profile = sandbox;
                state.session_approval_cache = crate::execution::ApprovalGrantCache::default();
                state.turn_approval_cache = crate::execution::ApprovalGrantCache::default();
                let preset = protocol_preset_from_safety(profile.preset);
                state.summary.set_permission_preset(preset);
                let updated_at = Utc::now();
                state.summary.updated_at = updated_at;
                // Keep actor version aligned with settings-epoch bumps on the
                // rollout so session/read refresh does not under-report for
                // the next metadata/update expectedVersion check.
                state.summary.version = state.summary.version.saturating_add(1);
                let _ = reply.send(());
            }
            SessionCommand::ApplyEffectiveContextWindow { limit, reply } => {
                // Applied value is the model usable window; do not keep a
                // sticky session override that could diverge from the model.
                state.core.config.effective_context_window_override = None;
                state.core.config.token_budget.context_window = limit;
                state.core.config.token_budget.auto_compact_token_limit = Some(limit);
                state.config.effective_context_window_override = None;
                state.config.token_budget.context_window = limit;
                state.config.token_budget.auto_compact_token_limit = Some(limit);
                state.summary.settings.effective_context_window = Some(limit as u64);
                let _ = reply.send(Ok(()));
            }
            SessionCommand::ApplySandboxProfile { profile, reply } => {
                // Validation only; approval caches are intentionally preserved:
                // the sandbox profile does not widen tool permissions.
                match crate::sandbox_profile::normalize_sandbox_profile_name(
                    &profile,
                    &state.summary.cwd,
                ) {
                    Ok(name) => {
                        state.core.config.sandbox_profile = Some(name.clone());
                        state.config.sandbox_profile = Some(name.clone());
                        let _ = reply.send(Ok(name));
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
            }
            SessionCommand::SetSessionTitleUserRename { title, reply } => {
                let updated_at = Utc::now();
                state.summary.title = Some(title.clone());
                state.summary.title_state =
                    SessionTitleState::Final(SessionTitleFinalSource::UserRename);
                state.summary.updated_at = updated_at;
                let _ = reply.send(state.summary.clone());
            }
            SessionCommand::SetToolRegistry {
                tool_registry,
                reply,
            } => {
                state.tool_registry = tool_registry;
                let _ = reply.send(());
            }
            SessionCommand::GetRuntimeContext { reply } => {
                let _ = reply.send(Arc::clone(&state.runtime_context));
            }
            SessionCommand::GetResumeSnapshot { reply } => {
                let pending_texts = state
                    .pending_turn_queue
                    .lock()
                    .expect("pending turn queue mutex should not be poisoned")
                    .iter()
                    .filter_map(|item| match &item.kind {
                        devo_core::PendingInputKind::UserText { text } => Some(text.clone()),
                        devo_core::PendingInputKind::UserInput { display_text, .. } => {
                            Some(display_text.clone())
                        }
                        _ => None,
                    })
                    .collect();
                let _ = reply.send(super::snapshots::SessionResumeSnapshot {
                    summary: state.summary.clone(),
                    latest_turn: state.latest_turn.clone(),
                    loaded_item_count: state.loaded_item_count,
                    history_items: state.history_items.clone(),
                    pending_texts,
                });
            }
            SessionCommand::TryBeginActiveTurn {
                turn,
                turn_config,
                reply,
            } => {
                let queue_empty = state
                    .pending_turn_queue
                    .lock()
                    .expect("pending turn queue mutex should not be poisoned")
                    .is_empty();
                if state.active_turn.is_some() || !queue_empty {
                    let _ = reply.send(false);
                    continue;
                }
                let now = Utc::now();
                apply_turn_config_to_session_summary(&mut state.summary, &turn_config);
                ensure_session_context_locked(&mut state, &turn_config);
                state.summary.set_status(SessionStatus::Active);
                state.summary.updated_at = now;
                state.summary.last_activity_at = now;
                state.active_turn = Some(turn);
                let _ = reply.send(true);
            }
            SessionCommand::ReplaceState {
                state: new_state,
                reply,
            } => {
                state = *new_state;
                let _ = reply.send(());
            }
            SessionCommand::PersistTurnLine {
                runtime,
                turn,
                reply,
            } => {
                let result = (|| {
                    let rollout_path = state
                        .rollout_path
                        .as_ref()
                        .context("missing rollout path for turn persistence")?;
                    runtime.rollout_store.append_turn_deduped_at(
                        rollout_path,
                        state.session_id(),
                        &mut state.session_context_recorded,
                        &turn.native,
                        Some(crate::persistence::turn_persistence_extras_from_runtime(
                            &turn,
                            /*session_context*/ None,
                            state.core.latest_turn_context.clone(),
                            /*latest_query_usage*/ None,
                            /*context_occupancy*/ None,
                        )),
                        state.core.session_context.clone(),
                    )
                })();
                let _ = reply.send(result);
            }
            SessionCommand::Shutdown { reply } => {
                // Withdraw the session's delivered credentials (design doc
                // §9): normal session end revokes every ACE this session was
                // granted; crashed sessions are covered by the journal sweep.
                #[cfg(windows)]
                if let Some(kernel) = state.kernel.as_ref()
                    && let Some(authority) = kernel.fence_credentials()
                    && let Err(err) = authority.revoke_all()
                {
                    tracing::warn!(
                        %err,
                        "session credential revocation on shutdown failed; \
                         the startup journal sweep will retry"
                    );
                }
                let kernel = state.kernel.take();
                let _ = reply.send(kernel);
                break;
            }
        }
        state.sync_native_runtime_fields();
    }
}

fn apply_turn_config_to_session_summary(
    summary: &mut crate::runtime_session_summary::RuntimeSessionSummary,
    turn_config: &TurnConfig,
) {
    let model = match &turn_config.provider_route {
        devo_provider::ProviderRoute::Connection { provider_id, .. } => {
            format!("{provider_id}/{}", turn_config.request_model)
        }
        devo_provider::ProviderRoute::Default => turn_config.model.slug.clone(),
    };
    summary.model.model = turn_config
        .variant
        .as_deref()
        .map(|variant| format!("{model}/{variant}"))
        .unwrap_or(model);
    summary.model.provider = String::from("unknown");
    summary.settings.reasoning_effort = turn_config.reasoning_effort_selection.clone();
}

/// Capture locked session context before the first durable turn start is written.
///
/// This must happen before `PersistTurnLine` so a process crash between turn start
/// persistence and query finalization still leaves `SessionContextUpdated` in the
/// rollout journal.
fn ensure_session_context_locked(state: &mut SessionActorState, turn_config: &TurnConfig) {
    if state.core.session_context.is_some() {
        return;
    }
    let agents_md_manager = devo_core::AgentsMdManager::new(state.core.config.agents_md.clone());
    let locked_agents_snapshot =
        devo_core::load_workspace_instructions(&state.core.cwd, &agents_md_manager);
    state.core.session_context = Some(devo_core::SessionContext::capture(
        &turn_config.model,
        turn_config.reasoning_effort_selection.as_deref(),
        &state.core.cwd,
        locked_agents_snapshot,
        state.core.config.available_skills_instructions.clone(),
    ));
}

fn pop_queued_turn_input_data(
    item: devo_protocol::PendingInputItem,
) -> Option<QueuedTurnInputData> {
    match item.kind {
        devo_core::PendingInputKind::UserText { text } => Some(QueuedTurnInputData {
            queued_input_id: item.id,
            client_user_message_id: client_user_message_id_from_pending_metadata(
                item.metadata.as_ref(),
            ),
            display_input: text.clone(),
            input_text: text,
            input_messages: Vec::new(),
            input_images: Vec::new(),
            input_image_paths: Vec::new(),
            collaboration_mode: collaboration_mode_from_pending_metadata(item.metadata.as_ref()),
            model_selection: model_selection_from_pending_metadata(item.metadata.as_ref()),
            subagent_usage_owner: subagent_usage_owner_from_pending_metadata(
                item.metadata.as_ref(),
            ),
        }),
        devo_core::PendingInputKind::UserInput {
            display_text,
            prompt_text,
            prompt_messages,
            prompt_images,
            input,
            ..
        } => {
            let input_image_paths = input
                .iter()
                .filter_map(|item| match item {
                    devo_protocol::native::item::UserInput::LocalImage { path, .. } => {
                        Some(path.clone())
                    }
                    _ => None,
                })
                .collect();
            Some(QueuedTurnInputData {
                queued_input_id: item.id,
                client_user_message_id: client_user_message_id_from_pending_metadata(
                    item.metadata.as_ref(),
                ),
                display_input: display_text,
                input_text: prompt_text,
                input_messages: prompt_messages,
                input_images: prompt_images,
                input_image_paths,
                collaboration_mode: collaboration_mode_from_pending_metadata(
                    item.metadata.as_ref(),
                ),
                model_selection: model_selection_from_pending_metadata(item.metadata.as_ref()),
                subagent_usage_owner: subagent_usage_owner_from_pending_metadata(
                    item.metadata.as_ref(),
                ),
            })
        }
        _ => None,
    }
}

fn client_user_message_id_from_pending_metadata(
    metadata: Option<&serde_json::Value>,
) -> Option<String> {
    metadata
        .and_then(|metadata| metadata.get("clientUserMessageId"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

fn collaboration_mode_from_pending_metadata(
    metadata: Option<&serde_json::Value>,
) -> devo_protocol::CollaborationMode {
    metadata
        .and_then(|metadata| {
            metadata
                .get("collaboration_mode")
                .or_else(|| metadata.get("interaction_mode"))
        })
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn string_field_from_pending_metadata(
    metadata: Option<&serde_json::Value>,
    key: &str,
) -> Option<String> {
    metadata?
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn model_selection_from_pending_metadata(metadata: Option<&serde_json::Value>) -> Option<String> {
    string_field_from_pending_metadata(metadata, "model_binding_id")
        .or_else(|| string_field_from_pending_metadata(metadata, "model"))
}

fn subagent_usage_owner_from_pending_metadata(
    metadata: Option<&serde_json::Value>,
) -> Option<(
    devo_protocol::native::ids::SessionId,
    Option<devo_protocol::native::ids::TurnId>,
)> {
    let parent_session_id =
        string_field_from_pending_metadata(metadata, "devo_subagent_usage_parent_session_id")
            .map(|value| devo_protocol::native::ids::SessionId::from_string(value.to_owned()))?;
    let parent_turn_id =
        string_field_from_pending_metadata(metadata, "devo_subagent_usage_parent_turn_id")
            .map(|value| devo_protocol::native::ids::TurnId::from_string(value.to_owned()));
    Some((parent_session_id, parent_turn_id))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use devo_protocol::PendingInputItem;
    use devo_protocol::PendingInputKind;
    use pretty_assertions::assert_eq;

    use super::QueuedTurnInputData;
    use super::pop_queued_turn_input_data;

    #[test]
    fn pop_queued_turn_input_data_preserves_pending_and_client_message_ids() {
        let item = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "queued prompt".to_string(),
            },
            Some(serde_json::json!({"clientUserMessageId":"stable-message-1"})),
            Utc::now(),
        );
        let queued_input_id = item.id;

        let popped = pop_queued_turn_input_data(item).expect("user input should be queued");

        assert_eq!(
            popped,
            QueuedTurnInputData {
                queued_input_id,
                client_user_message_id: Some("stable-message-1".to_string()),
                display_input: "queued prompt".to_string(),
                input_text: "queued prompt".to_string(),
                input_messages: Vec::new(),
                input_images: Vec::new(),
                input_image_paths: Vec::new(),
                collaboration_mode: devo_protocol::CollaborationMode::default(),
                model_selection: None,
                subagent_usage_owner: None,
            }
        );
    }
}
