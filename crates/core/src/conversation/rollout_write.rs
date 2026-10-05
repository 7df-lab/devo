//! Persistence-boundary helpers: Native session/turn → versioned rollout
//! lines for the **live write path**, plus thin record wrappers when only
//! stored record shapes exist.
//!
//! Prefer path-first Native builders ([`session_line`], [`turn_line`]).
//! [`session_line_from_record`] / [`turn_line_from_record`] convert once at
//! the persistence boundary.

use chrono::{DateTime, Utc};
use devo_protocol::native::error::AgentError;
use devo_protocol::native::ids::{ItemId, SessionId, TurnId};
use devo_protocol::native::model::{ModelBinding, PermissionProfile};
use devo_protocol::native::session::{
    GitInfo, Session, SessionActivity, SessionParent, SessionSettings, SessionStatus,
};
use devo_protocol::native::turn::{Turn, TurnKind, TurnStatus};
use devo_protocol::native::usage::{SessionUsage, TurnUsage as CanonicalTurnUsage, UsageTotals};

use crate::MessageEditRecordedRecord;
use crate::SessionContext;
use crate::TurnKind as LegacyTurnKind;
use crate::TurnSupersededRecord;
use crate::TurnWorkspaceChangeRecordedRecord;
use crate::TurnWorkspaceCheckpointRecordedRecord;
use crate::TurnWorkspaceRestoreCompletedRecord;
use crate::TurnWorkspaceRestoreStartedRecord;
use crate::conversation::{
    CompactionSnapshotLine, SessionRecord, SessionSettingsField, TurnRecord,
    TurnStatus as LegacyTurnStatus,
};

use super::rollout::{
    InternalRecord, ROLLOUT_FORMAT_VERSION, RolloutLine, SessionPersistenceExtras,
    TurnPersistenceExtras,
};
use super::rollout_write_state::RecordConversionError;

fn permission_profile_from_session_record(record: &SessionRecord) -> PermissionProfile {
    use devo_protocol::PermissionPreset;

    match record.permission_preset {
        Some(PermissionPreset::Default) => PermissionProfile::Default,
        Some(PermissionPreset::AutoReview) => PermissionProfile::AutoReview,
        Some(PermissionPreset::FullAccess) => PermissionProfile::FullAccess,
        None => {
            let approval_mode = record.approval_mode.to_ascii_lowercase();
            if approval_mode.contains("auto") {
                PermissionProfile::AutoReview
            } else if approval_mode.contains("full") {
                PermissionProfile::FullAccess
            } else {
                PermissionProfile::Default
            }
        }
    }
}

/// Converts a legacy `TurnRecord` into the canonical `Turn`. Shared by the
/// live write path and history readers that still hold turn records.
pub fn canonical_turn_from_record(record: &TurnRecord) -> Result<Turn, RecordConversionError> {
    let kind = match &record.kind {
        LegacyTurnKind::Regular | LegacyTurnKind::Review | LegacyTurnKind::Other(_) => {
            TurnKind::Regular
        }
        LegacyTurnKind::ManualCompaction => TurnKind::Compaction,
    };

    let status = match record.status {
        LegacyTurnStatus::Pending
        | LegacyTurnStatus::Running
        | LegacyTurnStatus::WaitingApproval => TurnStatus::InProgress,
        LegacyTurnStatus::Completed => TurnStatus::Completed,
        LegacyTurnStatus::Interrupted => TurnStatus::Interrupted,
        LegacyTurnStatus::Failed => TurnStatus::Failed,
    };

    let error = record.error.as_ref().map(|error| {
        let mut projected = AgentError::new(error.code.clone(), error.message.clone());
        if let Some(hint) = &error.recovery_hint {
            projected.details = Some(serde_json::json!({ "recoveryHint": hint }));
        }
        projected
    });

    let usage = record
        .latest_query_usage
        .as_ref()
        .or(record.usage.as_ref())
        .map(|usage| CanonicalTurnUsage {
            query: UsageTotals {
                total_tokens: u64::from(
                    usage
                        .total_tokens
                        .unwrap_or(usage.input_tokens + usage.output_tokens),
                ),
                input_tokens: u64::from(usage.input_tokens),
                output_tokens: u64::from(usage.output_tokens),
                reasoning_tokens: u64::from(usage.reasoning_output_tokens.unwrap_or(0)),
                cache_read_input_tokens: u64::from(usage.cache_read_input_tokens.unwrap_or(0)),
                cache_creation_input_tokens: u64::from(
                    usage.cache_creation_input_tokens.unwrap_or(0),
                ),
                call_count: 0,
                metered_call_count: 1,
                ..UsageTotals::default()
            },
            overhead: UsageTotals::default(),
        });

    Ok(Turn {
        id: record.id,
        session_id: record.session_id,
        sequence: record.sequence,
        kind,
        status,
        model: ModelBinding {
            provider: record
                .model_binding_id
                .clone()
                .unwrap_or_else(|| "unknown".into()),
            model: if record.request_model.is_empty() {
                record.model.clone()
            } else {
                record.request_model.clone()
            },
            variant: None,
            reasoning_effort: record
                .reasoning_effort_selection
                .as_deref()
                .and_then(|selection| selection.parse().ok()),
        },
        collaboration_mode: Some(
            record
                .turn_context
                .as_ref()
                .map(|context| context.collaboration_mode)
                .unwrap_or_default(),
        ),
        started_at: record.started_at,
        completed_at: record.completed_at,
        error,
        usage,
    })
}

