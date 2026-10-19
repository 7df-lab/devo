use super::*;

impl ServerRuntime {
    pub(super) async fn capture_turn_workspace_baseline(
        self: &Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
        cwd: PathBuf,
    ) {
        match crate::workspace_changes::capture_baseline(
            self.metadata.server_home.clone(),
            session_id,
            turn_id,
            cwd,
        )
        .await
        {
            Ok(captured) => {
                self.active_workspace_baselines
                    .lock()
                    .await
                    .insert(turn_id, captured.baseline);
                let rollout_path = self.session_rollout_path(session_id).await;
                if let Some(rollout_path) = rollout_path
                    && let Err(error) = self
                        .rollout_store
                        .append_workspace_checkpoint_recorded_at(&rollout_path, captured.record)
                {
                    tracing::warn!(
                        session_id = %session_id,
                        turn_id = %turn_id,
                        error = %error,
                        "failed to persist workspace checkpoint record"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn_id,
                    error = %error,
                    "failed to capture workspace baseline"
                );
            }
        }
    }

    pub(super) async fn finalize_turn_workspace_changes(
        self: &Arc<Self>,
        session_id: SessionId,
        turn: &crate::turn::RuntimeTurn,
    ) {
        let turn_id = turn.turn_id();
        let Some(baseline) = self
            .active_workspace_baselines
            .lock()
            .await
            .remove(&turn_id)
        else {
            return;
        };
        match crate::workspace_changes::finalize_baseline(
            self.metadata.server_home.clone(),
            baseline,
        )
        .await
        {
            Ok(finalized) => {
                let rollout_path = self.session_rollout_path(session_id).await;
                if let Some(rollout_path) = rollout_path
                    && let Err(error) = self
                        .rollout_store
                        .append_workspace_change_recorded_at(&rollout_path, finalized.record)
                {
                    tracing::warn!(
                        session_id = %session_id,
                        turn_id = %turn_id,
                        error = %error,
                        "failed to persist workspace change record"
                    );
                }
                self.broadcast_notification(
                    devo_protocol::native::event::ServerNotification::WorkspaceChangesUpdated(
                        devo_protocol::native::event::WorkspaceChangesUpdatedNotification {
                            session_id: turn.native.session_id,
                            turn_id: turn.native.id,
                            scope: WorkspaceChangeScope::Turn,
                            status: finalized.view.status,
                            coverage: finalized.view.coverage,
                            change_set_status: finalized.view.change_set_status,
                            stats: devo_protocol::native::event::WorkspaceChangeStatsNotification {
                                files_changed: finalized.view.stats.files_changed,
                                additions: finalized.view.stats.additions,
                                deletions: finalized.view.stats.deletions,
                            },
                            version: Utc::now().timestamp_millis().max(0) as u64,
                            generated_at: Utc::now(),
                        },
                    ),
                )
                .await;
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn_id,
                    error = %error,
                    "failed to finalize workspace changes"
                );
            }
        }
    }

    /// Push an Accumulating turn-scoped workspace summary after a mutating tool.
    ///
    /// Clients re-read `workspace/changes/read` for file rows; this notify is the
    /// wake-up so TUI/Desktop can surface diffs as soon as the tool completes.
    pub(super) async fn notify_accumulating_turn_workspace_changes(
        self: &Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
    ) {
        let baseline = {
            let baselines = self.active_workspace_baselines.lock().await;
            baselines.get(&turn_id).cloned()
        };
        let Some(baseline) = baseline else {
            return;
        };
        match crate::workspace_changes::read_active_turn_view(
            baseline,
            WorkspaceDiffDetail::Summary,
            None,
        )
        .await
        {
            Ok(view) => {
                if view.stats.files_changed == 0
                    && view.stats.additions == 0
                    && view.stats.deletions == 0
                {
                    return;
                }
                self.broadcast_notification(
                    devo_protocol::native::event::ServerNotification::WorkspaceChangesUpdated(
                        devo_protocol::native::event::WorkspaceChangesUpdatedNotification {
                            session_id,
                            turn_id,
                            scope: WorkspaceChangeScope::Turn,
                            status: view.status,
                            coverage: view.coverage,
                            change_set_status: view.change_set_status,
                            stats: devo_protocol::native::event::WorkspaceChangeStatsNotification {
                                files_changed: view.stats.files_changed,
                                additions: view.stats.additions,
                                deletions: view.stats.deletions,
                            },
                            version: Utc::now().timestamp_millis().max(0) as u64,
                            generated_at: Utc::now(),
                        },
                    ),
                )
                .await;
            }
            Err(error) => {
                tracing::debug!(
                    session_id = %session_id,
                    turn_id = %turn_id,
                    error = %error,
                    "skip accumulating workspace notify"
                );
            }
        }
    }
}
