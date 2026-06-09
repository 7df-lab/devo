//! Server-owned autonomous continuation for active thread goals.
//!
//! The TUI persists goal state through `goal/set`; this module decides when an
//! active goal is eligible to become an internal Build turn and launches that
//! turn without adding a synthetic user message to the transcript.

use super::*;
use futures::future::BoxFuture;

struct GoalContinuationCandidate {
    goal_id: GoalId,
    goal: devo_protocol::ThreadGoal,
}

impl ServerRuntime {
    pub(super) fn maybe_start_goal_continuation_turn(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
                return;
            };
            if !session_allows_goal_continuation(&session_arc).await {
                return;
            }
            let Some(candidate) = self.goal_continuation_candidate(session_id).await else {
                return;
            };
            if !session_allows_goal_continuation(&session_arc).await {
                return;
            }

            let (turn_config, resolved_request) = {
                let session = session_arc.lock().await;
                let requested_model = session.summary.model.as_deref();
                let requested_thinking = session.summary.thinking.clone();
                let turn_config = self
                    .deps
                    .resolve_turn_config(requested_model, requested_thinking);
                let resolved_request = turn_config
                    .model
                    .resolve_thinking_selection(turn_config.thinking_selection.as_deref());
                (turn_config, resolved_request)
            };
            let request_model = turn_config.provider_request_model(&resolved_request.request_model);

            let now = Utc::now();
            let turn = {
                let mut session = session_arc.lock().await;
                if !session_allows_goal_continuation_locked(&session) {
                    return;
                }
                let turn = TurnMetadata {
                    turn_id: TurnId::new(),
                    session_id,
                    sequence: session
                        .latest_turn
                        .as_ref()
                        .map_or(1, |turn| turn.sequence + 1),
                    status: TurnStatus::Running,
                    kind: devo_core::TurnKind::Regular,
                    model: turn_config.model.slug.clone(),
                    thinking: turn_config.thinking_selection.clone(),
                    reasoning_effort: resolved_request.effective_reasoning_effort,
                    request_model,
                    request_thinking: resolved_request.request_thinking.clone(),
                    started_at: now,
                    completed_at: None,
                    usage: None,
                };
                session.summary.status = SessionRuntimeStatus::ActiveTurn;
                session.summary.updated_at = now;
                session.summary.model = Some(turn_config.model.slug.clone());
                session.summary.thinking = turn_config.thinking_selection.clone();
                session.active_turn = Some(turn.clone());
                turn
            };
            if !self
                .mark_goal_continuation_turn_started(session_id, &candidate.goal_id)
                .await
            {
                self.clear_goal_continuation_turn_reservation(&session_arc, turn.turn_id)
                    .await;
                return;
            }

            if let Err(error) = self
                .append_goal_continuation_turn_start(session_id, &turn)
                .await
            {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn.turn_id,
                    error = %error,
                    "failed to persist goal continuation turn start"
                );
                self.clear_goal_continuation_turn_reservation(&session_arc, turn.turn_id)
                    .await;
                return;
            }

            self.broadcast_event(ServerEvent::SessionStatusChanged(
                SessionStatusChangedPayload {
                    session_id,
                    status: SessionRuntimeStatus::ActiveTurn,
                },
            ))
            .await;
            self.broadcast_event(ServerEvent::TurnStarted(TurnEventPayload {
                session_id,
                turn: turn.clone(),
            }))
            .await;

            let cancel_token = CancellationToken::new();
            self.active_turn_cancellations
                .lock()
                .await
                .insert(session_id, cancel_token);
            let runtime = Arc::clone(self);
            let task_turn = turn.clone();
            let task_turn_config = turn_config.clone();
            let task = tokio::spawn(async move {
                runtime
                    .execute_turn(
                        session_id,
                        task_turn,
                        task_turn_config,
                        String::new(),
                        String::new(),
                        devo_protocol::CollaborationMode::Build,
                        TurnInputMode::HiddenGoalContinuation {
                            goal: candidate.goal,
                        },
                    )
                    .await;
            });
            self.active_tasks
                .lock()
                .await
                .insert(session_id, task.abort_handle());
        })
    }

    async fn goal_continuation_candidate(
        &self,
        session_id: SessionId,
    ) -> Option<GoalContinuationCandidate> {
        let stores = self.goal_stores.lock().await;
        let goal = stores.get(&session_id)?.get()?;
        if !goal.check_continuation().should_continue {
            return None;
        }
        Some(GoalContinuationCandidate {
            goal_id: goal.goal_id.clone(),
            goal: goal.to_thread_goal(),
        })
    }

    async fn append_goal_continuation_turn_start(
        &self,
        session_id: SessionId,
        turn: &TurnMetadata,
    ) -> anyhow::Result<()> {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return Ok(());
        };
        let (record, session_context, turn_context) = {
            let session = session_arc.lock().await;
            let core_session = session.core_session.lock().await;
            (
                session.record.clone(),
                core_session.session_context.clone(),
                core_session.latest_turn_context.clone(),
            )
        };
        if let Some(record) = record {
            self.rollout_store.append_turn(
                &record,
                build_turn_record(turn, session_context, turn_context),
            )?;
        }
        Ok(())
    }

    async fn mark_goal_continuation_turn_started(
        &self,
        session_id: SessionId,
        goal_id: &GoalId,
    ) -> bool {
        let mut stores = self.goal_stores.lock().await;
        stores
            .get_mut(&session_id)
            .and_then(GoalStore::get_mut)
            .filter(|goal| &goal.goal_id == goal_id)
            .filter(|goal| goal.check_continuation().should_continue)
            .map(|goal| {
                goal.usage.record_turn();
                true
            })
            .unwrap_or(false)
    }

    async fn clear_goal_continuation_turn_reservation(
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
        }
    }
}

async fn session_allows_goal_continuation(session_arc: &Arc<Mutex<RuntimeSession>>) -> bool {
    let session = session_arc.lock().await;
    session_allows_goal_continuation_locked(&session)
}

fn session_allows_goal_continuation_locked(session: &RuntimeSession) -> bool {
    if session.active_turn.is_some()
        || !session.pending_approvals.is_empty()
        || !session.pending_user_inputs.is_empty()
    {
        return false;
    }
    session
        .pending_turn_queue
        .lock()
        .expect("pending turn queue mutex should not be poisoned")
        .is_empty()
}
