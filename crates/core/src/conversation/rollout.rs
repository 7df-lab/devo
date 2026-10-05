//! The versioned whole-line rollout envelope (v2) and the reader dispatch.
//!
//! Truth source: `devo-api-design/05-migration.md` §2.2, superseded for the
//! RLM breaking release by `L2-DES-RLM-001` DD-9: **legacy (v1) lines are
//! rejected**. The write path only ever appends v2. Reading goes through
//! [`parse_rollout_line`], which requires top-level `v: 2` and refuses
//! unknown versions and pre-RLM (missing `v`) sessions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use devo_protocol::native::ids::{ItemId, SessionId, TurnId};
use devo_protocol::native::item::{InternalEntry, ItemEnvelope};
use devo_protocol::native::session::Session;
use devo_protocol::native::turn::Turn;
use devo_protocol::native::usage::{TurnUsage, UsageRecord};

use crate::{
    MessageEditRecordedRecord, SessionContext, TurnContext, TurnSupersededRecord,
    TurnWorkspaceChangeRecordedRecord, TurnWorkspaceCheckpointRecordedRecord,
    TurnWorkspaceRestoreCompletedRecord, TurnWorkspaceRestoreStartedRecord,
};

/// The format version written by the v2 write path.
pub const ROLLOUT_FORMAT_VERSION: u32 = 2;

/// Persistence-only extras on the v2 SessionMeta line: internal
/// implementation details that canonical `Session` deliberately omits from
/// the public surface but legacy replay needs at resume time. Never enters
/// the public schema — `RolloutLine` is a core type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionPersistenceExtras {
    /// The locked session context captured for prompt replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_context: Option<SessionContext>,
    /// The CLI version that created the session (audit field).
    pub cli_version: String,
    /// The session source kind, such as `cli` or `api` (audit field).
    pub source: String,
    /// Session-level collaboration mode override (Build/Plan), when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collaboration_mode: Option<crate::CollaborationMode>,
    /// Session-level permission preset override, when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_preset: Option<crate::PermissionPreset>,
    /// Relative path to the kernel dill snapshot under the session dir (RLM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_snapshot_path: Option<String>,
}

/// Persistence-only extras on the v2 Turn line, same rationale as
/// [`SessionPersistenceExtras`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnPersistenceExtras {
    /// The locked session context used to build the stable request prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_context: Option<SessionContext>,
    /// The turn context snapshot used for this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_context: Option<TurnContext>,
    /// The concrete request thinking parameter used to execute the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_thinking: Option<String>,
    /// The estimated input-token count at turn start, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_token_estimate: Option<u32>,
    /// Provider usage of the latest model query (excludes tool/retry calls).
    /// Native end-to-end — same shape as `Turn.usage` / live runtime meters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_query_usage: Option<TurnUsage>,
    /// Context-window occupancy after the latest model query in this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
    /// The terminal provider/model stop reason, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<crate::StopReason>,
    /// The typed terminal failure reason, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<crate::TurnFailureReason>,
}

