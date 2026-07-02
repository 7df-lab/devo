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
                let record = self.session_record_snapshot(session_id).await;
                if let Some(record) = record
                    && let Err(error) = self
                        .rollout_store
                        .append_workspace_checkpoint_recorded(&record, captured.record)
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
        turn: &TurnMetadata,
    ) {
        let Some(baseline) = self
            .active_workspace_baselines
            .lock()
            .await
            .remove(&turn.turn_id)
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
                let record = self.session_record_snapshot(session_id).await;
                if let Some(record) = record
                    && let Err(error) = self
                        .rollout_store
                        .append_workspace_change_recorded(&record, finalized.record)
                {
                    tracing::warn!(
                        session_id = %session_id,
                        turn_id = %turn.turn_id,
                        error = %error,
                        "failed to persist workspace change record"
                    );
                }
                self.broadcast_event(ServerEvent::WorkspaceChangesUpdated(
                    WorkspaceChangesUpdatedPayload {
                        session_id,
                        turn_id: turn.turn_id,
                        scope: WorkspaceChangeScope::Turn,
                        status: finalized.view.status,
                        coverage: finalized.view.coverage,
                        change_set_status: finalized.view.change_set_status,
                        stats: finalized.view.stats,
                        version: Utc::now().timestamp_millis().max(0) as u64,
                        generated_at: Utc::now(),
                    },
                ))
                .await;
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn.turn_id,
                    error = %error,
                    "failed to finalize workspace changes"
                );
            }
        }
    }
}
