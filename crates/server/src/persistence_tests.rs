use std::collections::HashMap;
use std::path::PathBuf;

use chrono::TimeZone;
use chrono::Utc;
use pretty_assertions::assert_eq;

use super::ReplayHistoryItemPayload;
use super::ReplayState;
use super::RolloutLineReadError;
use super::build_prompt_messages_from_snapshot;
use super::parse_rollout_line;
use crate::execution::ServerRuntimeDependencies;
use crate::persisted_native_item::PersistedNativeItem;
use crate::persistence::apply_turn_item;
use devo_core::CompactionSnapshotLine;
use devo_core::ContentPart;
use devo_core::EditId;
use devo_core::EditState;
use devo_core::EnvironmentContext;
use devo_core::ItemId;
use devo_core::ItemLine;
use devo_core::ItemRecord;
use devo_core::LanguageContext;
use devo_core::LegacyRolloutLine;
use devo_core::Message;
use devo_core::MessageEditRecordedLine;
use devo_core::MessageEditRecordedRecord;
use devo_core::Model;
use devo_core::Persona;
use devo_core::SessionContext;
use devo_core::SessionId;
use devo_core::SessionMetaLine;
use devo_core::SessionRecord;
use devo_core::SessionRollbackLine;
use devo_core::SessionTitleState;
use devo_core::TextItem;
use devo_core::ToolCallItem;
use devo_core::ToolResultItem;
use devo_core::TurnContext;
use devo_core::TurnId;
use devo_core::TurnItem;
use devo_core::TurnKind;
use devo_core::TurnLine;
use devo_core::TurnRecord;
use devo_core::TurnStatus;
use devo_core::TurnSupersededLine;
use devo_core::TurnSupersededRecord;
use devo_core::WorkspaceRestorePolicy;
use devo_protocol::native::ids::{ItemId as NativeItemId, TurnId as NativeTurnId};
use devo_protocol::native::item::Item as NativeItem;
use devo_protocol::native::item::UserMessageEntry;

fn sample_record(store: &super::RolloutStore, cwd: PathBuf, title: Option<&str>) -> SessionRecord {
    sample_record_for(store, SessionId::new(), Utc::now(), cwd, title)
}

fn sample_record_for(
    store: &super::RolloutStore,
    session_id: SessionId,
    created_at: chrono::DateTime<Utc>,
    cwd: PathBuf,
    title: Option<&str>,
) -> SessionRecord {
    store.create_session_record(
        session_id,
        created_at,
        cwd,
        Vec::new(),
        title.map(str::to_string),
        Some("test-model".into()),
        None,
        None,
        "test-provider".into(),
        None,
    )
}

fn test_turn_record(session_id: SessionId, turn_id: TurnId) -> TurnRecord {
    test_turn_at(session_id, turn_id, 1, Utc::now(), TurnStatus::Completed)
}