/// The v2 whole-line rollout envelope. Every line kind carries the format
/// version, a wall-clock timestamp, and its payload in a stable flat shape,
/// e.g. `{"v":2,"kind":"item","timestamp":"...","item":{...}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RolloutLine {
    /// Canonical session metadata. Session and extras are boxed to keep the
    /// enum small (serde-transparent).
    SessionMeta {
        v: u32,
        timestamp: DateTime<Utc>,
        session: Box<Session>,
        /// Replay-only fields the canonical session does not model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extras: Option<Box<SessionPersistenceExtras>>,
    },
    /// Canonical turn metadata.
    Turn {
        v: u32,
        timestamp: DateTime<Utc>,
        turn: Turn,
        /// Replay-only fields the canonical turn does not model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extras: Option<Box<TurnPersistenceExtras>>,
    },
    /// One typed item envelope (`{"kind":"item","item":{...}}`).
    Item {
        v: u32,
        timestamp: DateTime<Utc>,
        item: ItemEnvelope,
    },
    /// Rollout-only records that are not public items; see
    /// [`InternalRecord`]. Identity and position travel on the line so
    /// internal entries are exactly recoverable: `Entry` records are
    /// turn-scoped and consume one sequence position (shared with the item
    /// stream); the other records are session-scoped markers that carry
    /// their own identity inside the payload, so `turn_id` is `None` and
    /// `seq` is 0 for them.
    Internal {
        v: u32,
        timestamp: DateTime<Utc>,
        session_id: SessionId,
        turn_id: Option<TurnId>,
        seq: u64,
        entry: InternalRecord,
    },
    /// Session title change. The legacy `title_state` is dropped: title
    /// lifecycle is a derived cache in the new model.
    SessionTitleUpdated {
        v: u32,
        timestamp: DateTime<Utc>,
        session_id: SessionId,
        title: String,
        previous_title: Option<String>,
    },
    /// A compaction snapshot reference: which item summarizes the compacted
    /// history and which pre-existing items survive, in prompt order.
    CompactionSnapshot {
        v: u32,
        timestamp: DateTime<Utc>,
        session_id: SessionId,
        turn_id: TurnId,
        summary_item_id: ItemId,
        preserved_item_ids: Vec<ItemId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
    },
    /// An append-only rollback marker: the retained turns/items after the
    /// in-memory history was rebuilt.
    SessionRollback {
        v: u32,
        timestamp: DateTime<Utc>,
        session_id: SessionId,
        retained_turn_ids: Vec<TurnId>,
        retained_item_ids: Vec<ItemId>,
        latest_turn_id: Option<TurnId>,
    },
    /// A workspace checkpoint captured before a turn; payload unchanged from
    /// the legacy record.
    WorkspaceCheckpoint {
        v: u32,
        timestamp: DateTime<Utc>,
        record: TurnWorkspaceCheckpointRecordedRecord,
    },
    /// One recorded workspace change; payload unchanged from the legacy
    /// record.
    WorkspaceChange {
        v: u32,
        timestamp: DateTime<Utc>,
        record: TurnWorkspaceChangeRecordedRecord,
    },
    /// A workspace restore started; payload unchanged from the legacy record.
    WorkspaceRestoreStarted {
        v: u32,
        timestamp: DateTime<Utc>,
        record: TurnWorkspaceRestoreStartedRecord,
    },
    /// A workspace restore completed; payload unchanged from the legacy
    /// record.
    WorkspaceRestoreCompleted {
        v: u32,
        timestamp: DateTime<Utc>,
        record: TurnWorkspaceRestoreCompletedRecord,
    },
}

