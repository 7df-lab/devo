use super::*;
use crate::persisted_native_item::PersistedNativeItem;
use devo_protocol::native::{ids::ItemId, item::Item, turn::TurnKind};
use pretty_assertions::assert_eq;

#[test]
fn terminal_snapshot_preserves_all_assistant_messages_from_only_its_turn() {
    let previous = TurnId::new();
    let current = TurnId::new();
    let items = [
        PersistedNativeItem::new(
            previous,
            TurnKind::Regular,
            ItemId::new(),
            Item::AssistantMessage {
                text: "old answer".into(),
            },
        ),
        PersistedNativeItem::new(
            current,
            TurnKind::Regular,
            ItemId::new(),
            Item::AssistantMessage {
                text: "Checking the result.\n".into(),
            },
        ),
        PersistedNativeItem::new(
            current,
            TurnKind::Regular,
            ItemId::new(),
            Item::AssistantMessage {
                text: "Final answer.".into(),
            },
        ),
    ];
    assert_eq!(
        subagent_terminal_status_detail_from_items(&items, &[], current, SubagentStatus::Completed),
        Some("Checking the result.\nFinal answer.".into())
    );
}

#[test]
fn terminal_snapshot_joins_checkpoint_and_resumed_inline_without_duplicate_items() {
    let turn = TurnId::new();
    let before = PersistedNativeItem::new(
        turn,
        TurnKind::Regular,
        ItemId::new(),
        Item::AssistantMessage {
            text: "Before approval.\n".into(),
        },
    );
    let after = PersistedNativeItem::new(
        turn,
        TurnKind::Regular,
        ItemId::new(),
        Item::AssistantMessage {
            text: "After approval.".into(),
        },
    );
    let stable = [before.clone()];
    let inline = [before, after];
    assert_eq!(
        subagent_terminal_status_detail_from_items(
            &stable,
            &inline,
            turn,
            SubagentStatus::Completed
        ),
        Some("Before approval.\nAfter approval.".into())
    );
}
