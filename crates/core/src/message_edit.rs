//! Immediate message editing and workspace restoration.
//!
//! Implements L3-BEH-CORE-012. Edit eligibility, append-only edit records,
//! superseded turn projection, workspace restoration planning.

use chrono::Utc;
use sha2::Digest;
use sha2::Sha256;

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
    let replacement_turn_id = target_turn_id.map(|_| TurnId::new());
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
            replacement_turn_id,
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
    if let (Some(turn_id), Some(replacement_turn_id)) = (target_turn_id, replacement_turn_id) {
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
pub fn is_safe_to_restore(current_content: &str, expected_post_turn_hash: &str) -> bool {
    let expected_hash = expected_post_turn_hash
        .trim()
        .strip_prefix("sha256:")
        .unwrap_or_else(|| expected_post_turn_hash.trim());
    if expected_hash.is_empty()
        || expected_hash.len() != 64
        || !expected_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return false;
    }

    content_sha256_hex(current_content).eq_ignore_ascii_case(expected_hash)
}

fn content_sha256_hex(content: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let digest = Sha256::digest(content.as_bytes());
    let mut encoded = String::with_capacity(64);
    for &byte in &digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
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
    use pretty_assertions::assert_eq;

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
        let DurableRecord::MessageEditRecorded(edit_record) = &records[0] else {
            panic!("expected message edit record");
        };
        let DurableRecord::TurnSuperseded(superseded_record) = &records[1] else {
            panic!("expected turn superseded record");
        };
        assert_eq!(
            edit_record.replacement_turn_id,
            Some(superseded_record.replacement_turn_id)
        );
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
        let DurableRecord::MessageEditRecorded(edit_record) = &records[0] else {
            panic!("expected message edit record");
        };
        assert_eq!(edit_record.replacement_turn_id, None);
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

    /// Trace: L2-DES-APP-003
    /// Verifies: safe workspace restore accepts unchanged post-turn file content.
    #[test]
    fn safe_restore_accepts_matching_post_turn_hash() {
        let expected_hash = content_sha256_hex("post-turn content");

        assert!(is_safe_to_restore("post-turn content", &expected_hash));
    }

    /// Trace: L2-DES-APP-003
    /// Verifies: safe workspace restore rejects files that diverged after the superseded turn.
    #[test]
    fn safe_restore_rejects_diverged_content() {
        let expected_hash = content_sha256_hex("post-turn content");

        assert!(!is_safe_to_restore("user changed content", &expected_hash));
    }

    /// Trace: L2-DES-APP-003
    /// Verifies: safe workspace restore accepts explicit sha256-prefixed hashes.
    #[test]
    fn safe_restore_accepts_sha256_prefixed_hash() {
        let expected_hash = format!("sha256:{}", content_sha256_hex("post-turn content"));

        assert!(is_safe_to_restore("post-turn content", &expected_hash));
    }

    /// Trace: L2-DES-APP-003
    /// Verifies: safe workspace restore fails closed when no usable post-turn hash exists.
    #[test]
    fn safe_restore_rejects_empty_or_malformed_hash() {
        assert!(!is_safe_to_restore("post-turn content", ""));
        assert!(!is_safe_to_restore("post-turn content", "not-a-sha256"));
    }
}