/// Native `Session` → v2 SessionMeta line (preferred live append boundary).
pub fn session_line(
    session: Session,
    extras: Option<SessionPersistenceExtras>,
    timestamp: DateTime<Utc>,
) -> RolloutLine {
    RolloutLine::SessionMeta {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        session: Box::new(session),
        extras: extras.map(Box::new),
    }
}

/// Persistence extras extracted from a legacy [`SessionRecord`].
///
/// Migrate / index / test boundary only. Live create paths invent
/// [`SessionPersistenceExtras`] at origin instead of converting a record.
pub fn session_persistence_extras_from_record(record: &SessionRecord) -> SessionPersistenceExtras {
    SessionPersistenceExtras {
        session_context: record.session_context.clone(),
        cli_version: record.cli_version.clone(),
        source: record.source.clone(),
        collaboration_mode: record.collaboration_mode,
        permission_preset: record.permission_preset,
        kernel_snapshot_path: None,
    }
}

/// SessionRecord → Native `Session` (persistence-boundary conversion).
pub fn native_session_from_record(
    record: &SessionRecord,
) -> Result<Session, RecordConversionError> {
    let parent_id = record.parent_session_id;
    let is_agent = record.agent_role.is_some()
        || record.agent_nickname.is_some()
        || record.agent_path.is_some();
    let parent = match (parent_id, is_agent) {
        (Some(parent_id), true) => Some(SessionParent::Agent {
            session_id: parent_id,
            role: record.agent_role.clone(),
        }),
        _ => None,
    };
    let fork_from_id = match record.fork_from_id {
        Some(id) => Some(id),
        None if parent_id.is_some() && !is_agent => parent_id,
        None => None,
    };
    let at_turn_id = record.fork_at_turn_id;

    let permission_profile = permission_profile_from_session_record(record);

    let git_info = if record.git_sha.is_some()
        || record.git_branch.is_some()
        || record.git_origin_url.is_some()
    {
        Some(GitInfo {
            sha: record.git_sha.clone(),
            branch: record.git_branch.clone(),
            origin_url: record.git_origin_url.clone(),
            dirty: None,
            observed_at: record.updated_at,
        })
    } else {
        None
    };

    let legacy_totals = UsageTotals {
        total_tokens: record.tokens_used.max(0) as u64,
        ..UsageTotals::default()
    };

    Ok(Session {
        id: record.id,
        version: 1,
        cwd: record.cwd.clone(),
        additional_directories: record.additional_directories.clone(),
        parent,
        fork_from_id,
        at_turn_id,
        ephemeral: false,
        created_at: record.created_at,
        status: SessionStatus::Idle,
        flags: Vec::new(),
        archived: record.archived_at.is_some(),
        activity: SessionActivity::Idle,
        active_turn_id: None,
        queued_count: 0,
        title: record.title.clone(),
        title_state: record.title_state.clone(),
        model: ModelBinding {
            provider: record.model_provider.clone(),
            model: record.model.clone().unwrap_or_default(),
            variant: None,
            reasoning_effort: record
                .reasoning_effort_selection
                .as_deref()
                .and_then(|selection| selection.parse().ok()),
        },
        settings: SessionSettings {
            permission_profile,
            reasoning_effort: record.reasoning_effort_selection.clone(),
            mode: None,
            sandbox_profile: (!record.sandbox_policy.is_empty())
                .then(|| record.sandbox_policy.clone()),
            effective_context_window: record.effective_context_window,
            auto_refine_enabled: None,
            auto_refine_turn_interval: None,
            python_cell_first_wait_ms: None,
        },
        git_info,
        preview: record.first_user_message.clone().unwrap_or_default(),
        last_activity_at: record.last_activity_at.unwrap_or(record.updated_at),
        transcript_size_bytes: None,
        message_count: None,
        summary: None,
        task_state: None,
        usage: SessionUsage {
            total: legacy_totals.clone(),
            by_purpose: Vec::new(),
            legacy: Some(legacy_totals),
            updated_at: record.updated_at,
        },
    })
}

