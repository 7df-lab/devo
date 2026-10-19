//! In-memory durable journal row: Native [`Item`] plus turn metadata.
//!
//! First-party persist / fork / history / prompt rebuild operate on this type
//! directly. Legacy [`TurnItem`] conversion is migrate/resume-only.

#[cfg(test)]
use std::path::PathBuf;

use devo_protocol::SessionHistoryEntry;
use devo_protocol::native::ids::{ItemId, TurnId};
use devo_protocol::native::item::Item;
#[cfg(test)]
use devo_protocol::native::item::UserInput;
use devo_protocol::native::item::UserMessageEntry;
use devo_protocol::native::turn::TurnKind;

/// Durable in-memory item owned by the session actor / TurnWorkingSet.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PersistedNativeItem {
    pub(crate) turn_id: TurnId,
    pub(crate) turn_kind: TurnKind,
    pub(crate) item_id: ItemId,
    pub(crate) item: Item,
}

impl PersistedNativeItem {
    pub(crate) fn new(turn_id: TurnId, turn_kind: TurnKind, item_id: ItemId, item: Item) -> Self {
        Self {
            turn_id,
            turn_kind,
            item_id,
            item,
        }
    }

    pub(crate) fn legacy_turn_id(&self) -> Option<devo_core::TurnId> {
        Some(self.turn_id)
    }

    pub(crate) fn legacy_item_id(&self) -> Option<devo_core::ItemId> {
        Some(self.item_id)
    }
}

/// Compatibility alias while call sites migrate off the TurnItem-backed name.
pub(crate) type PersistedTurnItem = PersistedNativeItem;

pub(crate) fn is_user_message(item: &Item) -> bool {
    matches!(
        item,
        Item::UserMessage {
            entry: UserMessageEntry::TurnStart | UserMessageEntry::Queue,
            ..
        }
    )
}

pub(crate) fn tool_call_id(item: &Item) -> Option<&str> {
    match item {
        Item::ToolCall { call_id, .. }
        | Item::ToolResult { call_id, .. }
        | Item::CommandExecution { call_id, .. }
        | Item::HostedToolCall { call_id, .. } => Some(call_id.as_str()),
        _ => None,
    }
}

/// Items that rebuild model prompt messages after compaction / resume.
pub(crate) fn prompt_visible_native_item(item: &Item) -> bool {
    match item {
        Item::UserMessage { .. }
        | Item::AssistantMessage { .. }
        | Item::Reasoning { .. }
        | Item::Plan { .. }
        | Item::ToolCall { .. }
        | Item::ToolResult { .. }
        | Item::CommandExecution { .. }
        | Item::HostedToolCall { .. }
        | Item::ContextCompaction { .. } => true,
        Item::Approval { .. }
        | Item::FileChange { .. }
        | Item::UserInputRequest { .. }
        | Item::SubAgent { .. }
        | Item::BackgroundTask { .. }
        | Item::GoalProgress { .. }
        | Item::Refinement { .. }
        | Item::Warning { .. }
        | Item::BranchSummary { .. } => false,
    }
}

pub(crate) fn prompt_visible_persisted_item(item: &PersistedNativeItem) -> bool {
    prompt_visible_native_item(&item.item)
}

/// History row for a persisted Native item.
///
/// Terminal turn failures use [`Item::Warning`] on the live path. Migrate-only
/// [`SessionHistoryEntry::TurnSummary`] / [`SessionHistoryEntry::Error`] are
/// not produced here.
pub(crate) fn history_entry_from_native_item(item: &Item) -> Option<SessionHistoryEntry> {
    match item {
        Item::AssistantMessage { text, .. } if text.trim().is_empty() => None,
        _ => Some(SessionHistoryEntry::item(item.clone())),
    }
}

/// Live/replay history row for a terminal turn failure (Native-shaped).
pub(crate) fn turn_failure_history_entry(
    code: impl Into<String>,
    message: impl Into<String>,
) -> SessionHistoryEntry {
    SessionHistoryEntry::item(Item::Warning {
        code: code.into(),
        message: message.into(),
        retryable: false,
    })
}

/// Best-effort ContextCompaction Native item from summary text (live compaction).
pub(crate) fn context_compaction_item(
    summary: impl Into<String>,
    trigger: devo_protocol::native::item::CompactionTrigger,
) -> Item {
    use devo_protocol::native::item::ContextUsage;
    Item::ContextCompaction {
        trigger,
        before: ContextUsage {
            measured: false,
            ..ContextUsage::default()
        },
        after: None,
        summary: Some(summary.into()),
    }
}

/// Map a compaction-time occupancy snapshot onto the item's usage fields.
///
/// Occupancy carries totals only, so the granular token buckets stay zero;
/// `measured` marks whether a snapshot existed at all.
pub(crate) fn context_usage_from_occupancy(
    occupancy: Option<&devo_protocol::native::item::ContextOccupancy>,
) -> devo_protocol::native::item::ContextUsage {
    use devo_protocol::native::item::ContextUsage;
    match occupancy {
        Some(occupancy) => ContextUsage {
            total_tokens: occupancy.total_tokens,
            context_window_tokens: Some(occupancy.context_window_tokens),
            measured: true,
            ..ContextUsage::default()
        },
        None => ContextUsage {
            measured: false,
            ..ContextUsage::default()
        },
    }
}

#[cfg(test)]
pub(crate) fn user_message_item(
    text: impl Into<String>,
    local_image_paths: &[PathBuf],
    entry: UserMessageEntry,
) -> Item {
    let text = text.into();
    let mut content = Vec::with_capacity(1 + local_image_paths.len());
    content.push(UserInput::Text { text });
    for path in local_image_paths {
        content.push(UserInput::LocalImage {
            path: path.clone(),
            detail: None,
        });
    }
    Item::UserMessage {
        client_user_message_id: None,
        content,
        entry,
    }
}
