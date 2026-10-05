//! `session/tree/read` and `session/tree/navigate` — in-session transcript tree.

use chrono::Utc;
use devo_core::{build_session_tree, resolve_parent_map};
use devo_protocol::native::ids::{ItemId, TurnId};
use devo_protocol::native::item::{Item, ItemEnvelope, ItemState, UserInput};
use devo_protocol::native::rpc_session::{
    SessionTreeNavigateParams, SessionTreeNavigateResult, SessionTreeReadParams,
    SessionTreeReadResult,
};
use devo_protocol::native::wire_projector::typed_item_envelope;

use super::super::*;

impl ServerRuntime {
    pub(crate) async fn handle_session_tree_read(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionTreeReadParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/tree/read params: {error}"),
                );
            }
        };
        let history = match self
            .load_canonical_history(&request_id, params.session_id)
            .await
        {
            Ok(history) => history,
            Err(response) => return response,
        };
        let (tree, leaf_id) = build_session_tree(&history);
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionTreeReadResult { tree, leaf_id },
        })
        .expect("serialize session/tree/read response")
    }

    pub(crate) async fn handle_session_tree_navigate(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionTreeNavigateParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/tree/navigate params: {error}"),
                );
            }
        };

        if let Some(handle) = self.session(params.session_id).await
            && handle.active_turn_id().await.flatten().is_some()
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ActiveTurnEditRejected,
                "cannot navigate the session tree while a turn is active",
            );
        }

        let history = match self
            .load_canonical_history(&request_id, params.session_id)
            .await
        {
            Ok(history) => history,
            Err(response) => return response,
        };

        let parents = resolve_parent_map(&history);
        let Some(target) = history.items.iter().find(|item| item.id == params.entry_id) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("tree entry not found: {}", params.entry_id),
            );
        };

        let (new_leaf, editor_text) = match &target.item {
            Item::UserMessage { content, .. } => {
                let parent = parents.get(&target.id).cloned().flatten();
                let text = content
                    .iter()
                    .filter_map(|part| match part {
                        UserInput::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                (parent, Some(text))
            }
            _ => (Some(target.id), None),
        };

        let Some(rollout_path) = self.resolve_rollout_path(&params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        let epoch = history.leaf_epoch.saturating_add(1);
        if let Err(error) = self.rollout_store.append_session_leaf_at(
            &rollout_path,
            params.session_id,
            new_leaf,
            epoch,
        ) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist session leaf: {error}"),
            );
        }

        let mut summary_item = None;
        let mut leaf_id = new_leaf;
        let mut leaf_epoch = epoch;
        if params.summarize.unwrap_or(false) {
            match self
                .append_branch_summary_item(
                    &params.session_id,
                    &rollout_path,
                    new_leaf,
                    params.custom_instructions.as_deref(),
                    params.replace_instructions.unwrap_or(false),
                    epoch,
                )
                .await
            {
                Ok(item) => {
                    leaf_id = Some(item.id);
                    leaf_epoch = epoch.saturating_add(1);
                    summary_item = Some(item);
                }
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        error,
                    );
                }
            }
        }

        // The live actor's in-memory journal keeps abandoned branches; the
        // durable leaf only fixes parenting and fresh reads. Re-scope the
        // actor's items to the navigated tip so the next turn's model context
        // matches what a resume from this rollout would rebuild (falls back
        // to the linear list when the journal lags the actor).
        let tip = match new_leaf {
            Some(item_id) => super::session::ActivePathTip::At(item_id),
            None => super::session::ActivePathTip::Root,
        };
        let scoped_item_ids = match self.session(params.session_id).await {
            Some(handle) => match handle.transcript_items().await {
                Some(items) => {
                    let mut scoped = super::session::filter_items_to_active_path(
                        &items, &history, tip,
                    )
                    .map(|scoped| scoped.iter().map(|item| item.item_id).collect::<Vec<_>>());
                    // A branch summary (when requested) becomes the new
                    // tip after the navigated entry; keep it in scope.
                    if let (Some(summary), Some(ids)) = (&summary_item, &mut scoped) {
                        ids.push(summary.id);
                    }
                    scoped
                }
                None => None,
            },
            None => None,
        };

        // Keep the live actor tip in sync so the next turn parents correctly.
        // The rollout's latest compaction snapshot rides along so a navigate
        // that keeps the summary on-branch also keeps the compacted prompt.
        if let Some(handle) = self.session(params.session_id).await {
            let compaction_snapshot =
                crate::persistence::latest_compaction_snapshot_from_rollout(&rollout_path);
            handle
                .set_transcript_leaf(leaf_id, leaf_epoch, scoped_item_ids, compaction_snapshot)
                .await;
        }

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionTreeNavigateResult {
                leaf_id,
                editor_text,
                cancelled: false,
                aborted: None,
                summary_item,
            },
        })
        .expect("serialize session/tree/navigate response")
    }

    async fn append_branch_summary_item(
        &self,
        session_id: &devo_protocol::native::ids::SessionId,
        rollout_path: &std::path::Path,
        parent_leaf: Option<ItemId>,
        custom_instructions: Option<&str>,
        replace_instructions: bool,
        leaf_epoch: u64,
    ) -> Result<ItemEnvelope, String> {
        let summary = if replace_instructions {
            custom_instructions
                .unwrap_or("Branched from previous path.")
                .to_string()
        } else if let Some(extra) = custom_instructions {
            format!("Branched from previous path.\n{extra}")
        } else {
            "Branched from previous path.".to_string()
        };

        let turn_id = TurnId::new();
        let item_id = ItemId::new();
        let item_seq = self.allocate_item_sequence(session_id).await;
        let native = Item::BranchSummary {
            summary,
            details: None,
        };
        let mut envelope = typed_item_envelope(
            *session_id,
            turn_id,
            item_id,
            item_seq,
            &native,
            ItemState::Completed,
            Utc::now(),
            None,
        );
        envelope.parent_id = parent_leaf;

        self.rollout_store
            .append_tree_edge_at(rollout_path, *session_id, item_id, parent_leaf)
            .map_err(|error| format!("failed to persist tree edge: {error}"))?;
        self.rollout_store
            .append_canonical_item_at(rollout_path, envelope.clone())
            .map_err(|error| format!("failed to persist branch summary: {error}"))?;
        self.rollout_store
            .append_session_leaf_at(
                rollout_path,
                *session_id,
                Some(item_id),
                leaf_epoch.saturating_add(1),
            )
            .map_err(|error| format!("failed to persist leaf after summary: {error}"))?;
        Ok(envelope)
    }
}