/// SessionRecord → v2 SessionMeta line (thin wrapper when only a record exists).
pub fn session_line_from_record(
    record: &SessionRecord,
    timestamp: DateTime<Utc>,
) -> Result<RolloutLine, RecordConversionError> {
    Ok(session_line(
        native_session_from_record(record)?,
        Some(session_persistence_extras_from_record(record)),
        timestamp,
    ))
}

/// Native `Turn` → v2 Turn line (preferred live append boundary).
pub fn turn_line(
    turn: Turn,
    extras: Option<TurnPersistenceExtras>,
    timestamp: DateTime<Utc>,
) -> RolloutLine {
    RolloutLine::Turn {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        turn,
        extras: extras.map(Box::new),
    }
}

/// TurnRecord → persistence extras (migrate / fixture boundary only).
///
/// Converts packed legacy `latest_query_usage` into Native via
/// [`devo_protocol::TurnUsage::to_native`]. Live append builds extras from
/// Native runtime state directly.
pub fn turn_persistence_extras_from_record(record: &TurnRecord) -> TurnPersistenceExtras {
    TurnPersistenceExtras {
        session_context: record.session_context.clone(),
        turn_context: record.turn_context.clone(),
        request_thinking: record.request_thinking.clone(),
        input_token_estimate: record.input_token_estimate,
        latest_query_usage: record
            .latest_query_usage
            .as_ref()
            .map(devo_protocol::TurnUsage::to_native),
        context_occupancy: record.context_occupancy.clone(),
        stop_reason: record.stop_reason.clone(),
        failure_reason: record.failure_reason,
    }
}

/// TurnRecord → v2 Turn line (thin wrapper when only a record exists).
pub fn turn_line_from_record(
    record: &TurnRecord,
    timestamp: DateTime<Utc>,
) -> Result<RolloutLine, RecordConversionError> {
    Ok(turn_line(
        canonical_turn_from_record(record)?,
        Some(turn_persistence_extras_from_record(record)),
        timestamp,
    ))
}

pub fn title_line(
    timestamp: DateTime<Utc>,
    session_id: SessionId,
    title: String,
    previous_title: Option<String>,
) -> RolloutLine {
    RolloutLine::SessionTitleUpdated {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        session_id,
        title,
        previous_title,
    }
}

pub fn session_context_line(
    timestamp: DateTime<Utc>,
    session_id: SessionId,
    session_context: SessionContext,
) -> RolloutLine {
    RolloutLine::Internal {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        session_id,
        turn_id: None,
        seq: 0,
        entry: InternalRecord::SessionContext(Box::new(session_context)),
    }
}

pub fn settings_line(
    timestamp: DateTime<Utc>,
    session_id: SessionId,
    field: SessionSettingsField,
    value: serde_json::Value,
    epoch: u64,
) -> RolloutLine {
    RolloutLine::Internal {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        session_id,
        turn_id: None,
        seq: 0,
        entry: InternalRecord::SessionSettings {
            schema_version: 1,
            field,
            value,
            epoch,
        },
    }
}

pub fn compaction_snapshot_line(
    timestamp: DateTime<Utc>,
    snapshot: &CompactionSnapshotLine,
) -> Result<RolloutLine, RecordConversionError> {
    Ok(RolloutLine::CompactionSnapshot {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        session_id: snapshot.session_id,
        turn_id: snapshot.turn_id,
        summary_item_id: snapshot.summary_item_id,
        // Native item ids are opaque interned strings (`item_<uuid-v7>`); the
        // read path (legacy_compaction_line_from_native) copies them verbatim.
        // Round-tripping through `Uuid::parse_str` rejected every native id
        // and made compactions with a preserved suffix fail their snapshot
        // persist after the summary item was already appended.
        preserved_item_ids: snapshot.preserved_item_ids.clone(),
        context_occupancy: snapshot.context_occupancy.clone(),
    })
}

