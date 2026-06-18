use chrono::Utc;
use devo_core::{
    ContentPart, EditId, EditState, FileRestoreOutcome, ItemId, MessageEditRecordedLine,
    MessageEditRecordedRecord, RestoreFileStatus, RestoreId, RolloutLine, SessionId, TurnId,
    TurnSupersededLine, TurnSupersededRecord, TurnWorkspaceRestoreCompletedLine,
    TurnWorkspaceRestoreCompletedRecord, TurnWorkspaceRestoreStartedLine,
    TurnWorkspaceRestoreStartedRecord, WorkspaceRestorePolicy,
};
use pretty_assertions::assert_eq;

#[test]
fn message_edit_rollout_lines_roundtrip() {
    let now = Utc::now();
    let session_id = SessionId::new();
    let edit_id = EditId::new();
    let restore_id = RestoreId::new();
    let superseded_turn_id = TurnId::new();
    let replacement_turn_id = TurnId::new();
    let target_message_id = ItemId::new();
    let replacement_message_id = ItemId::new();

    let variants = vec![
        RolloutLine::MessageEditRecorded(Box::new(MessageEditRecordedLine {
            timestamp: now,
            record: MessageEditRecordedRecord {
                schema_version: 1,
                session_id,
                edit_id,
                target_message_id,
                replacement_message_id,
                target_turn_id: Some(superseded_turn_id),
                replacement_turn_id: Some(replacement_turn_id),
                queue_item_id: None,
                edited_content_parts: vec![ContentPart::Text("edited".into())],
                edited_mentions: Vec::new(),
                workspace_restore_policy: WorkspaceRestorePolicy::Skip,
                edit_state: EditState::Accepted,
                requested_by_client_id: None,
                created_at: now,
            },
        })),
        RolloutLine::TurnSuperseded(Box::new(TurnSupersededLine {
            timestamp: now,
            record: TurnSupersededRecord {
                schema_version: 1,
                session_id,
                superseded_turn_id,
                replacement_turn_id,
                edit_id,
                restore_id: Some(restore_id),
                reason: "message_edit_previous".into(),
                created_at: now,
            },
        })),
        RolloutLine::TurnWorkspaceRestoreStarted(Box::new(TurnWorkspaceRestoreStartedLine {
            timestamp: now,
            record: TurnWorkspaceRestoreStartedRecord {
                schema_version: 1,
                session_id,
                turn_id: superseded_turn_id,
                restore_id,
                candidate_files: vec!["src/main.rs".into()],
                policy: WorkspaceRestorePolicy::Skip,
                started_at: now,
            },
        })),
        RolloutLine::TurnWorkspaceRestoreCompleted(Box::new(TurnWorkspaceRestoreCompletedLine {
            timestamp: now,
            record: TurnWorkspaceRestoreCompletedRecord {
                schema_version: 1,
                session_id,
                restore_id,
                outcomes: vec![FileRestoreOutcome {
                    file_path: "src/main.rs".into(),
                    status: RestoreFileStatus::Skipped,
                }],
                completed_at: now,
            },
        })),
    ];

    for variant in variants {
        let json = serde_json::to_string(&variant).expect("serialize rollout line");
        let restored: RolloutLine = serde_json::from_str(&json).expect("deserialize rollout line");
        assert_eq!(restored, variant);
    }
}
