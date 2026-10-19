pub mod event_projection;
pub mod history;
pub mod native_from_turn_item;
pub mod native_replay;
pub mod rollout;
pub mod rollout_write;
pub mod rollout_write_state;
pub mod session_tree;

mod records;

pub use devo_protocol::{ItemId, SessionId, SessionTitleState, TurnId, TurnStatus, TurnUsage};
pub use event_projection::{
    DerivedEvent, EVENT_SCHEMA_VERSION, events_from_rollout_line, session_stream_id,
    sessions_stream_id, source_fact_id,
};
pub use history::{
    CanonicalHistory, HistoryReadError, read_canonical_history, read_rollout_session_leaf,
    read_rollout_session_meta,
};
pub use native_from_turn_item::native_item_from_turn_item;
pub use native_replay::{
    NativeReplayError, item_record_from_native, legacy_compaction_line_from_native,
    legacy_lines_from_internal, legacy_rollback_line_from_native, legacy_title_line_from_native,
    session_record_from_native, turn_record_from_native,
};
pub use records::{
    ApprovalDecisionItem, ApprovalRequestItem, CommandExecutionItem, CompactionSnapshotLine,
    ItemLine, ItemRecord, LegacyRolloutLine, MessageEditRecordedLine, SessionContextUpdatedLine,
    SessionMetaLine, SessionRecord, SessionRollbackLine, SessionSettingsField, SessionSettingsLine,
    SessionTitleUpdatedLine, TextItem, ToolCallItem, ToolProgressItem, ToolResultItem, TurnError,
    TurnItem, TurnLine, TurnRecord, TurnSupersededLine, TurnWorkspaceChangeRecordedLine,
    TurnWorkspaceCheckpointRecordedLine, TurnWorkspaceRestoreCompletedLine,
    TurnWorkspaceRestoreStartedLine, Worklog,
};
pub use rollout::{
    InternalRecord, ROLLOUT_FORMAT_VERSION, RolloutLine, RolloutLineReadError,
    SessionPersistenceExtras, TurnPersistenceExtras, parse_rollout_line,
};
pub use rollout_write::{
    canonical_turn_from_record, compaction_snapshot_line, message_edit_line, native_item_id,
    native_session_from_record, native_session_id, native_turn_id, session_context_line,
    session_leaf_line, session_line, session_line_from_record,
    session_persistence_extras_from_record, session_rollback_line, settings_line, title_line,
    tree_edge_line, turn_line, turn_line_from_record, turn_persistence_extras_from_record,
    turn_superseded_line, workspace_change_line, workspace_checkpoint_line,
    workspace_restore_completed_line, workspace_restore_started_line,
};
pub use rollout_write_state::{RecordConversionError, RolloutWriteState};
pub use session_tree::{
    active_path_item_ids, active_path_items, build_session_tree, is_tree_visible_item,
    path_root_to_leaf, resolve_leaf_id, resolve_parent_map,
};
