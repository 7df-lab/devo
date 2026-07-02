use super::super::*;

impl ServerRuntime {
    pub(crate) async fn handle_workspace_changes_read(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: WorkspaceChangesReadParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid workspace/changes/read params: {error}"),
                );
            }
        };
        if params.scopes.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "workspace/changes/read requires at least one scope",
            );
        }

        let Some(session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let Some(reservation) = session_handle.turn_reservation_snapshot().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let cwd = params
            .cwd
            .clone()
            .unwrap_or_else(|| reservation.summary.cwd.clone());
        let active_turn_id = reservation.active_turn.as_ref().map(|turn| turn.turn_id);
        let latest_turn_id = reservation.latest_turn.as_ref().map(|turn| turn.turn_id);

        let mut views = Vec::with_capacity(params.scopes.len());
        for scope in params.scopes {
            let view = match scope {
                WorkspaceChangeScope::Branch => {
                    crate::workspace_changes::branch_view(
                        cwd.clone(),
                        params.base_branch.clone(),
                        params.diff_detail,
                        params.max_diff_bytes,
                    )
                    .await
                }
                WorkspaceChangeScope::Uncommitted => {
                    crate::workspace_changes::uncommitted_view(
                        cwd.clone(),
                        params.diff_detail,
                        params.max_diff_bytes,
                    )
                    .await
                }
                WorkspaceChangeScope::Turn => {
                    let turn_id = params.turn_id.or(active_turn_id).or(latest_turn_id);
                    match turn_id {
                        Some(turn_id) => {
                            self.read_turn_workspace_changes(
                                params.session_id,
                                turn_id,
                                cwd.clone(),
                                params.diff_detail,
                                params.max_diff_bytes,
                            )
                            .await
                        }
                        None => crate::workspace_changes::unsupported_view(
                            WorkspaceChangeScope::Turn,
                            cwd.clone(),
                            WorkspaceChangeAttribution::WorkspaceNet,
                            "turn_id_not_available",
                        ),
                    }
                }
            };
            views.push(view);
        }

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: WorkspaceChangesReadResult { views },
        })
        .expect("serialize workspace/changes/read response")
    }

    async fn read_turn_workspace_changes(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        cwd: PathBuf,
        diff_detail: WorkspaceDiffDetail,
        max_diff_bytes: Option<u64>,
    ) -> WorkspaceChangeView {
        if let Some(baseline) = self
            .active_workspace_baselines
            .lock()
            .await
            .get(&turn_id)
            .cloned()
        {
            return match crate::workspace_changes::read_active_turn_view(
                baseline,
                diff_detail,
                max_diff_bytes,
            )
            .await
            {
                Ok(view) => view,
                Err(error) => crate::workspace_changes::error_view(
                    WorkspaceChangeScope::Turn,
                    cwd,
                    WorkspaceChangeAttribution::WorkspaceNet,
                    error.to_string(),
                ),
            };
        }

        match crate::workspace_changes::read_finalized_turn_view(
            self.metadata.server_home.as_path(),
            session_id,
            turn_id,
            diff_detail,
            max_diff_bytes,
        ) {
            Ok(Some(view)) => view,
            Ok(None) => crate::workspace_changes::unsupported_view(
                WorkspaceChangeScope::Turn,
                cwd,
                WorkspaceChangeAttribution::WorkspaceNet,
                "turn_baseline_not_available",
            ),
            Err(error) => crate::workspace_changes::error_view(
                WorkspaceChangeScope::Turn,
                cwd,
                WorkspaceChangeAttribution::WorkspaceNet,
                error.to_string(),
            ),
        }
    }
}
