use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use crate::SessionId;
use crate::turn::CollaborationMode;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionStartParams {
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_directories: Vec<PathBuf>,
    pub ephemeral: bool,
    pub title: Option<String>,
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionStartResult {
    pub session: crate::native::session::Session,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionResumeParams {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionResumeResult {
    pub session: crate::native::session::Session,
    pub latest_turn: Option<crate::native::turn::Turn>,
    pub loaded_item_count: u64,
    /// In-memory history for ACP resume projection. First-party Native clients
    /// should use `session/items/list` instead of this field.
    pub history_items: Vec<SessionHistoryEntry>,
    /// Pending turn input texts queued for the next turn.
    pub pending_texts: Vec<String>,
}

/// Server-owned history row.
///
/// **Live / first-party path:** only [`SessionHistoryEntry::Item`] (Native
/// [`crate::native::item::Item`], including terminal
/// [`crate::native::item::Item::Warning`] for turn failures). Native `Turn`
/// status/timing is the turn summary source — do not invent parallel summary
/// vocabulary on this enum for new writes.
///
/// **Leftover (migrate / ACP):** `TurnSummary` / `Error` remain so packed
/// legacy `TurnItem::TurnSummary` and older resume payloads can still project
/// into ACP thought/error updates. Do not emit them from live finalize.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(
    tag = "entryType",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
// Wire DTO: boxing the Item variant would break nested exhaustive matches on
// the entry enum across protocol consumers.
#[allow(clippy::large_enum_variant)]
pub enum SessionHistoryEntry {
    Item {
        item: crate::native::item::Item,
    },
    /// Migrate/ACP-only synthetic row (not written by live finalize).
    TurnSummary {
        title: String,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(default)]
        collaboration_mode: CollaborationMode,
    },
    /// Migrate/ACP-only synthetic row (live path uses `Item::Warning`).
    Error {
        title: String,
        body: String,
    },
}

impl SessionHistoryEntry {
    pub fn item(item: crate::native::item::Item) -> Self {
        Self::Item { item }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionForkParams {
    pub session_id: SessionId,
    pub title: Option<String>,
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_turn_index: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionForkResult {
    pub session: crate::native::session::Session,
    pub forked_from_session_id: SessionId,
}

// ── Session Subscribe (L3-BEH-PROTOCOL-001 B3) ───────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionSubscribeParams {
    pub session_id: SessionId,
    #[serde(default)]
    pub from_sequence: Option<u64>,
    #[serde(default)]
    pub event_filter: Option<Vec<String>>,
    #[serde(default)]
    pub projection: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionSubscribeResult {
    pub subscription_id: String,
    pub session_id: SessionId,
    pub next_sequence: u64,
    pub session_snapshot: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn session_history_entry_item_roundtrips() {
        let entry = SessionHistoryEntry::item(crate::native::item::Item::UserMessage {
            client_user_message_id: None,
            content: vec![crate::native::item::UserInput::Text {
                text: "hello".into(),
            }],
            entry: crate::native::item::UserMessageEntry::TurnStart,
        });
        let json = serde_json::to_string(&entry).expect("serialize history entry");
        let restored: SessionHistoryEntry =
            serde_json::from_str(&json).expect("deserialize history entry");
        assert_eq!(restored, entry);
    }
}
