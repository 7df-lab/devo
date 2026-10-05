use devo_core::TurnItem;
use devo_core::native_item_from_turn_item;
use devo_protocol::CollaborationMode;
use devo_protocol::SessionHistoryEntry;

/// Projects one packed legacy turn item into a history entry when visible.
///
/// `TurnItem::TurnSummary` remains migrate-only and still becomes
/// [`SessionHistoryEntry::TurnSummary`] for ACP. Live finalize does not write
/// that variant — Native `Turn` status/timing is the summary source.
pub(crate) fn history_entry_from_turn_item(item: &TurnItem) -> Option<SessionHistoryEntry> {
    match item {
        TurnItem::TurnSummary(text) => {
            let (title, duration_ms) = match text.text.split_once(':') {
                Some((model, dur)) => (model.to_string(), dur.parse::<u64>().ok()),
                None => (text.text.clone(), None),
            };
            Some(SessionHistoryEntry::TurnSummary {
                title,
                body: String::new(),
                duration_ms,
                collaboration_mode: CollaborationMode::default(),
            })
        }
        other => native_item_from_turn_item(other).map(SessionHistoryEntry::item),
    }
}
