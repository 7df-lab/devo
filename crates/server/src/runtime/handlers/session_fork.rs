//! Durable user-session fork: self-contained child rollout (Codex-aligned).
//!
//! User forks copy kept history into a new rollout and record lineage on
//! `fork_from_id` / `fork_at_turn_id`. Sub-agent parentage stays on
//! `parent_session_id`.

use std::collections::HashSet;
use std::path::Path;

use chrono::Utc;
use devo_protocol::native::item::ItemState;
use devo_protocol::native::wire_projector::typed_item_envelope;

use super::super::*;
use crate::execution::PersistedTurnItem;
use crate::replay_hydrate::ReplayedTurn;
use crate::runtime::handlers::session::RuntimeSessionTurnCutOptions;
use devo_protocol::native::rpc_session::RollbackMode;
use devo_protocol::native::rpc_session::SessionForkCut;

/// Options for creating a durable user fork.
pub(crate) struct DurableForkOptions {
    pub(crate) source_session_id: SessionId,
    pub(crate) fork_at_turn_id: Option<TurnId>,
    pub(crate) user_turn_index: Option<u32>,
    pub(crate) cut: SessionForkCut,
    pub(crate) title_override: Option<String>,
    pub(crate) cwd_override: Option<std::path::PathBuf>,
}

impl ServerRuntime {
    /// Builds a forked runtime session and persists a self-contained child
    /// rollout (session meta + kept turns/items + applicable compaction).
    pub(crate) async fn create_durable_user_fork(
        &self,
        source: &RuntimeSession,
        options: DurableForkOptions,
    ) -> Result<RuntimeSession, String> {
        let now = Utc::now();
        let forked_id = SessionId::new();
        let rollback_mode = match options.cut {
            SessionForkCut::Through => RollbackMode::ThroughUserTurn,
            SessionForkCut::Before => RollbackMode::BeforeUserTurn,
        };
        let mut forked_runtime = self
            .build_runtime_session_from_user_turn_cut(
                source,
                RuntimeSessionTurnCutOptions {
                    session_id: forked_id,
                    user_turn_index: options.user_turn_index,
                    rollback_mode,
                    cwd_override: options.cwd_override,
                    title_override: options.title_override,
                    created_at: now,
                },
            )
            .await?;

        // User forks are independent sessions — never reuse parent_session_id.
        forked_runtime.summary.parent = None;
        forked_runtime.summary.fork_from_id = Some(options.source_session_id);
        forked_runtime.summary.at_turn_id = options.fork_at_turn_id;

        if forked_runtime.summary.ephemeral {
            return Ok(forked_runtime);
        }

        let mut invented = self
            .rollout_store
            .invent_session_persistence(&forked_id.clone());
        invented.extras.collaboration_mode = Some(forked_runtime.summary.collaboration_mode);
        invented.extras.permission_preset = forked_runtime.summary.permission_preset();
        if let Err(error) = self.rollout_store.append_session_meta_at(
            &invented.rollout_path,
            &forked_runtime.summary.native,
            Some(invented.extras.clone()),
        ) {
            return Err(format!(
                "failed to persist forked session metadata: {error}"
            ));
        }

        if let Some(session_context) = {
            let core = forked_runtime.core_session.lock().await;
            core.session_context.clone()
        } {
            if let Err(error) = self.rollout_store.append_session_context_updated_at(
                &invented.rollout_path,
                forked_id,
                session_context,
            ) {
                tracing::warn!(
                    session_id = %forked_id,
                    error = %error,
                    "failed to persist forked session context"
                );
            } else {
                forked_runtime.session_context_recorded = true;
            }
        }

        write_kept_history_to_rollout(
            &self.rollout_store,
            &invented.rollout_path,
            forked_id,
            &forked_runtime.persisted_turn_items,
            &forked_runtime.turns_by_id,
            forked_runtime.latest_compaction_snapshot.as_ref(),
        )?;

        if let Some(source_path) = &source.rollout_path {
            let kept_calls = forked_runtime
                .persisted_turn_items
                .iter()
                .filter_map(|item| crate::persisted_native_item::tool_call_id(&item.item))
                .collect::<HashSet<_>>();
            let references = devo_core::output_replay::read_output_references(source_path)
                .map_err(|error| format!("failed to read fork output references: {error}"))?;
            let artifacts = references
                .into_iter()
                .filter(|artifact| kept_calls.contains(artifact.call_id.as_str()))
                .collect();
            self.rollout_store
                .append_rollout_lines(
                    &invented.rollout_path,
                    vec![devo_core::RolloutLine::Internal {
                        v: 2,
                        timestamp: now,
                        session_id: forked_id,
                        turn_id: None,
                        seq: 0,
                        entry: devo_core::InternalRecord::Execution {
                            record:
                                devo_core::durable_execution::ExecutionRecord::OutputArtifacts {
                                    artifacts,
                                },
                        },
                    }],
                )
                .map_err(|error| format!("failed to persist fork output references: {error}"))?;
        }

        forked_runtime.rollout_path = Some(invented.rollout_path);
        Ok(forked_runtime)
    }
}

/// Writes kept history as Native Turn + ItemEnvelope lines (path-first).
///
/// Turns come from live [`ReplayedTurn`] snapshots; items are already Native
/// payloads on [`PersistedNativeItem`].
fn write_kept_history_to_rollout(
    rollout_store: &crate::persistence::RolloutStore,
    rollout_path: &Path,
    forked_id: SessionId,
    kept_items: &[PersistedTurnItem],
    turns_by_id: &std::collections::HashMap<devo_core::TurnId, ReplayedTurn>,
    latest_compaction: Option<&devo_core::CompactionSnapshotLine>,
) -> Result<(), String> {
    let native_session_id = forked_id;
    let mut written_turns = HashSet::new();
    let mut item_seq = 1u64;
    for item in kept_items {
        if written_turns.insert(item.turn_id)
            && let Some(legacy_turn_id) = item.legacy_turn_id()
            && let Some(source_turn) = turns_by_id.get(&legacy_turn_id)
        {
            let mut turn = source_turn.native.clone();
            turn.session_id = native_session_id;
            if let Err(error) =
                rollout_store.append_turn_at(rollout_path, &turn, Some(source_turn.extras.clone()))
            {
                return Err(format!("failed to persist forked turn: {error}"));
            }
        }
        let envelope = typed_item_envelope(
            native_session_id,
            item.turn_id,
            item.item_id,
            item_seq,
            &item.item,
            ItemState::Completed,
            Utc::now(),
            /*created_at*/ None,
        );
        item_seq = item_seq.saturating_add(1);
        if let Err(error) = rollout_store.append_canonical_item_at(rollout_path, envelope) {
            return Err(format!("failed to persist forked item: {error}"));
        }
    }

    if let Some(snapshot) = latest_compaction
        && written_turns.contains(&snapshot.turn_id)
    {
        let mut snapshot = snapshot.clone();
        snapshot.session_id = forked_id;
        if let Err(error) = rollout_store.append_compaction_snapshot_at(rollout_path, snapshot) {
            tracing::warn!(
                session_id = %forked_id,
                error = %error,
                "failed to persist forked compaction snapshot"
            );
        }
    }
    Ok(())
}
