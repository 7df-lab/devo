use chrono::Utc;
use devo_core::{
    ContentPart, EditId, EditState, FileRestoreOutcome, ItemId, MessageEditRecordedRecord,
    RestoreFileStatus, RestoreId, RolloutLine, SessionId, TurnId, TurnSupersededRecord,
    TurnWorkspaceRestoreCompletedRecord, TurnWorkspaceRestoreStartedRecord, WorkspaceRestorePolicy,
    message_edit_line, parse_rollout_line, turn_superseded_line, workspace_restore_completed_line,
    workspace_restore_started_line,
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

    let variants: Vec<RolloutLine> = vec![
        message_edit_line(
            now,
            &MessageEditRecordedRecord {
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
        )
        .expect("build message edit line"),
        turn_superseded_line(
            now,
            &TurnSupersededRecord {
                schema_version: 1,
                session_id,
                superseded_turn_id,
                replacement_turn_id,
                edit_id,
                restore_id: Some(restore_id),
                reason: "message_edit_previous".into(),
                created_at: now,
            },
        )
        .expect("build turn superseded line"),
        workspace_restore_started_line(
            now,
            TurnWorkspaceRestoreStartedRecord {
                schema_version: 1,
                session_id,
                turn_id: superseded_turn_id,
                restore_id,
                candidate_files: vec!["src/main.rs".into()],
                policy: WorkspaceRestorePolicy::Skip,
                started_at: now,
            },
        ),
        workspace_restore_completed_line(
            now,
            TurnWorkspaceRestoreCompletedRecord {
                schema_version: 1,
                session_id,
                restore_id,
                outcomes: vec![FileRestoreOutcome {
                    file_path: "src/main.rs".into(),
                    status: RestoreFileStatus::Skipped,
                }],
                completed_at: now,
            },
        ),
    ];

    for line in variants {
        let json = serde_json::to_string(&line).expect("serialize rollout line");
        let restored = parse_rollout_line(&json).expect("parse rollout line");
        assert_eq!(restored, line);
    }
}
