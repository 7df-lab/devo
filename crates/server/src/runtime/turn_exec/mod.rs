mod approval_resume;
mod context_compaction;
mod event_stream;
mod failure;
mod finalize;
mod followup;
mod item_stream;
pub(crate) mod journal;
mod query;
mod recovery;
mod recovery_notifications;
mod tool_display;
mod tool_results;
mod trace;
mod types;

pub(crate) use context_compaction::{
    manual_compaction_completed_event, manual_compaction_item_failed_event,
    manual_compaction_started_event,
};
pub(crate) use event_stream::{QUERY_EVENT_CHANNEL_CAPACITY, spawn_turn_event_stream};
pub(crate) use finalize::FinalizeTurnParams;
pub(crate) use query::TurnModelQueryParams;
pub(crate) use types::ExecuteTurnRequest;

use std::sync::Arc;

use anyhow::Context;
use devo_protocol::native::ids::SessionId;

use super::*;

/// Schedules post-`MergeTurn` work in fixed order:
/// compact → refine apply / auto-schedule → post-compact continue → queue → goal → title.
///
/// Must stay a sync function so callers' async opaque types do not recursively
/// include this spawn's future (rustc Send-cycle with `execute_turn`).
pub(crate) fn spawn_post_turn_scheduling(
    runtime: Arc<ServerRuntime>,
    session_id: SessionId,
    should_auto_continue_goal: bool,
) {
    tokio::spawn(async move {
        // Provider/program failures mark recovery Available (no typed
        // failure_reason). Still pause the goal before bailing — otherwise
        // recovery gating would leave Active goals looping eligibility checks.
        if should_auto_continue_goal && let Some(session_handle) = runtime.session(session_id).await
        {
            let _ = runtime
                .pause_goal_continuation_after_failed_turn(session_id, &session_handle)
                .await;
        }
        if runtime
            .turn_recovery(session_id)
            .await
            .ok()
            .flatten()
            .is_some()
        {
            return;
        }

        // (1) Compaction first — agent-requested pending compact (schedule-only
        // mid-turn). Execute before refine so order stays compact → refine → ….
        // In-turn auto-compact already ran inside `query` before MergeTurn.
        let native_session_id = if let Some(handle) = runtime.session(session_id).await
            && let Some(summary) = handle.summary().await
        {
            summary.native.id
        } else {
            // boundary: legacy session id when summary unavailable
            session_id
        };
        if let Some(pending) = super::compact_host::take_pending_compact(&native_session_id) {
            runtime
                .execute_agent_requested_compaction(session_id, pending.instructions)
                .await;
        }

        // Session facts for refine / auto-interval.
        let Some(handle) = runtime.session(session_id).await else {
            return;
        };
        let is_root = handle
            .summary()
            .await
            .is_none_or(|s| s.agent_path.is_none());
        let plan_mode = handle
            .collaboration_mode()
            .await
            .is_some_and(|m| m == devo_protocol::CollaborationMode::Plan);
        let session_dir =
            handle.rollout_path().await.flatten().and_then(|path| {
                crate::persistence::RolloutStore::rlm_session_dir_for_rollout(&path)
            });
        let turn_succeeded = handle
            .resume_snapshot()
            .await
            .and_then(|snap| snap.latest_turn)
            .is_some_and(|t| {
                matches!(
                    t.native.status,
                    devo_protocol::native::turn::TurnStatus::Completed
                )
            });
        // Goal-continuation turns must not count toward auto-refine (DD-3).
        // Thread TurnInputMode through post-turn when goal turns are marked distinctly.
        let is_goal_continuation = false;

        // (2) Apply pending refine (never mid-ipython) then maybe schedule auto.
        if let Some(dir) = session_dir.as_deref() {
            if let Some(applied) = runtime
                .apply_pending_refine_at_boundary(session_id, &native_session_id, dir, plan_mode)
                .await
            {
                runtime
                    .persist_applied_refinement(session_id, &applied)
                    .await;
            }
            let auto_settings = runtime
                .native_session_snapshot(session_id)
                .await
                .map(|session| {
                    super::refine::auto_refine_settings_from_session(
                        session.settings.auto_refine_enabled,
                        session.settings.auto_refine_turn_interval,
                    )
                })
                .unwrap_or_default();
            let _ = super::refine::maybe_schedule_auto_refine(
                &native_session_id,
                &auto_settings,
                is_root,
                is_goal_continuation,
                turn_succeeded && !plan_mode,
                Some(dir),
            );
        }

        // (3) Post-compact continue — reserved for P4 when compact owns retry.
        // (3b) Async bash/python completion follow-ups (idle wake).
        runtime
            .drain_async_tool_completion_notices(session_id)
            .await;
        // (4) Queue drain
        {
            let pending = handle
                .pending_queue_snapshot()
                .await
                .map(|snapshot| snapshot.pending_count)
                .unwrap_or_default();
            tracing::debug!(session_id = %session_id, pending, "post-turn queue check");
        }
        if runtime.chain_queued_followup_turn(session_id).await {
            return;
        }
        if runtime.spawn_next_turn_from_queue(session_id).await {
            return;
        }
        if runtime.child_parent_and_path(session_id).await.is_some()
            && runtime.child_can_accept_next_turn(session_id).await
        {
            let _ = runtime
                .drain_child_mailbox_into_user_turns(session_id)
                .await;
            return;
        }
        // (5) Goal continuation
        if should_auto_continue_goal {
            runtime.maybe_start_goal_continuation_turn(session_id).await;
        }
        // (6) Title polish last (idle-only auxiliary)
        runtime.notify_title_polish(session_id).await;
    });
}