/// Rollout-only records that are not public items. They never appear in
/// `item/*` events or the public schema; the rollout reader hands them
/// straight to the recovery pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum InternalRecord {
    /// Acknowledged tool execution and prompt checkpoint, never a public item.
    Execution {
        record: crate::durable_execution::ExecutionRecord,
    },
    /// A canonical internal replay entry. Kept as a nested payload rather than
    /// a flattened newtype variant: both enums use the `type` tag, so
    /// flattening would emit a duplicate `type` key and break round-trips.
    Entry { entry: InternalEntry },
    /// The locked session context captured for replay, payload unchanged from
    /// the legacy record. Boxed to keep the enum small (serde-transparent).
    SessionContext(Box<SessionContext>),
    /// An accepted message edit, payload unchanged from the legacy record.
    MessageEdit(MessageEditRecordedRecord),
    /// A superseded-turn marker, payload unchanged from the legacy record.
    TurnSuperseded(TurnSupersededRecord),
    /// Latest server-owned Goal snapshot for the session. `None` means the
    /// current Goal was cleared. Core replay deliberately treats the payload
    /// as opaque because Goal orchestration is server-owned.
    GoalState {
        schema_version: u32,
        goal: Option<serde_json::Value>,
    },
    /// RLM kernel OS-fence outcome at spawn (design doc `rlm-permissions.md`
    /// §5.3): `fenced`, `downgradedUnfenced` (loud downgrade — audit trail for
    /// the unfenced kernel), or `notRequested`. Replay treats it as opaque.
    KernelFence { schema_version: u32, state: String },
    /// One append-only model-call accounting entry. Unlike turn summary usage,
    /// this preserves failed attempts and non-turn overhead calls.
    UsageRecord { record: UsageRecord },
    /// One field-level session settings change (L2-DES-CONV-002 DD-4). The
    /// last line per field wins during replay; line position provides the
    /// total order, and `epoch` annotates the settings-write sequence for
    /// cross-path writes and traces.
    SessionSettings {
        /// The schema version for persisted settings lines.
        schema_version: u32,
        /// The settings field that changed.
        field: crate::conversation::records::SessionSettingsField,
        /// The new value in the field's natural JSON shape.
        value: serde_json::Value,
        /// Per-session settings-write sequence, assigned by the per-file
        /// projector at write time.
        epoch: u64,
    },
    /// Turn blocked on interactive approval; used to resume after restart.
    TurnApprovalCheckpoint(Box<crate::TurnApprovalCheckpointRecordedRecord>),
    /// Current transcript-tree tip (last-wins). `leaf_id: None` means empty tip.
    SessionLeaf {
        /// Settings-style write sequence for cross-path ordering/traces.
        epoch: u64,
        /// Tip item id, or `None` after reset to root.
        leaf_id: Option<devo_protocol::native::ids::ItemId>,
    },
    /// Durable parent pointer for one item in the transcript tree. Last edge
    /// for a given `child_id` wins; item envelopes may also carry `parent_id`.
    TreeEdge {
        child_id: devo_protocol::native::ids::ItemId,
        parent_id: Option<devo_protocol::native::ids::ItemId>,
    },
}

/// Errors from reading a single rollout line. Unknown format versions must
/// never be silently skipped (05 §2.2); a parse failure on any non-final line
/// marks the session damaged and stops automatic writes. Only a truncated
/// final line is a tolerable crash tail. Pre-RLM (v1 / missing `v`) sessions
/// cannot be resumed (`L2-DES-RLM-001` DD-9).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RolloutLineReadError {
    /// The line declares a format version this reader does not understand.
    #[error("unsupported rollout format version {version}")]
    RolloutVersionUnsupported { version: u32 },
    /// The line has no top-level format version.
    #[error("rollout format version is missing")]
    FormatVersionMissing,
    /// The line is not valid JSON for its declared version.
    #[error("damaged rollout line: {reason}")]
    Damaged { reason: String },
    /// The line is cut off mid-JSON. Only tolerable as the final line of the
    /// file (crash tail); mid-file it must be treated as
    /// [`Self::Damaged`].
    #[error("truncated rollout line (crash tail; only the final line may be ignored)")]
    TruncatedTail,
}

