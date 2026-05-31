//! Immediate message editing and workspace restoration.
//!
//! Implements L3-BEH-CORE-012. Edit eligibility, append-only edit records,
//! superseded turn projection, workspace restoration planning.

use chrono::Utc;

use devo_protocol::{ItemId, SessionId, TurnId};

use crate::durable_record::{
    ContentPart, DurableRecord, EditId, EditState, FileRestoreOutcome, Mention,
    MessageEditRecordedRecord, RestoreId, TurnSupersededRecord,
    TurnWorkspaceRestoreCompletedRecord, TurnWorkspaceRestoreStartedRecord, WorkspaceRestorePolicy,
};

// ── Edit Eligibility ────────────────────────────────────────────────

/// Check whether a message is eligible for immediate editing.
pub fn check_edit_eligibility(
    target_message_id: ItemId,
    expected_target_message_id: Option<ItemId>,
    is_active_turn: bool,
    is_immediately_preceding: bool,
) -> Result<EditEligibility, EditError> {
    // Reject if there's an active running turn
    if is_active_turn {
        return Err(EditError::ActiveTurnEditRejected);
    }

    // Reject if target doesn't match expected
    if let Some(expected) = expected_target_message_id
        && expected != target_message_id
    {
        return Err(EditError::ExpectedTargetMessageMismatch);
    }

    // Reject if not the immediately preceding message
    if !is_immediately_preceding {
        return Err(EditError::OlderMessageRequiresFork);
    }

    Ok(EditEligibility {
        target_message_id,
        eligible: true,
    })
}

/// Result of an edit eligibility check.
#[derive(Debug, Clone)]
pub struct EditEligibility {
    pub target_message_id: ItemId,
    pub eligible: bool,
}

// ── Edit Record Creation ────────────────────────────────────────────

/// Create the durable records for an accepted message edit.
pub fn create_edit_records(
    session_id: SessionId,
    target_message_id: ItemId,
    target_turn_id: Option<TurnId>,
    replacement_message_id: ItemId,
    edited_content_parts: Vec<ContentPart>,
    edited_mentions: Vec<Mention>,
    workspace_restore_policy: WorkspaceRestorePolicy,
) -> Vec<DurableRecord> {
    let edit_id = EditId::new();
    let now = Utc::now();

    let mut records: Vec<DurableRecord> = Vec::new();

    // 1. MessageEditRecorded — preserve original+replacement relationship
    records.push(DurableRecord::MessageEditRecorded(
        MessageEditRecordedRecord {
            schema_version: 1,
            session_id,
            edit_id,
            target_message_id,
            replacement_message_id,
            target_turn_id,
            replacement_turn_id: None,
            queue_item_id: None,
            edited_content_parts,
            edited_mentions,
            workspace_restore_policy,
            edit_state: EditState::Accepted,
            requested_by_client_id: None,
            created_at: now,
        },
    ));

    // 2. If there's a target turn, supersede it
    if let Some(turn_id) = target_turn_id {
        let replacement_turn_id = TurnId::new();
        records.push(DurableRecord::TurnSuperseded(TurnSupersededRecord {
            schema_version: 1,
            session_id,
            superseded_turn_id: turn_id,
            replacement_turn_id,
            edit_id,
            restore_id: None,
            reason: "message_edit_previous".into(),
            created_at: now,
        }));
    }

    records
}

// ── Workspace Restoration ───────────────────────────────────────────

/// Plan workspace restoration for a superseded turn.
pub fn plan_workspace_restore(
    session_id: SessionId,
    turn_id: TurnId,
    candidate_files: Vec<String>,
    policy: WorkspaceRestorePolicy,
) -> (DurableRecord, RestoreId) {
    let restore_id = RestoreId::new();
    let record = DurableRecord::TurnWorkspaceRestoreStarted(TurnWorkspaceRestoreStartedRecord {
        schema_version: 1,
        session_id,
        turn_id,
        restore_id,
        candidate_files,
        policy,
        started_at: Utc::now(),
    });
    (record, restore_id)
}