fn test_turn_at(
    session_id: SessionId,
    turn_id: TurnId,
    sequence: u32,
    at: chrono::DateTime<Utc>,
    status: TurnStatus,
) -> TurnRecord {
    TurnRecord {
        id: turn_id,
        session_id,
        sequence,
        started_at: at,
        completed_at: Some(at),
        status,
        kind: TurnKind::Regular,
        model: "test-model".into(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        request_model: "test-model".into(),
        request_thinking: None,
        input_token_estimate: None,
        usage: None,
        latest_query_usage: None,
        context_occupancy: None,
        stop_reason: None,
        failure_reason: None,
        error: None,
        session_context: None,
        turn_context: None,
        schema_version: 4,
    }
}

fn apply_turn_line(replay: &mut ReplayState, timestamp: chrono::DateTime<Utc>, turn: TurnRecord) {
    replay
        .apply_line(LegacyRolloutLine::Turn(Box::new(TurnLine {
            timestamp,
            turn,
        })))
        .expect("apply turn");
}

fn turn_usage(
    input_tokens: u32,
    output_tokens: u32,
    total_tokens: u32,
) -> devo_protocol::TurnUsage {
    devo_protocol::TurnUsage {
        input_tokens,
        output_tokens,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
        reasoning_output_tokens: None,
        total_tokens: Some(total_tokens),
    }
}

fn model_turn(
    session_id: SessionId,
    turn_id: TurnId,
    sequence: u32,
    at: chrono::DateTime<Utc>,
    status: TurnStatus,
    model: &str,
) -> TurnRecord {
    let mut turn = test_turn_at(session_id, turn_id, sequence, at, status);
    turn.model = model.into();
    turn.request_model = model.into();
    turn.schema_version = 2;
    turn
}

#[test]
fn replay_orders_items_by_sequence_before_timestamp() {
    let session_id = SessionId::new();
    let turn_id = TurnId::new();
    let earlier = Utc.with_ymd_and_hms(2026, 4, 6, 8, 0, 0).unwrap();
    let later = Utc.with_ymd_and_hms(2026, 4, 6, 8, 0, 1).unwrap();
    let mut replay = ReplayState::default();

    replay
        .apply_line(LegacyRolloutLine::Item(Box::new(ItemLine {
            timestamp: earlier,
            item: ItemRecord {
                id: ItemId::new(),
                session_id,
                turn_id,
                seq: 2,
                timestamp: earlier,
                started_at: None,
                attempt_placement: None,
                turn_status: None,
                sibling_turn_ids: Vec::new(),
                input_items: Vec::new(),
                output_items: vec![TurnItem::ToolCall(ToolCallItem {
                    tool_call_id: "call-1".to_string(),
                    tool_name: "bash".to_string(),
                    input: serde_json::json!({"command":"date"}),
                })],
                worklog: None,
                error: None,
                schema_version: 1,
            },
        })))
        .expect("replay later-seq line");
    replay
        .apply_line(LegacyRolloutLine::Item(Box::new(ItemLine {
            timestamp: later,
            item: ItemRecord {
                id: ItemId::new(),
                session_id,
                turn_id,
                seq: 1,
                timestamp: later,
                started_at: None,
                attempt_placement: None,
                turn_status: None,
                sibling_turn_ids: Vec::new(),
                output_items: vec![TurnItem::AgentMessage(TextItem::text("assistant 1"))],
                input_items: Vec::new(),
                worklog: None,
                error: None,
                schema_version: 1,
            },
        })))
        .expect("replay earlier-seq line");

    let mut items = replay.pending_items;
    items.sort_by(|left, right| {
        left.seq
            .cmp(&right.seq)
            .then_with(|| left.timestamp.cmp(&right.timestamp))
            .then_with(|| left.intra_record_order.cmp(&right.intra_record_order))
    });

    let titles = items
        .into_iter()
        .map(|item| match item.payload {
            ReplayHistoryItemPayload::NativeItem(NativeItem::AssistantMessage { text, .. }) => text,
            ReplayHistoryItemPayload::NativeItem(NativeItem::ToolCall { input, .. }) => input
                .as_ref()
                .and_then(|value| value["command"].as_str())
                .unwrap_or_default()
                .to_string(),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>();

    assert_eq!(titles, vec!["assistant 1", "date"]);
}

#[test]
fn replay_prefers_compaction_occupancy_over_prior_turn() {
    use pretty_assertions::assert_eq;

    let now = Utc.with_ymd_and_hms(2026, 7, 8, 10, 0, 0).unwrap();
    let session_id = SessionId::new();
    let turn_id = TurnId::new();
    let mut replay = ReplayState::default();
    let turn_occupancy = devo_protocol::native::item::ContextOccupancy::from_category_tokens(
        /*context_window_tokens*/ 100_000, /*base*/ 10_000, /*skills*/ 0,
        /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 40_000,
    );
    let compact_occupancy = devo_protocol::native::item::ContextOccupancy::from_category_tokens(
        /*context_window_tokens*/ 100_000, /*base*/ 10_000, /*skills*/ 0,
        /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 8_000,
    );

    let mut turn = test_turn_at(session_id, turn_id, 1, now, TurnStatus::Completed);
    turn.latest_query_usage = Some(turn_usage(50, 5, 55));
    turn.context_occupancy = Some(turn_occupancy.clone());
    apply_turn_line(&mut replay, now, turn);
    assert_eq!(replay.latest_context_occupancy, Some(turn_occupancy));
    replay
        .apply_line(LegacyRolloutLine::CompactionSnapshot(Box::new(
            CompactionSnapshotLine {
                timestamp: now,
                session_id,
                turn_id,
                summary_item_id: ItemId::new(),
                preserved_item_ids: Vec::new(),
                context_occupancy: Some(compact_occupancy.clone()),
            },
        )))
        .expect("compaction");
    assert_eq!(replay.latest_context_occupancy, Some(compact_occupancy));
}

#[test]
fn resume_context_pressure_prefers_compaction_occupancy_over_large_query() {
    use pretty_assertions::assert_eq;

    use devo_protocol::native::item::ContextOccupancy;
    use devo_protocol::native::usage::{TurnUsage, UsageTotals};

    let occupancy = ContextOccupancy::from_category_tokens(
        /*context_window_tokens*/ 250_000, /*base*/ 10_000, /*skills*/ 0,
        /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 40_000,
    );
    let usage = TurnUsage {
        query: UsageTotals {
            total_tokens: 320_000,
            input_tokens: 300_000,
            output_tokens: 20_000,
            metered_call_count: 1,
            ..UsageTotals::default()
        },
        overhead: UsageTotals::default(),
    };

    let (last_turn, last_input) = super::resume_context_pressure_tokens(
        Some(&occupancy),
        Some(&usage),
        /*prompt_token_estimate*/ 12_000,
    );
    assert_eq!(last_turn, 50_000);
    assert_eq!(last_input, 300_000);

    let (last_turn, last_input) = super::resume_context_pressure_tokens(
        /*occupancy*/ None,
        Some(&usage),
        /*prompt_token_estimate*/ 12_000,
    );
    assert_eq!(last_turn, 320_000);
    assert_eq!(last_input, 300_000);

    let (last_turn, last_input) = super::resume_context_pressure_tokens(
        /*occupancy*/ None, /*latest_query_usage*/ None,
        /*prompt_token_estimate*/ 12_000,
    );
    assert_eq!(last_turn, 12_000);
    assert_eq!(last_input, 12_000);
}

#[test]
fn replay_preserves_latest_query_usage_when_latest_turn_has_no_usage() {
    let now = Utc.with_ymd_and_hms(2026, 7, 8, 10, 0, 0).unwrap();
    let session_id = SessionId::new();
    let mut replay = ReplayState::default();
    let usage = turn_usage(30, 12, 42);

    let mut completed = model_turn(
        session_id,
        TurnId::new(),
        1,
        now,
        TurnStatus::Completed,
        "model-a",
    );
    completed.usage = Some(usage.clone());
    completed.latest_query_usage = Some(usage.clone());
    apply_turn_line(&mut replay, now, completed);
    let mut failed = model_turn(
        session_id,
        TurnId::new(),
        2,
        now,
        TurnStatus::Failed,
        "model-a",
    );
    failed.failure_reason = Some(devo_protocol::TurnFailureReason::MaxTurnRequests);
    apply_turn_line(&mut replay, now, failed);

    assert_eq!(replay.latest_query_usage, Some(usage.to_native()));
    assert_eq!(replay.last_turn_tokens, 42);
    assert_eq!(replay.last_input_tokens, 30);
}

#[test]
fn replay_does_not_promote_aggregate_turn_usage_to_latest_query_usage() {
    let now = Utc.with_ymd_and_hms(2026, 7, 8, 10, 0, 0).unwrap();
    let session_id = SessionId::new();
    let aggregate_usage = turn_usage(10_000, 2_000, 12_000);
    let mut replay = ReplayState::default();

    let mut turn = model_turn(
        session_id,
        TurnId::new(),
        1,
        now,
        TurnStatus::Completed,
        "model-a",
    );
    turn.usage = Some(aggregate_usage);
    apply_turn_line(&mut replay, now, turn);

    assert_eq!(replay.latest_query_usage, None);
    assert_eq!(replay.last_turn_tokens, 0);
    assert_eq!(replay.last_input_tokens, 0);
}

#[test]
fn replay_prunes_superseded_turn_from_rollout_projection() {
    let now = Utc.with_ymd_and_hms(2026, 6, 18, 8, 0, 0).unwrap();
    let session_id = SessionId::new();
    let original_turn_id = TurnId::new();
    let replacement_turn_id = TurnId::new();
    let original_item_id = ItemId::new();
    let replacement_item_id = ItemId::new();
    let edit_id = EditId::new();
    let mut replay = ReplayState::default();

    apply_turn_line(
        &mut replay,
        now,
        model_turn(
            session_id,
            original_turn_id,
            1,
            now,
            TurnStatus::Completed,
            "model-a",
        ),
    );
    replay
        .apply_line(LegacyRolloutLine::Item(Box::new(ItemLine {
            timestamp: now,
            item: ItemRecord {
                id: original_item_id,
                session_id,
                turn_id: original_turn_id,
                seq: 1,
                timestamp: now,
                started_at: None,
                attempt_placement: None,
                turn_status: Some(TurnStatus::Completed),
                sibling_turn_ids: Vec::new(),
                input_items: vec![TurnItem::UserMessage(TextItem::text("original"))],
                output_items: Vec::new(),
                worklog: None,
                error: None,
                schema_version: 1,
            },
        })))
        .expect("apply original item");
    replay
        .apply_line(LegacyRolloutLine::MessageEditRecorded(Box::new(
            MessageEditRecordedLine {
                timestamp: now,
                record: MessageEditRecordedRecord {
                    schema_version: 1,
                    session_id,
                    edit_id,
                    target_message_id: original_item_id,
                    replacement_message_id: replacement_item_id,
                    target_turn_id: Some(original_turn_id),
                    replacement_turn_id: Some(replacement_turn_id),
                    queue_item_id: None,
                    edited_content_parts: vec![ContentPart::Text("edited".into())],
                    edited_mentions: Vec::new(),
                    workspace_restore_policy: WorkspaceRestorePolicy::Skip,
                    edit_state: EditState::Accepted,
                    requested_by_client_id: None,
                    created_at: now,
                },
            },
        )))
        .expect("apply message edit line");
    replay
        .apply_line(LegacyRolloutLine::TurnSuperseded(Box::new(
            TurnSupersededLine {
                timestamp: now,
                record: TurnSupersededRecord {
                    schema_version: 1,
                    session_id,
                    superseded_turn_id: original_turn_id,
                    replacement_turn_id,
                    edit_id,
                    restore_id: None,
                    reason: "message_edit_previous".into(),
                    created_at: now,
                },
            },
        )))
        .expect("apply superseded line");
    let mut replacement = model_turn(
        session_id,
        replacement_turn_id,
        2,
        now,
        TurnStatus::Running,
        "model-a",
    );
    replacement.completed_at = None;
    apply_turn_line(&mut replay, now, replacement);
    replay
        .apply_line(LegacyRolloutLine::Item(Box::new(ItemLine {
            timestamp: now,
            item: ItemRecord {
                id: replacement_item_id,
                session_id,
                turn_id: replacement_turn_id,
                seq: 2,
                timestamp: now,
                started_at: None,
                attempt_placement: None,
                turn_status: Some(TurnStatus::Running),
                sibling_turn_ids: Vec::new(),
                input_items: vec![TurnItem::UserMessage(TextItem::text("edited"))],
                output_items: Vec::new(),
                worklog: None,
                error: None,
                schema_version: 1,
            },
        })))
        .expect("apply replacement item");

    let projected_items = replay
        .pending_items
        .iter()
        .filter_map(|item| match &item.payload {
            ReplayHistoryItemPayload::NativeItem(native_item) => Some(native_item.clone()),
            ReplayHistoryItemPayload::HistoryOnly(_) => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        projected_items,
        vec![crate::persisted_native_item::user_message_item(
            "edited",
            &[],
            UserMessageEntry::TurnStart,
        )]
    );
    assert_eq!(replay.turn_order, vec![replacement_turn_id]);
    assert_eq!(
        replay
            .latest_turn
            .as_ref()
            .map(|turn| turn.legacy_turn_id()),
        Some(replacement_turn_id)
    );
}

#[test]
fn root_and_child_rollout_paths_match_agent_layout() {
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let store = super::RolloutStore::new(dir.path().to_path_buf(), None);
    let root_id = SessionId::new();
    let child_id = SessionId::new();
    let grandchild_id = SessionId::new();

    let root_path = store.allocate_rollout_path(&root_id);
    assert_eq!(
        root_path,
        dir.path().join("sessions").join(format!("{root_id}.jsonl"))
    );

    let child_path = store
        .allocate_child_rollout_path(Some(&root_path), &root_id, &child_id)
        .expect("child path");
    let child_rel = child_path
        .strip_prefix(dir.path())
        .expect("child under data root");
    let child_parts: Vec<_> = child_rel
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert_eq!(child_parts[0], "session-artifacts");
    assert_eq!(child_parts[1], root_id.to_string());
    assert!(
        child_parts[2].starts_with("sub-") && child_parts[2].len() == 12,
        "expected sub-xxxxxxxx, got {}",
        child_parts[2]
    );
    assert_eq!(child_parts[3], format!("{child_id}.jsonl"));

    // Local harness / RLM_SESSION_DIR must be per-session artifacts, not sessions/.
    assert_eq!(
        super::RolloutStore::rlm_session_dir_for_rollout(&root_path),
        Some(
            dir.path()
                .join("session-artifacts")
                .join(root_id.to_string())
        )
    );
    assert_eq!(
        super::RolloutStore::rlm_session_dir_for_rollout(&child_path),
        child_path.parent().map(|p| p.to_path_buf())
    );

    let grandchild_path = store
        .allocate_child_rollout_path(Some(&child_path), &child_id, &grandchild_id)
        .expect("grandchild path");
    let grand_rel = grandchild_path
        .strip_prefix(dir.path())
        .expect("grandchild under data root");
    let grand_parts: Vec<_> = grand_rel
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert_eq!(grand_parts[0], "session-artifacts");
    assert_eq!(grand_parts[1], root_id.to_string());
    assert_eq!(grand_parts[2], child_parts[2]);
    assert!(
        grand_parts[3].starts_with("sub-") && grand_parts[3].len() == 12,
        "expected nested sub-xxxxxxxx, got {}",
        grand_parts[3]
    );
    assert_eq!(grand_parts[4], format!("{grandchild_id}.jsonl"));

    // Index scans both trees.
    std::fs::create_dir_all(root_path.parent().unwrap()).unwrap();
    std::fs::write(&root_path, "{}\n").unwrap();
    std::fs::write(&child_path, "{}\n").unwrap();
    // Harness sidecar state under the artifact dir is not a rollout; the boot
    // index/reconcile scans must not pick it up (they would warn "missing
    // SessionMeta line" on every server start).
    let harness_dir = dir
        .path()
        .join("session-artifacts")
        .join(root_id.to_string())
        .join("harness");
    std::fs::create_dir_all(&harness_dir).unwrap();
    std::fs::write(harness_dir.join("refinements.jsonl"), "{}\n").unwrap();
    let paths = store.rollout_paths().expect("list");
    assert!(paths.contains(&root_path));
    assert!(paths.contains(&child_path));
    assert!(!paths.iter().any(|p| p.ends_with("refinements.jsonl")));
}

#[test]
fn index_rollout_metadata_reads_session_meta_only() {
    use chrono::Utc;
    use pretty_assertions::assert_eq;
    use std::io::Write;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let data_root = dir.path().to_path_buf();
    let session_id = SessionId::new();
    let now = Utc::now();
    let rollout_store = super::RolloutStore::new(data_root.clone(), None);
    let record = sample_record_for(
        &rollout_store,
        session_id,
        now,
        data_root.clone(),
        Some("Indexed session"),
    );
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&record.rollout_path)
        .expect("open rollout")
        .write_all(b"{\"type\":\"turn\",\"payload\":{}}\n")
        .expect("append non-meta line");

    let db = crate::db::Database::open(data_root.join("index.db")).expect("open db");
    rollout_store
        .index_rollout_metadata(&db)
        .expect("index rollout metadata");

    let index = db
        .get_session_index(&session_id)
        .expect("get index")
        .expect("indexed session");
    assert_eq!(index.session.title.as_deref(), Some("Indexed session"));
    assert_eq!(index.rollout_path, Some(record.rollout_path));
    assert_eq!(index.session.parent_session_id, None);

    let roots = db.list_root_sessions().expect("list roots");
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].session_id, session_id);
}

#[test]
fn index_rollout_metadata_uses_latest_item_timestamp_as_last_activity() {
    use chrono::Duration;
    use chrono::Utc;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let data_root = dir.path().to_path_buf();
    let session_id = SessionId::new();
    let created_at = Utc::now() - Duration::hours(2);
    let rollout_store = super::RolloutStore::new(data_root.clone(), None);
    let mut record = sample_record_for(
        &rollout_store,
        session_id,
        created_at,
        data_root.clone(),
        Some("Active session"),
    );
    record.created_at = created_at;
    record.updated_at = created_at;
    record.last_activity_at = Some(created_at);
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");
    let before_item = Utc::now();
    rollout_store
        .append_item(
            &record,
            ItemRecord {
                id: ItemId::new(),
                session_id,
                turn_id: TurnId::new(),
                seq: 1,
                timestamp: before_item,
                started_at: Some(before_item),
                attempt_placement: None,
                turn_status: Some(TurnStatus::Completed),
                sibling_turn_ids: Vec::new(),
                input_items: Vec::new(),
                output_items: vec![TurnItem::AgentMessage(TextItem::text("reply"))],
                worklog: None,
                error: None,
                schema_version: 1,
            },
        )
        .expect("append item");

    let db = crate::db::Database::open(data_root.join("index.db")).expect("open db");
    rollout_store
        .index_rollout_metadata(&db)
        .expect("index rollout metadata");

    let index = db
        .get_session_index(&session_id)
        .expect("get index")
        .expect("indexed session");
    assert_eq!(index.session.created_at.timestamp(), created_at.timestamp());
    assert!(
        index.session.last_activity_at.timestamp() >= before_item.timestamp(),
        "last_activity_at={:?} before_item={:?}",
        index.session.last_activity_at,
        before_item
    );
    assert!(index.session.last_activity_at > index.session.created_at);
}

#[test]
fn index_rollout_metadata_overwrites_stale_sqlite_title_when_rollout_has_title() {
    use chrono::Utc;
    use devo_protocol::SessionTitleState;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let data_root = dir.path().to_path_buf();
    let session_id = SessionId::new();
    let now = Utc::now();
    let rollout_store = super::RolloutStore::new(data_root.clone(), None);
    let record = sample_record_for(&rollout_store, session_id, now, data_root.clone(), None);
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");
    rollout_store
        .append_title_update(
            &record,
            "Canonical rollout title".into(),
            SessionTitleState::Final(devo_core::SessionTitleFinalSource::ModelGenerated),
            None,
        )
        .expect("append title update");

    let db = crate::db::Database::open(data_root.join("index.db")).expect("open db");
    let mut index_row = super::session_index_row_from_record(&record, now);
    index_row.title = Some("Existing title".into());
    index_row.title_state =
        SessionTitleState::Final(devo_core::SessionTitleFinalSource::ModelGenerated);
    db.upsert_session(index_row, None)
        .expect("seed sqlite title");

    rollout_store
        .index_rollout_metadata(&db)
        .expect("index rollout metadata");

    let index = db
        .get_session_index(&session_id)
        .expect("get index")
        .expect("indexed session");
    assert_eq!(
        index.session.title.as_deref(),
        Some("Canonical rollout title")
    );
}

#[test]
fn index_rollout_metadata_reads_session_title_updates() {
    use chrono::Utc;
    use devo_core::SessionTitleFinalSource;
    use devo_core::SessionTitleState;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let data_root = dir.path().to_path_buf();
    let session_id = SessionId::new();
    let now = Utc::now();
    let rollout_store = super::RolloutStore::new(data_root.clone(), None);
    let record = sample_record_for(&rollout_store, session_id, now, data_root.clone(), None);
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");
    rollout_store
        .append_title_update(
            &record,
            "Updated from rollout".into(),
            SessionTitleState::Final(SessionTitleFinalSource::ModelGenerated),
            None,
        )
        .expect("append title update");

    let db = crate::db::Database::open(data_root.join("index.db")).expect("open db");
    rollout_store
        .index_rollout_metadata(&db)
        .expect("index rollout metadata");

    let index = db
        .get_session_index(&session_id)
        .expect("get index")
        .expect("indexed session");
    assert_eq!(index.session.title.as_deref(), Some("Updated from rollout"));
}

#[test]
fn replay_omits_empty_agent_messages_from_history_and_prompt() {
    let mut messages = Vec::new();
    let mut history_items = Vec::new();
    let mut tool_names_by_id = HashMap::new();

    for item in [
        TurnItem::UserMessage(TextItem::text("hello")),
        TurnItem::AgentMessage(TextItem::text(String::new())),
        TurnItem::AgentMessage(TextItem::text("  \n\t")),
        TurnItem::AgentMessage(TextItem::text("visible answer")),
    ] {
        apply_turn_item(
            &mut messages,
            &mut history_items,
            &mut tool_names_by_id,
            item,
        );
    }

    assert_eq!(
        history_items,
        vec![
            crate::SessionHistoryEntry::item(devo_protocol::native::item::Item::UserMessage {
                client_user_message_id: None,
                content: vec![devo_protocol::native::item::UserInput::Text {
                    text: "hello".to_string(),
                }],
                entry: devo_protocol::native::item::UserMessageEntry::default(),
            }),
            crate::SessionHistoryEntry::item(devo_protocol::native::item::Item::AssistantMessage {
                text: "visible answer".to_string(),
            },),
        ]
    );
    assert_eq!(
        messages,
        vec![
            Message::user("hello"),
            Message::assistant_text("visible answer"),
        ]
    );
}

#[test]
fn replay_backfills_tool_result_name_from_prior_tool_call() {
    let mut messages = Vec::new();
    let mut history_items = Vec::new();
    let mut tool_names_by_id = HashMap::new();

    apply_turn_item(
        &mut messages,
        &mut history_items,
        &mut tool_names_by_id,
        TurnItem::ToolCall(ToolCallItem {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            input: serde_json::json!({"filePath":"/tmp/test.txt"}),
        }),
    );
    apply_turn_item(
        &mut messages,
        &mut history_items,
        &mut tool_names_by_id,
        TurnItem::ToolResult(ToolResultItem {
            tool_call_id: "call-1".to_string(),
            tool_name: None,
            output: serde_json::Value::String("hello".to_string()),
            display_content: None,
            is_error: false,
        }),
    );

    assert_eq!(history_items.len(), 2);
    assert!(matches!(
        &history_items[0],
        crate::SessionHistoryEntry::Item {
            item: devo_protocol::native::item::Item::ToolCall { tool_name, .. }
        } if tool_name == "read"
    ));
    assert!(matches!(
        &history_items[1],
        crate::SessionHistoryEntry::Item {
            item: devo_protocol::native::item::Item::ToolResult {
                call_id,
                is_error: false,
                ..
            }
        } if call_id == "call-1"
    ));
}

#[test]
fn replay_nameless_edit_tool_result_still_emits_edited_metadata() {
    // edit is LiveOnly: start is not persisted, so resume only sees ToolResult
    // with tool_name lost by the v2 canonical schema. History stores Native ToolResult.
    let mut messages = Vec::new();
    let mut history_items = Vec::new();
    let mut tool_names_by_id = HashMap::new();

    apply_turn_item(
        &mut messages,
        &mut history_items,
        &mut tool_names_by_id,
        TurnItem::ToolResult(ToolResultItem {
            tool_call_id: "call-edit".to_string(),
            tool_name: None,
            output: serde_json::json!({
                "diff": "diff --git a/foo.txt b/foo.txt\n--- a/foo.txt\n+++ b/foo.txt\n@@ -1 +1 @@\n-old\n+new\n",
                "files": [{
                    "path": "foo.txt",
                    "kind": "update",
                    "diff": "--- a/foo.txt\n+++ b/foo.txt\n@@ -1 +1 @@\n-old\n+new\n",
                    "oldContent": "old\n",
                    "postContent": "new\n",
                    "additions": 1,
                    "deletions": 1
                }],
                "output": "edited foo.txt"
            }),
            display_content: Some("edited foo.txt".to_string()),
            is_error: false,
        }),
    );

    assert_eq!(history_items.len(), 1);
    let crate::SessionHistoryEntry::Item {
        item:
            devo_protocol::native::item::Item::ToolResult {
                display_content,
                output,
                ..
            },
    } = &history_items[0]
    else {
        panic!("expected Native ToolResult history entry");
    };
    assert_eq!(display_content.as_deref(), Some("edited foo.txt"));
    assert!(output.get("files").is_some());
}

#[test]
fn replay_uses_display_content_for_history_but_canonical_output_for_prompt() {
    let mut messages = Vec::new();
    let mut history_items = Vec::new();
    let mut tool_names_by_id = HashMap::new();

    apply_turn_item(
        &mut messages,
        &mut history_items,
        &mut tool_names_by_id,
        TurnItem::ToolCall(ToolCallItem {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            input: serde_json::json!({"filePath":"/tmp/test.txt"}),
        }),
    );
    apply_turn_item(
        &mut messages,
        &mut history_items,
        &mut tool_names_by_id,
        TurnItem::ToolResult(ToolResultItem {
            tool_call_id: "call-1".to_string(),
            tool_name: Some("read".to_string()),
            output: serde_json::Value::String("<content>canonical</content>".to_string()),
            display_content: Some("canonical".to_string()),
            is_error: false,
        }),
    );

    let crate::SessionHistoryEntry::Item {
        item:
            devo_protocol::native::item::Item::ToolResult {
                display_content, ..
            },
    } = &history_items[1]
    else {
        panic!("expected ToolResult history entry");
    };
    assert_eq!(display_content.as_deref(), Some("canonical"));
    assert_eq!(
        messages.last(),
        Some(&Message {
            role: devo_core::Role::User,
            content: vec![devo_core::ContentBlock::ToolResult {
                tool_use_id: "call-1".to_string(),
                content: "<content>canonical</content>".to_string(),
                is_error: false,
            }],
        })
    );
}

#[test]
fn prompt_messages_rebuild_from_compaction_snapshot_without_trimming_transcript() {
    let summary_item_id = ItemId::new();
    let preserved_item_id = ItemId::new();
    let later_item_id = ItemId::new();

    let persisted_turn_items = vec![
        PersistedNativeItem::new(
            NativeTurnId::new(),
            devo_protocol::native::turn::TurnKind::Regular,
            NativeItemId::new(),
            crate::persisted_native_item::user_message_item(
                "older user",
                &[],
                UserMessageEntry::TurnStart,
            ),
        ),
        PersistedNativeItem::new(
            NativeTurnId::new(),
            devo_protocol::native::turn::TurnKind::Regular,
            summary_item_id,
            crate::persisted_native_item::context_compaction_item(
                "<compaction_summary>summary</compaction_summary>",
                devo_protocol::native::item::CompactionTrigger::AutoThreshold,
            ),
        ),
        PersistedNativeItem::new(
            NativeTurnId::new(),
            devo_protocol::native::turn::TurnKind::Regular,
            preserved_item_id,
            crate::persisted_native_item::user_message_item(
                "latest user",
                &[],
                UserMessageEntry::TurnStart,
            ),
        ),
        PersistedNativeItem::new(
            NativeTurnId::new(),
            devo_protocol::native::turn::TurnKind::Regular,
            later_item_id,
            NativeItem::AssistantMessage {
                text: "latest assistant".to_string(),
            },
        ),
    ];

    let prompt_messages = build_prompt_messages_from_snapshot(
        &persisted_turn_items,
        &CompactionSnapshotLine {
            timestamp: Utc::now(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            summary_item_id,
            preserved_item_ids: vec![preserved_item_id],
            context_occupancy: None,
        },
    )
    .expect("prompt messages");

    assert_eq!(
        prompt_messages,
        vec![
            Message::assistant_text("<compaction_summary>summary</compaction_summary>"),
            Message::user("latest user"),
            Message::assistant_text("latest assistant"),
        ]
    );
}

#[test]
fn compaction_resume_rebuild_preserves_tool_request_with_its_result() {
    // End-to-end over the two functions production composes: the compaction
    // handler derives preserved item ids from the compactor's verbatim
    // suffix (preserved_item_ids_from_compacted), and resume rebuilds the
    // prompt from those ids (build_prompt_messages_from_snapshot). When the
    // suffix carries an ordinary ToolCall + ToolResult pair journaled as two
    // items, BOTH must survive — a rebuilt prompt with a tool result but no
    // preceding tool use is rejected by the provider.
    let user_id = NativeItemId::new();
    let request_id = NativeItemId::new();
    let result_id = NativeItemId::new();
    let assistant_id = NativeItemId::new();
    let summary_item_id = NativeItemId::new();
    let later_item_id = NativeItemId::new();
    let next = || NativeItemId::new();
    let _ = next();

    let persisted_turn_items = vec![
        PersistedNativeItem::new(
            NativeTurnId::new(),
            devo_protocol::native::turn::TurnKind::Regular,
            user_id,
            crate::persisted_native_item::user_message_item(
                "run the tool",
                &[],
                UserMessageEntry::TurnStart,
            ),
        ),
        PersistedNativeItem::new(
            NativeTurnId::new(),
            devo_protocol::native::turn::TurnKind::Regular,
            request_id,
            NativeItem::ToolCall {
                call_id: "call-9".to_string(),
                tool_name: "bash".to_string(),
                source: devo_protocol::native::item::ToolSource::Builtin,
                server_name: None,
                input: Some(serde_json::json!({ "command": "echo hi" })),
            },
        ),
        PersistedNativeItem::new(
            NativeTurnId::new(),
            devo_protocol::native::turn::TurnKind::Regular,
            result_id,
            NativeItem::ToolResult {
                call_id: "call-9".to_string(),
                output: serde_json::Value::String("hi".to_string()),
                display_content: None,
                is_error: false,
                truncated: false,
            },
        ),
        PersistedNativeItem::new(
            NativeTurnId::new(),
            devo_protocol::native::turn::TurnKind::Regular,
            assistant_id,
            NativeItem::AssistantMessage {
                text: "done".to_string(),
            },
        ),
        PersistedNativeItem::new(
            NativeTurnId::new(),
            devo_protocol::native::turn::TurnKind::Regular,
            summary_item_id,
            crate::persisted_native_item::context_compaction_item(
                "<compaction_summary>summary</compaction_summary>",
                devo_protocol::native::item::CompactionTrigger::Manual,
            ),
        ),
        // A post-compaction turn the snapshot must keep verbatim.
        PersistedNativeItem::new(
            NativeTurnId::new(),
            devo_protocol::native::turn::TurnKind::Regular,
            later_item_id,
            crate::persisted_native_item::user_message_item(
                "after compact",
                &[],
                UserMessageEntry::TurnStart,
            ),
        ),
    ];

    // The compactor's response: summary + verbatim suffix with the tool pair.
    let compacted_items = vec![
        devo_core::ResponseItem::Message(devo_core::Message::assistant_text("summary")),
        devo_core::ResponseItem::Message(devo_core::Message::user("run the tool")),
        devo_core::ResponseItem::ToolCall {
            id: "call-9".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({ "command": "echo hi" }),
        },
        devo_core::ResponseItem::ToolCallOutput {
            tool_use_id: "call-9".to_string(),
            content: "hi".to_string(),
            is_error: false,
        },
        devo_core::ResponseItem::Message(devo_core::Message::assistant_text("done")),
    ];
    let preserved_item_ids =
        crate::runtime::preserved_item_ids_from_compacted(&persisted_turn_items, &compacted_items);
    assert_eq!(
        preserved_item_ids,
        vec![user_id, request_id, result_id, assistant_id],
        "preserved ids must include the ToolCall request item, not just its result"
    );

    let prompt_messages = build_prompt_messages_from_snapshot(
        &persisted_turn_items,
        &CompactionSnapshotLine {
            timestamp: Utc::now(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            summary_item_id,
            preserved_item_ids,
            context_occupancy: None,
        },
    )
    .expect("prompt messages");

    let tool_use_count = prompt_messages
        .iter()
        .flat_map(|message| &message.content)
        .filter(|block| {
            matches!(
                block,
                devo_core::ContentBlock::ToolUse {
                    id,
                    ..
                } if id == "call-9"
            )
        })
        .count();
    let tool_result_count = prompt_messages
        .iter()
        .flat_map(|message| &message.content)
        .filter(|block| {
            matches!(
                block,
                devo_core::ContentBlock::ToolResult {
                    tool_use_id,
                    ..
                } if tool_use_id == "call-9"
            )
        })
        .count();
    assert_eq!(
        (tool_use_count, tool_result_count),
        (1, 1),
        "rebuilt prompt must carry the tool use and its result together: {prompt_messages:?}"
    );
    // The post-compaction turn survives after the preserved suffix.
    assert!(
        prompt_messages.contains(&Message::user("after compact")),
        "post-compaction items must follow the preserved suffix: {prompt_messages:?}"
    );
}

#[test]
fn replay_restores_context_snapshots_from_turn_records() {
    let session_id = SessionId::new();
    let turn_id = TurnId::new();
    let now = Utc.with_ymd_and_hms(2026, 4, 27, 8, 0, 0).unwrap();
    let session_context = SessionContext {
        base_instructions: "base".into(),
        available_skills: None,
        workspace_instructions: Some("workspace".into()),
        locked_agents_snapshot: None,
        environment: EnvironmentContext {
            cwd: PathBuf::from("/tmp/root"),
            shell: "bash".into(),
            current_date: "2026-04-27".into(),
            timezone: "UTC".into(),
        },
        language: LanguageContext::default(),
        persona: Persona::Default,
        model: Model {
            slug: "model-a".into(),
            ..Model::default()
        },
        reasoning_effort_selection: None,
        reasoning_effort: None,
        system_prompt_mode: devo_core::SystemPromptMode::CodingAgent,
    };
    let turn_context = TurnContext {
        environment: EnvironmentContext {
            cwd: PathBuf::from("/tmp/next"),
            shell: "bash".into(),
            current_date: "2026-04-28".into(),
            timezone: "UTC".into(),
        },
        persona: Persona::Default,
        model: Model {
            slug: "model-b".into(),
            ..Model::default()
        },
        reasoning_effort_selection: Some("enabled".into()),
        reasoning_effort: None,
        observed_agents_snapshot: None,
        collaboration_mode: devo_core::CollaborationMode::Build,
    };
    let mut replay = ReplayState::default();

    apply_session_meta(&mut replay, now, sample_replay_session(session_id, now));
    let mut turn = model_turn(
        session_id,
        turn_id,
        1,
        now,
        TurnStatus::Completed,
        "model-b",
    );
    turn.reasoning_effort_selection = Some("enabled".into());
    turn.request_thinking = Some("enabled".into());
    turn.session_context = Some(session_context.clone());
    turn.turn_context = Some(turn_context.clone());
    apply_turn_line(&mut replay, now, turn);

    assert_eq!(replay.session_context, Some(session_context));
    assert_eq!(replay.latest_turn_context, Some(turn_context));
    assert!(replay.session_context_recorded);
}

fn sample_replay_session(session_id: SessionId, now: chrono::DateTime<Utc>) -> SessionRecord {
    SessionRecord {
        id: session_id,
        rollout_path: PathBuf::from("rollout.jsonl"),
        created_at: now,
        updated_at: now,
        last_activity_at: Some(now),
        source: "cli".into(),
        agent_nickname: None,
        agent_role: None,
        agent_path: None,
        model_provider: "test".into(),
        model: Some("model-a".into()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        cwd: PathBuf::from("/tmp/root"),
        additional_directories: Vec::new(),
        cli_version: "0.1.0".into(),
        title: None,
        title_state: SessionTitleState::Unset,
        sandbox_policy: "workspace-write".into(),
        approval_mode: "on-request".into(),
        effective_context_window: None,
        tokens_used: 0,
        first_user_message: None,
        archived_at: None,
        git_sha: None,
        git_branch: None,
        git_origin_url: None,
        parent_session_id: None,
        fork_from_id: None,
        fork_at_turn_id: None,
        session_context: None,
        latest_turn_context: None,
        collaboration_mode: None,
        permission_preset: None,
        schema_version: 2,
    }
}

fn apply_session_meta(
    replay: &mut ReplayState,
    timestamp: chrono::DateTime<Utc>,
    session: SessionRecord,
) {
    replay
        .apply_line(LegacyRolloutLine::SessionMeta(Box::new(SessionMetaLine {
            timestamp,
            session,
        })))
        .expect("apply session meta");
}

fn sample_session_context(base_instructions: &str) -> SessionContext {
    SessionContext {
        base_instructions: base_instructions.into(),
        available_skills: None,
        workspace_instructions: Some("workspace".into()),
        locked_agents_snapshot: None,
        environment: EnvironmentContext {
            cwd: PathBuf::from("/tmp/root"),
            shell: "bash".into(),
            current_date: "2026-04-27".into(),
            timezone: "UTC".into(),
        },
        language: LanguageContext::default(),
        persona: Persona::Default,
        model: Model {
            slug: "model-a".into(),
            ..Model::default()
        },
        reasoning_effort_selection: None,
        reasoning_effort: None,
        system_prompt_mode: devo_core::SystemPromptMode::CodingAgent,
    }
}

#[test]
fn replay_restores_context_from_session_context_updated_line() {
    let session_id = SessionId::new();
    let turn_id = TurnId::new();
    let now = Utc.with_ymd_and_hms(2026, 4, 27, 8, 0, 0).unwrap();
    let session_context = sample_session_context("base");
    let turn_context = TurnContext {
        environment: EnvironmentContext {
            cwd: PathBuf::from("/tmp/next"),
            shell: "bash".into(),
            current_date: "2026-04-28".into(),
            timezone: "UTC".into(),
        },
        persona: Persona::Default,
        model: Model {
            slug: "model-b".into(),
            ..Model::default()
        },
        reasoning_effort_selection: Some("enabled".into()),
        reasoning_effort: None,
        observed_agents_snapshot: None,
        collaboration_mode: devo_core::CollaborationMode::Build,
    };
    let mut replay = ReplayState::default();

    apply_session_meta(&mut replay, now, sample_replay_session(session_id, now));
    replay
        .apply_line(LegacyRolloutLine::SessionContextUpdated(Box::new(
            devo_core::SessionContextUpdatedLine {
                timestamp: now,
                session_id,
                session_context: session_context.clone(),
                schema_version: 1,
            },
        )))
        .expect("apply session context");
    let mut turn = model_turn(
        session_id,
        turn_id,
        1,
        now,
        TurnStatus::Completed,
        "model-b",
    );
    turn.reasoning_effort_selection = Some("enabled".into());
    turn.request_thinking = Some("enabled".into());
    turn.turn_context = Some(turn_context.clone());
    apply_turn_line(&mut replay, now, turn);

    assert_eq!(replay.session_context, Some(session_context));
    assert_eq!(replay.latest_turn_context, Some(turn_context));
    assert!(replay.session_context_recorded);
}

#[test]
fn replay_preserves_session_context_updated_across_rollback() {
    let session_id = SessionId::new();
    let turn_id = TurnId::new();
    let now = Utc.with_ymd_and_hms(2026, 4, 27, 8, 0, 0).unwrap();
    let session_context = sample_session_context("base");
    let turn_context = TurnContext {
        environment: EnvironmentContext {
            cwd: PathBuf::from("/tmp/next"),
            shell: "bash".into(),
            current_date: "2026-04-28".into(),
            timezone: "UTC".into(),
        },
        persona: Persona::Default,
        model: Model {
            slug: "model-b".into(),
            ..Model::default()
        },
        reasoning_effort_selection: Some("enabled".into()),
        reasoning_effort: None,
        observed_agents_snapshot: None,
        collaboration_mode: devo_core::CollaborationMode::Build,
    };
    let mut replay = ReplayState::default();

    apply_session_meta(&mut replay, now, sample_replay_session(session_id, now));
    replay
        .apply_line(LegacyRolloutLine::SessionContextUpdated(Box::new(
            devo_core::SessionContextUpdatedLine {
                timestamp: now,
                session_id,
                session_context: session_context.clone(),
                schema_version: 1,
            },
        )))
        .expect("apply session context");
    let mut turn = model_turn(
        session_id,
        turn_id,
        1,
        now,
        TurnStatus::Completed,
        "model-b",
    );
    turn.reasoning_effort_selection = Some("enabled".into());
    turn.request_thinking = Some("enabled".into());
    turn.turn_context = Some(turn_context);
    apply_turn_line(&mut replay, now, turn);
    replay
        .apply_line(LegacyRolloutLine::SessionRollback(Box::new(
            SessionRollbackLine {
                timestamp: now,
                session_id,
                retained_turn_ids: Vec::new(),
                retained_item_ids: Vec::new(),
                latest_turn_id: None,
                schema_version: 1,
            },
        )))
        .expect("apply rollback");

    assert_eq!(replay.session_context, Some(session_context));
    assert!(replay.session_context_recorded);
    assert!(replay.latest_turn.is_none());
}

#[test]
fn append_turn_deduped_writes_session_context_once() {
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let data_root = dir.path().to_path_buf();
    let session_id = SessionId::new();
    let now = Utc::now();
    let rollout_store = super::RolloutStore::new(data_root.clone(), None);
    let record = sample_record_for(
        &rollout_store,
        session_id,
        now,
        data_root.clone(),
        Some("dedupe test"),
    );
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");

    let session_context = sample_session_context("unique-base-instruction-marker");
    let mut session_context_recorded = false;

    for sequence in 1..=2 {
        let turn = test_turn_at(
            session_id,
            TurnId::new(),
            sequence,
            now,
            TurnStatus::Completed,
        );
        rollout_store
            .append_turn_deduped(
                &record,
                &mut session_context_recorded,
                turn,
                Some(session_context.clone()),
            )
            .expect("append deduped turn");
    }

    assert!(session_context_recorded);
    let rollout = std::fs::read_to_string(&record.rollout_path).expect("read rollout");
    assert_eq!(rollout.matches("unique-base-instruction-marker").count(), 1);
    // v2 write path: the locked context travels as an internal line.
    assert!(rollout.contains("\"sessionContext\""));
}

// ── Versioned rollout parsing (P3b) ─────────────────────────────

fn test_deps(data_root: &std::path::Path) -> ServerRuntimeDependencies {
    crate::test_support::TestRuntime::noop().deps(data_root)
}

fn write_raw_lines(path: &std::path::Path, raw_lines: &[String]) {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create rollout directory");
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open rollout for raw append");
    for raw in raw_lines {
        file.write_all(raw.as_bytes()).expect("write raw line");
        file.write_all(b"\n").expect("write newline");
    }
}

fn raw_rollout_lines(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .expect("read rollout")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
fn write_path_appends_versioned_rollout_lines() {
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
    let record = sample_record(&rollout_store, dir.path().to_path_buf(), None);
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");
    let turn = test_turn_record(record.id, TurnId::new());
    rollout_store
        .append_turn(&record, turn.clone())
        .expect("append turn");
    let item = super::build_item_record(
        record.id,
        turn.id,
        ItemId::new(),
        1,
        TurnItem::AgentMessage(TextItem::text("hi")),
        Some(TurnStatus::Running),
        None,
        None,
    );
    rollout_store
        .append_item(&record, item)
        .expect("append item");

    let raw_lines = raw_rollout_lines(&record.rollout_path);
    assert_eq!(raw_lines.len(), 3);
    for raw in &raw_lines {
        assert!(raw.contains("\"v\":2"), "line is v2: {raw}");
        parse_rollout_line(raw).expect("line parses");
    }
}

#[test]
fn hydration_folds_approval_decision_onto_request_across_restart() {
    use devo_core::ApprovalDecisionItem;
    use devo_core::ApprovalRequestItem;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
    let record = sample_record(&rollout_store, dir.path().to_path_buf(), None);
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");
    let turn = test_turn_record(record.id, TurnId::new());
    rollout_store
        .append_turn(&record, turn.clone())
        .expect("append turn");
    let request_record_id = ItemId::new();
    rollout_store
        .append_item(
            &record.clone(),
            super::build_item_record(
                record.id,
                turn.id,
                request_record_id,
                1,
                TurnItem::ApprovalRequest(ApprovalRequestItem {
                    approval_id: "appr-1".into(),
                    action_summary: "Run ls".into(),
                    justification: "listing".into(),
                    resource: Some("ShellExec".into()),
                    available_scopes: vec!["once".into()],
                    command_pattern: None,
                    command_prefix: None,
                    path: None,
                    host: None,
                    target: Some("ls".into()),
                }),
                Some(TurnStatus::Running),
                None,
                None,
            ),
        )
        .expect("append approval request");

    // "Restart": a brand-new store must hydrate its write state from the
    // on-disk v2 history before appending.
    let restarted_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
    restarted_store
        .append_item(
            &record.clone(),
            super::build_item_record(
                record.id,
                turn.id,
                ItemId::new(),
                2,
                TurnItem::ApprovalDecision(ApprovalDecisionItem {
                    approval_id: "appr-1".into(),
                    decision: "approve".into(),
                    scope: "once".into(),
                    decision_source: None,
                }),
                Some(TurnStatus::Running),
                None,
                None,
            ),
        )
        .expect("append approval decision");

    let approvals: Vec<devo_core::RolloutLine> = raw_rollout_lines(&record.rollout_path)
        .iter()
        .map(|raw| parse_rollout_line(raw).expect("line parses"))
        .filter(|line| {
            matches!(
                line,
                devo_core::RolloutLine::Item { item, .. }
                    if matches!(item.item, devo_protocol::native::item::Item::Approval { .. })
            )
        })
        .collect();
    assert_eq!(approvals.len(), 2);
    let devo_core::RolloutLine::Item { item: request, .. } = &approvals[0] else {
        panic!("request line");
    };
    let devo_core::RolloutLine::Item { item: decision, .. } = &approvals[1] else {
        panic!("decision line");
    };
    // The decision folded onto the request's item id and seq — not an
    // orphan Warning with a fresh id.
    assert_eq!(request.id.as_str(), request_record_id.to_string());
    assert_eq!(decision.id, request.id);
    assert_eq!(decision.seq, request.seq);
    assert_eq!((request.revision, decision.revision), (1, 2));
    assert_eq!(
        decision.state,
        devo_protocol::native::item::ItemState::Completed
    );
    assert!(
        matches!(&decision.item, devo_protocol::native::item::Item::Approval { decision: Some(d), .. }
                if d.decision == devo_protocol::native::item::ApprovalDecisionKind::Approved
                    && d.scope == devo_protocol::native::item::ApprovalScope::Once)
    );
}

#[test]
fn canonical_only_interaction_and_file_change_items_round_trip() {
    use devo_protocol::native::ids::ItemId as CanonicalItemId;
    use devo_protocol::native::item::{
        FileChangeEntry, FileChangeKind, Item, ItemEnvelope, ItemState,
    };
    use tempfile::TempDir;
    use uuid::Uuid;

    let dir = TempDir::new().expect("temp dir");
    let store = super::RolloutStore::new(dir.path().to_path_buf(), None);
    let record = sample_record(&store, dir.path().to_path_buf(), None);
    store
        .append_session_meta(&record)
        .expect("append session meta");
    let turn_id = TurnId::new();
    let metadata = test_turn_record(record.id, turn_id);
    store.append_turn(&record, metadata).expect("append turn");
    let now = Utc::now();
    let session_id = record.id;
    let question_item_id = CanonicalItemId::from_legacy_uuid(Uuid::now_v7());
    let waiting = ItemEnvelope {
        id: question_item_id,
        session_id,
        turn_id,
        seq: 1,
        revision: 1,
        created_at: now,
        updated_at: now,
        state: ItemState::Waiting,
        item: Item::UserInputRequest {
            request_id: "question-1".into(),
            target_item_id: None,
            questions: Vec::new(),
            answers: None,
        },
        parent_id: None,
    };
    let file_change = ItemEnvelope {
        id: CanonicalItemId::from_legacy_uuid(Uuid::now_v7()),
        session_id,
        turn_id,
        seq: 2,
        revision: 1,
        created_at: now,
        updated_at: now,
        state: ItemState::Completed,
        item: Item::FileChange {
            call_id: "edit-1".into(),
            changes: vec![FileChangeEntry {
                path: PathBuf::from("src/lib.rs"),
                change: FileChangeKind::Update {
                    unified_diff: "@@ -1 +1 @@".into(),
                    move_path: None,
                },
            }],
            sandbox: None,
        },
        parent_id: None,
    };
    store
        .append_canonical_item(&record, waiting.clone())
        .expect("append waiting item");
    store
        .append_canonical_item(&record, file_change.clone())
        .expect("append file change");

    let history = devo_core::read_canonical_history(&record.rollout_path).expect("read history");
    assert_eq!(history.items, vec![waiting, file_change]);
}

#[test]
fn hydration_fails_closed_on_damaged_history() {
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
    let record = sample_record(&rollout_store, dir.path().to_path_buf(), None);
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");
    write_raw_lines(
        &record.rollout_path,
        &[r#"{"v":2,"kind":"nope"}"#.to_string()],
    );

    let restarted_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
    let metadata = test_turn_record(record.id, TurnId::new());
    let turn = metadata;
    let error = restarted_store
        .append_turn(&record, turn)
        .expect_err("append onto damaged history must fail");
    assert!(
        format!("{error:#}").contains("refusing to append"),
        "unexpected error: {error:#}"
    );
}

#[tokio::test]
async fn hydration_tolerates_truncated_final_line() {
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
    let record = sample_record(&rollout_store, dir.path().to_path_buf(), None);
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");
    write_raw_lines(
        &record.rollout_path,
        &[r#"{"v":2,"kind":"item","timestamp":"2026"#.to_string()],
    );

    let restarted_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
    let metadata = test_turn_record(record.id, TurnId::new());
    let turn = metadata;
    restarted_store
        .append_turn(&record, turn)
        .expect("crash tail is tolerated");
    let deps = test_deps(dir.path());
    restarted_store
        .load_session_from_rollout(&record.rollout_path, &deps)
        .await
        .expect("append after crash tail must remain loadable");
}

/// Trace: prompt-payload audit (R27, bug #43)
/// Verifies: a session restored from its rollout re-advertises the skills
/// currently on disk. The persisted `session_context` snapshot used to win
/// wholesale, so an existing session kept advertising the skills text
/// captured at creation — across every restart — even after the skill files
/// (or the bundled assets) changed.
#[tokio::test]
async fn resume_refreshes_available_skills_from_disk() {
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let deps = test_deps(dir.path());
    let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
    let record = sample_record(&rollout_store, dir.path().to_path_buf(), None);
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");
    // A journal whose turn carries a skills advertisement that no longer
    // matches what the workspace renders today.
    let stale_skills = "<available_skills>\nstale skills text\n</available_skills>";
    let mut session_context = sample_session_context("base");
    session_context.available_skills = Some(stale_skills.to_string());
    let mut turn = test_turn_record(record.id, TurnId::new());
    turn.session_context = Some(session_context);
    rollout_store
        .append_turn(&record, turn)
        .expect("append turn");

    let recovered = rollout_store
        .load_session_from_rollout(&record.rollout_path, &deps)
        .await
        .expect("load resumed session");
    let core = recovered.core_session.lock().await;
    let context = core.session_context.as_ref().expect("session context");
    assert_ne!(
        context.available_skills.as_deref(),
        Some(stale_skills),
        "resumed session must not re-advertise the stale journal snapshot"
    );
    assert_eq!(
        context.available_skills, core.config.available_skills_instructions,
        "skills advertisement is re-rendered from the current workspace"
    );
}

#[tokio::test]
async fn load_tolerates_truncated_crash_tail_with_trailing_blank_lines() {
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let deps = test_deps(dir.path());
    let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
    let record = sample_record(&rollout_store, dir.path().to_path_buf(), None);
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");
    write_raw_lines(
        &record.rollout_path,
        &[
            r#"{"v":2,"kind":"item","timestamp":"2026"#.to_string(),
            String::new(),
        ],
    );

    let recovered = rollout_store
        .load_session_from_rollout(&record.rollout_path, &deps)
        .await
        .expect("truncated crash tail with trailing blank must load");
    assert_eq!(recovered.summary.session_id(), record.id);
}

#[tokio::test]
async fn versioned_reader_fails_closed_on_mid_file_damage_but_tolerates_crash_tail() {
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let deps = test_deps(dir.path());
    let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
    let record = sample_record(&rollout_store, dir.path().to_path_buf(), None);
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");
    // Damaged middle line, then a valid line after it.
    write_raw_lines(
        &record.rollout_path,
        &[r#"{"v":2,"kind":"nope"}"#.to_string()],
    );
    let metadata = test_turn_record(record.id, TurnId::new());
    let turn = metadata;
    rollout_store
        .append_turn(&record, turn)
        .expect("append turn");

    let error = rollout_store
        .load_session_from_rollout(&record.rollout_path, &deps)
        .await
        .err()
        .expect("damaged mid-file line must fail the load");
    assert!(
        format!("{error:#}").contains("refusing to resume"),
        "unexpected error: {error:#}"
    );

    // A truncated final line (crash tail) is tolerated instead.
    let tail_record = sample_record(&rollout_store, dir.path().to_path_buf(), None);
    rollout_store
        .append_session_meta(&tail_record)
        .expect("append session meta");
    write_raw_lines(
        &tail_record.rollout_path,
        &[r#"{"v":2,"kind":"item","timestamp":"2026"#.to_string()],
    );
    let recovered = rollout_store
        .load_session_from_rollout(&tail_record.rollout_path, &deps)
        .await
        .expect("crash tail is tolerated");
    assert_eq!(recovered.summary.session_id(), tail_record.id);
}

/// Trace: L2-DES-RLM-001
/// Verifies that rollouts without a format version refuse resume.
#[tokio::test]
async fn unversioned_rollout_refuses_resume() {
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let deps = test_deps(dir.path());
    let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
    let record = sample_record(
        &rollout_store,
        dir.path().to_path_buf(),
        Some("Legacy session"),
    );
    let metadata = test_turn_record(record.id, TurnId::new());
    let legacy_lines = [
        LegacyRolloutLine::SessionMeta(Box::new(SessionMetaLine {
            timestamp: Utc::now(),
            session: record.clone(),
        })),
        LegacyRolloutLine::Turn(Box::new(TurnLine {
            timestamp: Utc::now(),
            turn: metadata,
        })),
    ];
    write_raw_lines(
        &record.rollout_path,
        &legacy_lines
            .iter()
            .map(|line| serde_json::to_string(line).expect("serialize legacy line"))
            .collect::<Vec<_>>(),
    );

    let err = parse_rollout_line(&raw_rollout_lines(&record.rollout_path)[0])
        .expect_err("legacy must fail");
    assert_eq!(err, RolloutLineReadError::FormatVersionMissing);

    match rollout_store
        .load_session_from_rollout(&record.rollout_path, &deps)
        .await
    {
        Ok(_) => panic!("pre-RLM resume must fail"),
        Err(resume_err) => {
            let message = resume_err.to_string();
            assert!(
                message.contains("damaged")
                    || message.contains("pre-RLM")
                    || message.contains("legacy"),
                "unexpected error: {resume_err:#}"
            );
        }
    }
}

fn append_basic_session_lines(
    rollout_store: &super::RolloutStore,
    data_root: &std::path::Path,
) -> devo_core::SessionRecord {
    let record = sample_record(rollout_store, data_root.to_path_buf(), None);
    rollout_store
        .append_session_meta(&record)
        .expect("append session meta");
    let turn = test_turn_record(record.id, TurnId::new());
    rollout_store
        .append_turn(&record, turn.clone())
        .expect("append turn");
    let item = super::build_item_record(
        record.id,
        turn.id,
        ItemId::new(),
        1,
        TurnItem::AgentMessage(TextItem::text("hi")),
        Some(TurnStatus::Running),
        None,
        None,
    );
    rollout_store
        .append_item(&record, item)
        .expect("append item");
    record
}

#[test]
fn append_projects_events_into_event_log() {
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let db = std::sync::Arc::new(
        crate::db::Database::open(dir.path().join("devo.db")).expect("open db"),
    );
    let rollout_store =
        super::RolloutStore::new(dir.path().to_path_buf(), Some(std::sync::Arc::clone(&db)));
    let record = append_basic_session_lines(&rollout_store, dir.path());

    // session/created lands on both the session stream and the per-cwd
    // sessions stream; turn and item facts land on the session stream.
    assert_eq!(db.event_log_len().expect("count"), 4);
    let session_stream = devo_core::session_stream_id(
        &devo_protocol::native::ids::SessionId::from_string(record.id.to_string()),
    );
    let rows = db
        .event_log_rows(&session_stream, 0)
        .expect("session stream");
    let kinds: Vec<&str> = rows.iter().map(|row| row.event_kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["session/created", "turn/completed", "item/completed"]
    );
    let seqs: Vec<u64> = rows.iter().map(|row| row.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3]);
    // Three physical rows written; watermark is the last line index.
    assert_eq!(
        db.projection_watermark(&record.rollout_path)
            .expect("watermark"),
        Some(2)
    );

    // The stored envelope payload parses as a typed EventEnvelope whose
    // meta.seq is hydrated from the row at replay time.
    let envelope: devo_protocol::native::event::EventEnvelope =
        serde_json::from_str(&rows[2].payload).expect("envelope payload parses");
    assert_eq!(envelope.meta.seq, None);
    assert!(envelope.meta.persisted);
}

#[test]
fn event_log_insert_is_idempotent_by_source_fact() {
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let db = std::sync::Arc::new(
        crate::db::Database::open(dir.path().join("devo.db")).expect("open db"),
    );
    let rollout_store =
        super::RolloutStore::new(dir.path().to_path_buf(), Some(std::sync::Arc::clone(&db)));
    let record = append_basic_session_lines(&rollout_store, dir.path());
    assert_eq!(db.event_log_len().expect("count"), 4);

    // Re-deriving the same facts (simulated crash recovery) inserts nothing.
    let raw_lines = raw_rollout_lines(&record.rollout_path);
    let mut rows = Vec::new();
    for (index, raw) in raw_lines.iter().enumerate() {
        let line = parse_rollout_line(raw).expect("parse");
        rows.extend(
            super::event_log_rows_for_rollout_line(&record.rollout_path, index as u64, 0, &line)
                .expect("derive rows"),
        );
    }
    assert_eq!(rows.len(), 4);
    let inserted = db.insert_event_log_rows(&rows).expect("re-insert");
    assert_eq!(inserted, 0);
    assert_eq!(db.event_log_len().expect("count"), 4);
}

// ── Field-level session settings log (L2-DES-CONV-002 Phase 1) ──

fn settings_test_record() -> (tempfile::TempDir, super::RolloutStore, SessionRecord) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
    let record = sample_record(&rollout_store, dir.path().to_path_buf(), None);
    (dir, rollout_store, record)
}

fn settings_line(
    record: &SessionRecord,
    field: devo_core::SessionSettingsField,
    value: serde_json::Value,
) -> LegacyRolloutLine {
    LegacyRolloutLine::SessionSettings(devo_core::SessionSettingsLine {
        timestamp: Utc::now(),
        session_id: record.id,
        field,
        value,
        epoch: 0,
    })
}

/// Trace: L2-DES-CONV-002
/// Verifies: a field-level settings line wins over the whole-record
/// SessionMeta value during replay (DD-4).
#[test]
fn session_settings_field_line_wins_over_session_meta_preset() {
    let (_dir, _store, mut record) = settings_test_record();
    record.permission_preset = Some(devo_protocol::PermissionPreset::Default);
    let mut replay = ReplayState::default();
    replay
        .apply_line(LegacyRolloutLine::SessionMeta(Box::new(SessionMetaLine {
            timestamp: Utc::now(),
            session: record.clone(),
        })))
        .expect("apply session meta");
    replay
        .apply_line(settings_line(
            &record,
            devo_core::SessionSettingsField::PermissionPreset,
            serde_json::to_value(devo_protocol::PermissionPreset::FullAccess)
                .expect("serialize preset"),
        ))
        .expect("apply settings line");

    let mut replayed = replay.session.take().expect("session record");
    replay.apply_session_settings(&mut replayed);
    assert_eq!(
        replayed.extras.permission_preset,
        Some(devo_protocol::PermissionPreset::FullAccess)
    );
}

/// Trace: L2-DES-CONV-002
/// Verifies: a PermissionPreset line clears the explicit SandboxProfile
/// override accumulated so far (approved patch-interaction rule).
#[test]
fn session_settings_preset_line_clears_explicit_sandbox_override() {
    let (_dir, _store, record) = settings_test_record();
    let mut replay = ReplayState::default();
    replay
        .apply_line(settings_line(
            &record,
            devo_core::SessionSettingsField::SandboxProfile,
            serde_json::Value::String("strict".into()),
        ))
        .expect("apply sandbox line");
    replay
        .apply_line(settings_line(
            &record,
            devo_core::SessionSettingsField::PermissionPreset,
            serde_json::to_value(devo_protocol::PermissionPreset::Default)
                .expect("serialize preset"),
        ))
        .expect("apply preset line");

    assert_eq!(replay.sandbox_profile_override(), None);
}

/// Trace: L2-DES-CONV-002
/// Verifies: an explicit SandboxProfile line written after the preset line
/// survives replay as the effective override.
#[test]
fn session_settings_explicit_sandbox_survives_when_written_after_preset() {
    let (_dir, _store, record) = settings_test_record();
    let mut replay = ReplayState::default();
    replay
        .apply_line(settings_line(
            &record,
            devo_core::SessionSettingsField::PermissionPreset,
            serde_json::to_value(devo_protocol::PermissionPreset::Default)
                .expect("serialize preset"),
        ))
        .expect("apply preset line");
    replay
        .apply_line(settings_line(
            &record,
            devo_core::SessionSettingsField::SandboxProfile,
            serde_json::Value::String("strict".into()),
        ))
        .expect("apply sandbox line");

    assert_eq!(
        replay.sandbox_profile_override(),
        Some("strict".to_string())
    );
}

/// Trace: L2-DES-CONV-002
/// Verifies: model-family field lines override the corresponding record
/// fields during replay.
#[test]
fn session_settings_model_fields_override_record() {
    let (_dir, _store, mut record) = settings_test_record();
    record.model = Some("old-model".into());
    let mut replay = ReplayState::default();
    replay
        .apply_line(LegacyRolloutLine::SessionMeta(Box::new(SessionMetaLine {
            timestamp: Utc::now(),
            session: record.clone(),
        })))
        .expect("apply session meta");
    replay
        .apply_line(settings_line(
            &record,
            devo_core::SessionSettingsField::Model,
            serde_json::to_value(Some("new-model".to_string())).expect("serialize model"),
        ))
        .expect("apply model line");
    replay
        .apply_line(settings_line(
            &record,
            devo_core::SessionSettingsField::ReasoningEffortSelection,
            serde_json::to_value(Some("high".to_string())).expect("serialize effort"),
        ))
        .expect("apply effort line");

    let mut replayed = replay.session.take().expect("session record");
    replay.apply_session_settings(&mut replayed);
    assert_eq!(replayed.native.model.model, "new-model");
    assert_eq!(
        replayed.native.settings.reasoning_effort,
        Some("high".to_string())
    );
}

/// Trace: L2-DES-CONV-002
/// Verifies: a settings line written through RolloutStore lands on disk as
/// a v2 Internal SessionSettings record and compatibility-maps back to the
/// same retained replay line.
#[test]
fn session_settings_line_roundtrips_through_store_and_projectors() {
    let (_dir, store, record) = settings_test_record();
    store.append_session_meta(&record).expect("append meta");
    store
        .append_session_settings_batch_at(
            &record.rollout_path,
            record.id,
            1,
            &[(
                devo_core::SessionSettingsField::SandboxProfile,
                serde_json::Value::String("workspace".into()),
            )],
        )
        .expect("append settings line");

    let raw_lines = std::fs::read_to_string(&record.rollout_path)
        .expect("read rollout")
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(raw_lines.len(), 2);
    let line = parse_rollout_line(raw_lines.last().expect("settings raw line")).expect("parse");
    let devo_core::rollout::RolloutLine::Internal { entry, .. } = &line else {
        panic!("settings line must be a v2 Internal record");
    };
    assert_eq!(
        entry,
        &devo_core::InternalRecord::SessionSettings {
            schema_version: 1,
            field: devo_core::SessionSettingsField::SandboxProfile,
            value: serde_json::Value::String("workspace".into()),
            epoch: 1,
        }
    );

    let devo_core::rollout::RolloutLine::Internal {
        timestamp,
        session_id,
        turn_id,
        seq,
        entry,
        ..
    } = &line
    else {
        unreachable!();
    };
    let legacy_lines = devo_core::legacy_lines_from_internal(
        *timestamp,
        session_id,
        turn_id.as_ref(),
        *seq,
        entry,
    )
    .expect("compatibility mapping");
    let [LegacyRolloutLine::SessionSettings(line)] = legacy_lines.as_slice() else {
        panic!("mapping must yield exactly one SessionSettings legacy line");
    };
    assert_eq!(line.session_id, record.id);
    assert_eq!(line.field, devo_core::SessionSettingsField::SandboxProfile);
    assert_eq!(line.value, serde_json::Value::String("workspace".into()));
}