/// Parses one rollout JSONL line. Requires top-level `v: 2`. A missing `v`
/// returns [`RolloutLineReadError::FormatVersionMissing`]. Other versions →
/// [`RolloutLineReadError::RolloutVersionUnsupported`].
///
/// A truncated line reports [`RolloutLineReadError::TruncatedTail`]; the
/// caller decides whether it is the file's final line (tolerable crash tail)
/// or mid-file damage.
pub fn parse_rollout_line(line: &str) -> Result<RolloutLine, RolloutLineReadError> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|error| {
        if error.is_eof() {
            RolloutLineReadError::TruncatedTail
        } else {
            RolloutLineReadError::Damaged {
                reason: error.to_string(),
            }
        }
    })?;
    let Some(version) = value.get("v") else {
        return Err(RolloutLineReadError::FormatVersionMissing);
    };
    let version = version
        .as_u64()
        .ok_or_else(|| RolloutLineReadError::Damaged {
            reason: format!("rollout version is not an unsigned integer: {version}"),
        })?;
    match version {
        2 => {
            let line = serde_json::from_value::<RolloutLine>(value).map_err(|error| {
                RolloutLineReadError::Damaged {
                    reason: error.to_string(),
                }
            })?;
            Ok(line)
        }
        other => Err(RolloutLineReadError::RolloutVersionUnsupported {
            version: u32::try_from(other).unwrap_or(u32::MAX),
        }),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;

    use super::*;
    use devo_protocol::native::ids::ItemId as CanonicalItemId;
    use devo_protocol::native::item::{Item, ItemState, UserInput, UserMessageEntry};

    fn fixed_ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap()
    }

    fn sample_item_line() -> RolloutLine {
        RolloutLine::Item {
            v: ROLLOUT_FORMAT_VERSION,
            timestamp: fixed_ts(),
            item: ItemEnvelope {
                id: CanonicalItemId::from_string("item_1".into()),
                session_id: SessionId::from_string("ses_1".into()),
                turn_id: TurnId::from_string("turn_1".into()),
                seq: 1,
                revision: 1,
                created_at: fixed_ts(),
                updated_at: fixed_ts(),
                state: ItemState::Completed,
                item: Item::UserMessage {
                    client_user_message_id: None,
                    content: vec![UserInput::Text {
                        text: "hello".into(),
                    }],
                    entry: UserMessageEntry::TurnStart,
                },
                parent_id: None,
            },
        }
    }

    #[test]
    fn item_line_serializes_with_flat_versioned_envelope() {
        let json = serde_json::to_value(sample_item_line()).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "v": 2,
                "kind": "item",
                "timestamp": "2026-08-01T00:00:00Z",
                "item": {
                    "id": "item_1",
                    "sessionId": "ses_1",
                    "turnId": "turn_1",
                    "seq": 1,
                    "revision": 1,
                    "createdAt": "2026-08-01T00:00:00Z",
                    "updatedAt": "2026-08-01T00:00:00Z",
                    "state": "completed",
                    "item": {
                        "type": "userMessage",
                        "content": [{"type": "text", "text": "hello"}],
                        "entry": "turnStart"
                    }
                }
            })
        );
    }

    #[test]
    fn dispatch_parses_current_format_line() {
        let line = serde_json::to_string(&sample_item_line()).expect("serialize");
        let parsed = parse_rollout_line(&line).expect("parse");
        assert_eq!(parsed, sample_item_line());
    }

    #[test]
    fn dispatch_rejects_missing_format_version() {
        let line = r#"{"timestamp":"2026-08-01T00:00:00Z","type":"session_title_updated","session_id":"00000000-0000-0000-0000-000000000001","title":"x","title_state":"generating"}"#;
        let error = parse_rollout_line(line).expect_err("must fail");
        assert_eq!(error, RolloutLineReadError::FormatVersionMissing);
    }

    #[test]
    fn dispatch_rejects_unknown_version_for_valid_current_shape() {
        let mut value = serde_json::to_value(sample_item_line()).expect("serialize");
        value["v"] = serde_json::json!(3);
        let error = parse_rollout_line(&value.to_string()).expect_err("must fail");
        assert_eq!(
            error,
            RolloutLineReadError::RolloutVersionUnsupported { version: 3 }
        );
    }

    #[test]
    fn dispatch_rejects_unknown_version_before_payload_validation() {
        let error = parse_rollout_line(r#"{"v":3,"kind":"nonsense"}"#).expect_err("must fail");
        assert_eq!(
            error,
            RolloutLineReadError::RolloutVersionUnsupported { version: 3 }
        );
    }

    #[test]
    fn dispatch_rejects_non_integer_version_as_damaged() {
        let error = parse_rollout_line(r#"{"v":"2","kind":"item"}"#).expect_err("must fail");
        assert!(matches!(error, RolloutLineReadError::Damaged { .. }));
    }

    #[test]
    fn dispatch_flags_truncated_line_as_crash_tail() {
        let line = serde_json::to_string(&sample_item_line()).expect("serialize");
        let truncated = &line[..line.len() / 2];
        let error = parse_rollout_line(truncated).expect_err("must fail");
        assert_eq!(error, RolloutLineReadError::TruncatedTail);
    }

    #[test]
    fn dispatch_flags_well_formed_json_with_wrong_shape_as_damaged() {
        let error = parse_rollout_line(r#"{"v":2,"kind":"nonsense"}"#).expect_err("must fail");
        assert!(matches!(error, RolloutLineReadError::Damaged { .. }));
    }
}