pub fn message_edit_line(
    timestamp: DateTime<Utc>,
    record: &MessageEditRecordedRecord,
) -> Result<RolloutLine, RecordConversionError> {
    Ok(RolloutLine::Internal {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        session_id: record.session_id,
        turn_id: record.replacement_turn_id.or(record.target_turn_id),
        seq: 0,
        entry: InternalRecord::MessageEdit(record.clone()),
    })
}

pub fn turn_superseded_line(
    timestamp: DateTime<Utc>,
    record: &TurnSupersededRecord,
) -> Result<RolloutLine, RecordConversionError> {
    Ok(RolloutLine::Internal {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        session_id: record.session_id,
        turn_id: Some(record.replacement_turn_id),
        seq: 0,
        entry: InternalRecord::TurnSuperseded(record.clone()),
    })
}

pub fn session_leaf_line(
    timestamp: DateTime<Utc>,
    session_id: SessionId,
    leaf_id: Option<ItemId>,
    epoch: u64,
) -> RolloutLine {
    RolloutLine::Internal {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        session_id,
        turn_id: None,
        seq: 0,
        entry: InternalRecord::SessionLeaf { epoch, leaf_id },
    }
}

pub fn tree_edge_line(
    timestamp: DateTime<Utc>,
    session_id: SessionId,
    child_id: ItemId,
    parent_id: Option<ItemId>,
) -> RolloutLine {
    RolloutLine::Internal {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        session_id,
        turn_id: None,
        seq: 0,
        entry: InternalRecord::TreeEdge {
            child_id,
            parent_id,
        },
    }
}

pub fn workspace_checkpoint_line(
    timestamp: DateTime<Utc>,
    record: TurnWorkspaceCheckpointRecordedRecord,
) -> RolloutLine {
    RolloutLine::WorkspaceCheckpoint {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        record,
    }
}

pub fn workspace_change_line(
    timestamp: DateTime<Utc>,
    record: TurnWorkspaceChangeRecordedRecord,
) -> RolloutLine {
    RolloutLine::WorkspaceChange {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        record,
    }
}

pub fn workspace_restore_started_line(
    timestamp: DateTime<Utc>,
    record: TurnWorkspaceRestoreStartedRecord,
) -> RolloutLine {
    RolloutLine::WorkspaceRestoreStarted {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        record,
    }
}

pub fn workspace_restore_completed_line(
    timestamp: DateTime<Utc>,
    record: TurnWorkspaceRestoreCompletedRecord,
) -> RolloutLine {
    RolloutLine::WorkspaceRestoreCompleted {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        record,
    }
}

pub fn session_rollback_line(
    timestamp: DateTime<Utc>,
    session_id: SessionId,
    retained_turn_ids: Vec<TurnId>,
    retained_item_ids: Vec<ItemId>,
    latest_turn_id: Option<TurnId>,
) -> RolloutLine {
    RolloutLine::SessionRollback {
        v: ROLLOUT_FORMAT_VERSION,
        timestamp,
        session_id,
        retained_turn_ids,
        retained_item_ids,
        latest_turn_id,
    }
}

/// Maps a legacy/opaque id string into a Native session id.
pub fn native_session_id(id: impl std::fmt::Display) -> Result<SessionId, RecordConversionError> {
    Ok(SessionId::from_string(id.to_string()))
}

/// Maps a legacy/opaque id string into a Native item id.
pub fn native_item_id(id: impl std::fmt::Display) -> Result<ItemId, RecordConversionError> {
    Ok(ItemId::from_string(id.to_string()))
}

/// Maps a legacy/opaque id string into a Native turn id.
pub fn native_turn_id(id: impl std::fmt::Display) -> Result<TurnId, RecordConversionError> {
    Ok(TurnId::from_string(id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn compaction_snapshot_line_accepts_native_item_ids() {
        // Native ids are opaque (`item_<uuid-v7>`); a preserved-suffix
        // compaction must persist its snapshot even though those ids are not
        // bare UUIDs. Regression: round-tripping through `Uuid::parse_str`
        // rejected every native id and failed the persist after the summary
        // item had already been appended.
        let preserved = vec![ItemId::new(), ItemId::new()];
        let snapshot = CompactionSnapshotLine {
            timestamp: Utc::now(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            summary_item_id: ItemId::new(),
            preserved_item_ids: preserved.clone(),
            context_occupancy: None,
        };
        let line = compaction_snapshot_line(snapshot.timestamp, &snapshot)
            .expect("native preserved item ids must project");
        match line {
            RolloutLine::CompactionSnapshot {
                preserved_item_ids, ..
            } => assert_eq!(preserved_item_ids, preserved),
            other => panic!("expected CompactionSnapshot line, got {other:?}"),
        }
    }
}