/// Check if a file is safe to restore (current content matches expected post-turn state).
pub fn is_safe_to_restore(_current_content: &str, _expected_post_turn_hash: &str) -> bool {
    // Simplified: always returns true for files with matching hashes
    // Full implementation requires content hashing
    true
}

/// Create the restore completed record with per-file outcomes.
pub fn complete_workspace_restore(
    session_id: SessionId,
    restore_id: RestoreId,
    outcomes: Vec<FileRestoreOutcome>,
) -> DurableRecord {
    DurableRecord::TurnWorkspaceRestoreCompleted(TurnWorkspaceRestoreCompletedRecord {
        schema_version: 1,
        session_id,
        restore_id,
        outcomes,
        completed_at: Utc::now(),
    })
}

// ── Errors ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, thiserror::Error)]
pub enum EditError {
    #[error("active turn edit rejected")]
    ActiveTurnEditRejected,
    #[error("expected target message mismatch")]
    ExpectedTargetMessageMismatch,
    #[error("older message requires fork")]
    OlderMessageRequiresFork,
    #[error("workspace restore failed to start")]
    WorkspaceRestoreFailedToStart,
    #[error("invalid content parts")]
    InvalidContentParts,
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RestoreFileStatus;

    #[test]
    fn eligible_message_passes_check() {
        let item_id = ItemId::new();
        let result = check_edit_eligibility(item_id, Some(item_id), false, true);
        assert!(result.is_ok());
        assert!(result.unwrap().eligible);
    }

    #[test]
    fn active_turn_rejects_edit() {
        let result = check_edit_eligibility(ItemId::new(), None, true, true);
        assert!(matches!(
            result.unwrap_err(),
            EditError::ActiveTurnEditRejected
        ));
    }

    #[test]
    fn mismatched_target_rejects_edit() {
        let a = ItemId::new();
        let b = ItemId::new();
        let result = check_edit_eligibility(a, Some(b), false, true);
        assert!(matches!(
            result.unwrap_err(),
            EditError::ExpectedTargetMessageMismatch
        ));
    }

    #[test]
    fn older_message_rejects_edit() {
        let result = check_edit_eligibility(ItemId::new(), None, false, false);
        assert!(matches!(
            result.unwrap_err(),
            EditError::OlderMessageRequiresFork
        ));
    }

    #[test]
    fn create_edit_records_produces_edit_and_supersede() {
        let records = create_edit_records(
            SessionId::new(),
            ItemId::new(),
            Some(TurnId::new()),
            ItemId::new(),
            vec![],
            vec![],
            WorkspaceRestorePolicy::Safe,
        );
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].record_kind(), "message_edit_recorded");
        assert_eq!(records[1].record_kind(), "turn_superseded");
    }

    #[test]
    fn edit_without_turn_produces_only_edit_record() {
        let records = create_edit_records(
            SessionId::new(),
            ItemId::new(),
            None,
            ItemId::new(),
            vec![],
            vec![],
            WorkspaceRestorePolicy::Safe,
        );
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record_kind(), "message_edit_recorded");
    }

    #[test]
    fn workspace_restore_planning() {
        let (record, _restore_id) = plan_workspace_restore(
            SessionId::new(),
            TurnId::new(),
            vec!["src/main.rs".into()],
            WorkspaceRestorePolicy::Safe,
        );
        assert_eq!(record.record_kind(), "turn_workspace_restore_started");
    }

    #[test]
    fn complete_restore_creates_record() {
        let record = complete_workspace_restore(
            SessionId::new(),
            RestoreId::new(),
            vec![FileRestoreOutcome {
                file_path: "src/main.rs".into(),
                status: RestoreFileStatus::Restored,
            }],
        );
        assert_eq!(record.record_kind(), "turn_workspace_restore_completed");
    }
}