impl ServerRuntime {
    /// Execute one turn on a spawned working copy; the session actor stays free.
    pub(in crate::runtime) async fn execute_turn(self: Arc<Self>, request: ExecuteTurnRequest) {
        let Some(handle) = self.session(request.session_id).await else {
            return;
        };
        handle.execute_turn(Arc::clone(&self), request).await;
    }

    pub(crate) async fn persist_turn_line_deduped(
        self: &Arc<Self>,
        session_id: devo_core::SessionId,
        turn: &crate::turn::RuntimeTurn,
    ) -> anyhow::Result<()> {
        let handle = self
            .session(session_id)
            .await
            .context("session not found")?;
        handle
            .persist_turn_line(Arc::clone(self), turn.clone())
            .await
    }

    pub(crate) async fn persist_runtime_turn_line_deduped(
        self: &Arc<Self>,
        session_id: devo_core::SessionId,
        turn: &crate::turn::RuntimeTurn,
    ) -> anyhow::Result<()> {
        self.persist_turn_line_deduped(session_id, turn).await
    }

    pub(super) async fn prepare_turn_execution_for_actor(
        self: &Arc<Self>,
        state: &mut SessionActorState,
        turn: &crate::turn::RuntimeTurn,
        display_input: &str,
        input_image_paths: &[std::path::PathBuf],
        emits_user_message: bool,
        client_user_message_id: Option<&str>,
    ) {
        state.turn_approval_cache = crate::execution::ApprovalGrantCache::default();
        // Emit the visible user bubble before workspace baseline capture so the
        // TUI is not blocked on git I/O (L2-DES-APP-010 first-party latency).
        if emits_user_message {
            let user_message = if let Some(client_user_message_id) = client_user_message_id {
                crate::runtime::items::native_user_message_item_with_client_message_id(
                    display_input.to_string(),
                    input_image_paths,
                    devo_protocol::native::item::UserMessageEntry::Queue,
                    client_user_message_id.to_string(),
                )
            } else {
                crate::runtime::items::native_user_message_item(
                    display_input.to_string(),
                    input_image_paths,
                    devo_protocol::native::item::UserMessageEntry::Queue,
                )
            };
            self.emit_turn_native_item(
                state.summary.native.id,
                *turn.native_turn_id(),
                user_message,
            )
            .await;
        }
        self.capture_turn_workspace_baseline(
            state.session_id(),
            turn.turn_id(),
            state.summary.cwd.clone(),
        )
        .await;
    }

    pub(in crate::runtime) fn tool_registry_for_actor_state(
        &self,
        state: &SessionActorState,
    ) -> Arc<devo_core::tools::ToolRegistry> {
        state
            .tool_registry
            .clone()
            .unwrap_or_else(|| state.runtime_context.tool_registry())
    }
}

#[cfg(test)]
mod tests;
