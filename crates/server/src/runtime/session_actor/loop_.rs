use std::sync::Arc;

use chrono::Utc;
use devo_core::SessionTitleFinalSource;
use devo_core::SessionTitleState;
use devo_core::TurnConfig;
use devo_core::TurnStatus;
use devo_protocol::ApprovalScopeValue;
use tokio::sync::mpsc;

use super::commands::SessionCommand;
use super::snapshots::{
    HookContextSnapshot, PendingQueueSnapshot, QueuedTurnInputData, ShellExecContextSnapshot,
    TitleGenerationContext, TurnPersistenceSnapshot, TurnReservationSnapshot,
};
use super::state::SessionActorState;
use super::turn::execute_turn_in_actor;
use crate::SessionRuntimeStatus;
use crate::execution::PendingApproval;
use crate::runtime::session_model_selection;

pub(super) async fn run_session_actor(
    mut state: SessionActorState,
    mut mailbox: mpsc::Receiver<SessionCommand>,
    _runtime: Arc<crate::runtime::ServerRuntime>,
) {
    while let Some(command) = mailbox.recv().await {
        match command {
            SessionCommand::ExecuteTurn {
                runtime: turn_runtime,
                request,
                reply,
            } => {
                let session_id = request.session_id;
                execute_turn_in_actor(&mut state, turn_runtime.clone(), request).await;
                let _ = reply.send(());
                tokio::spawn(async move {
                    if !turn_runtime.chain_queued_followup_turn(session_id).await {
                        turn_runtime
                            .maybe_start_goal_continuation_turn(session_id)
                            .await;
                    }
                });
            }
            SessionCommand::GetSummary { reply } => {
                let _ = reply.send(state.summary.clone());
            }
            SessionCommand::GetSpawnSnapshot { reply } => {
                let snapshot = state.spawn_snapshot();
                let _ = reply.send(snapshot);
            }
            SessionCommand::GetApprovalCacheSnapshot { reply } => {
                let _ = reply.send(state.approval_cache_snapshot());
            }
            SessionCommand::GetCollaborationMode { reply } => {
                let _ = reply.send(state.core.collaboration_mode);
            }
            SessionCommand::GetRuntimeContext { reply } => {
                let _ = reply.send(Arc::clone(&state.runtime_context));
            }
            SessionCommand::GetParentSessionId { reply } => {
                let _ = reply.send(state.parent_session_id());
            }
            SessionCommand::GetTurnReservationSnapshot { reply } => {
                let _ = reply.send(TurnReservationSnapshot {
                    max_turns: state.max_turns,
                    active_turn: state.active_turn.clone(),
                    latest_turn: state.latest_turn.clone(),
                    pending_turn_queue: Arc::clone(&state.pending_turn_queue),
                    ephemeral: state.summary.ephemeral,
                    parent_session_id: state.parent_session_id(),
                    summary: state.summary.clone(),
                    runtime_context: Arc::clone(&state.runtime_context),
                });
            }
            SessionCommand::GetHookContextSnapshot { reply } => {
                let _ = reply.send(HookContextSnapshot {
                    runtime_context: Arc::clone(&state.runtime_context),
                    record: state.record.clone(),
                    summary: state.summary.clone(),
                    config: state.config.clone(),
                });
            }
            SessionCommand::GetTurnPersistenceSnapshot { reply } => {
                let _ = reply.send(TurnPersistenceSnapshot {
                    record: state.record.clone(),
                    session_context: state.core.session_context.clone(),
                    latest_turn_context: state.core.latest_turn_context.clone(),
                });
            }
            SessionCommand::GetShellExecContext { cwd, reply } => {
                let _ = &cwd;
                let tool_registry = state
                    .tool_registry
                    .clone()
                    .unwrap_or_else(|| Arc::clone(&state.runtime_context.registry));
                let _ = reply.send(ShellExecContextSnapshot {
                    permission_mode: state.core.config.permission_mode,
                    permission_profile: state.core.config.permission_profile.clone(),
                    runtime_context: Arc::clone(&state.runtime_context),
                    tool_registry,
                });
            }
            SessionCommand::GetTitleGenerationContext { reply } => {
                let _ = reply.send(TitleGenerationContext {
                    model_selection: session_model_selection(&state.summary).map(str::to_string),
                    reasoning_effort_selection: state.summary.reasoning_effort_selection.clone(),
                    title_state: state.summary.title_state.clone(),
                    runtime_context: Arc::clone(&state.runtime_context),
                });
            }
            SessionCommand::GetPendingQueueSnapshot { reply } => {
                let queue = state
                    .pending_turn_queue
                    .lock()
                    .expect("pending turn queue mutex should not be poisoned");
                let pending_texts: Vec<String> = queue
                    .iter()
                    .filter_map(|item| match &item.kind {
                        devo_core::PendingInputKind::UserText { text } => Some(text.clone()),
                        devo_core::PendingInputKind::UserInput { display_text, .. } => {
                            Some(display_text.clone())
                        }
                        _ => None,
                    })
                    .collect();
                let pending_count = pending_texts.len();
                let _ = reply.send(PendingQueueSnapshot {
                    pending_count,
                    pending_texts,
                });
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
                let popped = queue.pop_front().and_then(pop_queued_turn_input_data);
                let _ = reply.send(popped);
            }
            SessionCommand::GetActiveTurnId { reply } => {
                let _ = reply.send(state.active_turn.as_ref().map(|turn| turn.turn_id));
            }
            SessionCommand::GetRecord { reply } => {
                let _ = reply.send(state.record.clone());
            }
            SessionCommand::PreparePersistItem { turn_id, reply } => {
                let turn_kind = state
                    .active_turn
                    .as_ref()
                    .filter(|turn| turn.turn_id == turn_id)
                    .map(|turn| turn.kind.clone())
                    .or_else(|| {
                        state
                            .latest_turn
                            .as_ref()
                            .filter(|turn| turn.turn_id == turn_id)
                            .map(|turn| turn.kind.clone())
                    })
                    .unwrap_or_default();
                let _ = reply.send(super::snapshots::PersistItemPrep {
                    turn_kind,
                    record: state.record.clone(),
                });
            }
            SessionCommand::TakeShutdownDeferredSnapshot { reply } => {
                let stream = state.stream.lock().await;
                let _ = reply.send(super::snapshots::ShutdownDeferredSnapshot {
                    deferred_assistant: stream.deferred_assistant.clone(),
                    deferred_reasoning: stream.deferred_reasoning.clone(),
                    active_turn_id: state.active_turn.as_ref().map(|turn| turn.turn_id),
                    record: state.record.clone(),
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
            SessionCommand::ResetTurnApprovalCache => {
                state.turn_approval_cache = crate::execution::ApprovalGrantCache::default();
            }
            SessionCommand::TouchLastActivity => {
                state.summary.last_activity_at = state.summary.last_activity_at.max(Utc::now());
            }
            SessionCommand::ApplyApprovalScope { scope, pending } => {
                apply_approval_scope_to_state(&mut state, &scope, &pending);
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
                if matches!(state.summary.title_state, SessionTitleState::Final(_)) {
                    let _ = reply.send(None);
                    continue;
                }
                let updated_at = Utc::now();
                state.summary.title = Some(title.clone());
                state.summary.title_state = title_state.clone();
                state.summary.updated_at = updated_at;
                if let Some(record) = state.record.as_mut() {
                    record.title = Some(title);
                    record.title_state = title_state;
                    record.updated_at = updated_at;
                }
                let _ = reply.send(Some(state.summary.clone()));
            }
            SessionCommand::BeginActiveTurn { turn, turn_config } => {
                let now = Utc::now();
                apply_turn_config_to_session_summary(&mut state.summary, &turn_config);
                state.summary.status = SessionRuntimeStatus::ActiveTurn;
                state.summary.updated_at = now;
                state.summary.last_activity_at = now;
                state.active_turn = Some(turn);
            }
            SessionCommand::ClearActiveTurnIfMatches { turn_id, reply } => {
                let cleared = state
                    .active_turn
                    .as_ref()
                    .is_some_and(|active| active.turn_id == turn_id);
                if cleared {
                    state.active_turn = None;
                    state.summary.status = SessionRuntimeStatus::Idle;
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
                state.summary.status = SessionRuntimeStatus::Idle;
                state.summary.updated_at = now;
                state.summary.last_activity_at = now;
            }
            SessionCommand::SetActiveGoal { goal } => match goal {
                Some(goal) => state.core.set_active_goal(goal),
                None => state.core.clear_active_goal(),
            },
            SessionCommand::EnqueuePendingTurnInput { item } => {
                state
                    .pending_turn_queue
                    .lock()
                    .expect("pending turn queue mutex should not be poisoned")
                    .push_back(item);
            }
            SessionCommand::ActivateQueuedTurn { turn, turn_config } => {
                let now = Utc::now();
                apply_turn_config_to_session_summary(&mut state.summary, &turn_config);
                state.summary.status = SessionRuntimeStatus::ActiveTurn;
                state.summary.updated_at = now;
                state.summary.last_activity_at = now;
                state.active_turn = Some(turn);
            }
            SessionCommand::CompleteShellTurn {
                turn,
                is_error,
                reply,
            } => {
                let mut final_turn = turn;
                final_turn.completed_at = Some(Utc::now());
                final_turn.status = if is_error {
                    TurnStatus::Failed
                } else {
                    TurnStatus::Completed
                };
                state.latest_turn = Some(final_turn.clone());
                state.active_turn = None;
                state.summary.status = SessionRuntimeStatus::Idle;
                state.summary.updated_at = Utc::now();
                state.summary.last_activity_at = state.summary.updated_at;
                let _ = reply.send(final_turn);
            }
            SessionCommand::UpdateCorePermissionMode { permission_mode } => {
                state.core.config.permission_mode = permission_mode;
            }
            SessionCommand::UpdateRecordRolloutPath { rollout_path } => {
                if let Some(record) = state.record.as_mut() {
                    record.rollout_path = rollout_path;
                }
            }
            SessionCommand::ApplyParentUsageSnapshot { snapshot } => {
                snapshot.apply_to_actor_state(&mut state);
            }
            SessionCommand::InterruptActiveTurn { reply } => {
                let now = Utc::now();
                state.summary.status = SessionRuntimeStatus::Idle;
                state.summary.updated_at = now;
                state.summary.last_activity_at = now;
                state.summary.total_input_tokens = state.core.total_input_tokens;
                state.summary.total_output_tokens = state.core.total_output_tokens;
                state.summary.total_tokens = state.core.total_tokens;
                state.summary.total_cache_creation_tokens = state.core.total_cache_creation_tokens;
                state.summary.total_cache_read_tokens = state.core.total_cache_read_tokens;
                state.summary.prompt_token_estimate = state.core.prompt_token_estimate;
                let interrupted = state.active_turn.take().map(|mut turn| {
                    turn.status = TurnStatus::Interrupted;
                    turn.completed_at = Some(now);
                    state.latest_turn = Some(turn.clone());
                    turn
                });
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
            }
            SessionCommand::EnqueueBtwInput { item } => {
                state
                    .btw_input_queue
                    .lock()
                    .expect("btw input queue mutex should not be poisoned")
                    .push_back(item);
            }
            SessionCommand::UpdateSessionMetadata {
                model,
                model_binding_id,
                reasoning_effort_selection,
                reply,
            } => {
                let updated_at = Utc::now();
                state.summary.model = model.clone();
                state.summary.model_binding_id = model_binding_id.clone();
                state.summary.reasoning_effort_selection = reasoning_effort_selection.clone();
                state.summary.updated_at = updated_at;
                if let Some(record) = state.record.as_mut() {
                    record.model = model;
                    record.model_binding_id = model_binding_id;
                    record.reasoning_effort_selection = reasoning_effort_selection;
                    record.updated_at = updated_at;
                }
                let _ = reply.send(state.summary.clone());
            }
            SessionCommand::ApplyPermissionProfile { profile, reply } => {
                state.core.config.permission_mode = profile.permission_mode();
                state.core.config.permission_profile = profile.clone();
                state.config.permission_mode = profile.permission_mode();
                state.config.permission_profile = profile;
                state.session_approval_cache = crate::execution::ApprovalGrantCache::default();
                state.turn_approval_cache = crate::execution::ApprovalGrantCache::default();
                let _ = reply.send(());
            }
            SessionCommand::SetSessionTitleUserRename { title, reply } => {
                let updated_at = Utc::now();
                state.summary.title = Some(title.clone());
                state.summary.title_state =
                    SessionTitleState::Final(SessionTitleFinalSource::UserRename);
                state.summary.updated_at = updated_at;
                if let Some(record) = state.record.as_mut() {
                    record.title = Some(title);
                    record.title_state =
                        SessionTitleState::Final(SessionTitleFinalSource::UserRename);
                    record.updated_at = updated_at;
                }
                let _ = reply.send(state.summary.clone());
            }
            SessionCommand::SetToolRegistry {
                tool_registry,
                reply,
            } => {
                state.tool_registry = tool_registry;
                let _ = reply.send(());
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
                state.summary.status = SessionRuntimeStatus::ActiveTurn;
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
            SessionCommand::Shutdown { reply } => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

fn apply_turn_config_to_session_summary(
    summary: &mut crate::session::SessionMetadata,
    turn_config: &TurnConfig,
) {
    summary.model = Some(turn_config.model.slug.clone());
    summary.model_binding_id = turn_config.model_binding_id.clone();
    summary.reasoning_effort_selection = turn_config.reasoning_effort_selection.clone();
}

fn pop_queued_turn_input_data(
    item: devo_protocol::PendingInputItem,
) -> Option<QueuedTurnInputData> {
    match item.kind {
        devo_core::PendingInputKind::UserText { text } => Some(QueuedTurnInputData {
            display_input: text.clone(),
            input_text: text,
            input_messages: Vec::new(),
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
            ..
        } => Some(QueuedTurnInputData {
            display_input: display_text,
            input_text: prompt_text,
            input_messages: prompt_messages,
            collaboration_mode: collaboration_mode_from_pending_metadata(item.metadata.as_ref()),
            model_selection: model_selection_from_pending_metadata(item.metadata.as_ref()),
            subagent_usage_owner: subagent_usage_owner_from_pending_metadata(
                item.metadata.as_ref(),
            ),
        }),
        _ => None,
    }
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
) -> Option<(devo_protocol::SessionId, Option<devo_core::TurnId>)> {
    let parent_session_id =
        string_field_from_pending_metadata(metadata, "devo_subagent_usage_parent_session_id")
            .and_then(|value| devo_protocol::SessionId::try_from(value).ok())?;
    let parent_turn_id =
        string_field_from_pending_metadata(metadata, "devo_subagent_usage_parent_turn_id")
            .and_then(|value| devo_core::TurnId::try_from(value).ok());
    Some((parent_session_id, parent_turn_id))
}

fn apply_approval_scope_to_state(
    state: &mut SessionActorState,
    scope: &ApprovalScopeValue,
    pending: &PendingApproval,
) {
    match scope {
        ApprovalScopeValue::Once => {}
        ApprovalScopeValue::Turn => {
            state
                .turn_approval_cache
                .tools
                .insert(pending.tool_name.clone());
        }
        ApprovalScopeValue::Session => {
            state
                .session_approval_cache
                .tools
                .insert(pending.tool_name.clone());
        }
        ApprovalScopeValue::PathPrefix => {
            if let Some(path) = pending.path.clone() {
                state.turn_approval_cache.path_prefixes.insert(path);
            }
        }
        ApprovalScopeValue::Host => {
            if let Some(host) = pending.host.clone() {
                state.turn_approval_cache.hosts.insert(host);
            }
        }
        ApprovalScopeValue::Tool => {
            state
                .turn_approval_cache
                .tools
                .insert(pending.tool_name.clone());
        }
        ApprovalScopeValue::CommandPrefix => {
            if let Some(command_prefix) = pending.command_prefix.clone() {
                state
                    .session_approval_cache
                    .command_prefixes
                    .insert(command_prefix);
            }
        }
    }
}
