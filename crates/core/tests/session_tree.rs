//! Transcript tree graph: sibling branches and leaf path filtering.

use chrono::Utc;
use devo_core::{CanonicalHistory, active_path_item_ids, build_session_tree};
use devo_protocol::native::ids::{ItemId, SessionId, TurnId};
use devo_protocol::native::item::{Item, ItemEnvelope, ItemState, UserInput, UserMessageEntry};
use pretty_assertions::assert_eq;

fn envelope(id: &str, seq: u64, item: Item) -> ItemEnvelope {
    ItemEnvelope {
        id: ItemId::from_string(id.to_string()),
        session_id: SessionId::from_string("ses_test".into()),
        turn_id: TurnId::from_string("turn_test".into()),
        seq,
        revision: 1,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        state: ItemState::Completed,
        item,
        parent_id: None,
    }
}

/// Verifies: sibling children under a shared parent and leaf tip selection.
#[test]
fn build_tree_keeps_sibling_branches() {
    let root = envelope(
        "item_a",
        1,
        Item::UserMessage {
            client_user_message_id: None,
            content: vec![UserInput::Text {
                text: "first".into(),
            }],
            entry: UserMessageEntry::TurnStart,
        },
    );
    let asst = envelope("item_b", 2, Item::AssistantMessage { text: "ok".into() });
    let path_a = envelope(
        "item_c",
        3,
        Item::UserMessage {
            client_user_message_id: None,
            content: vec![UserInput::Text {
                text: "path A".into(),
            }],
            entry: UserMessageEntry::TurnStart,
        },
    );
    let path_b = envelope(
        "item_d",
        4,
        Item::UserMessage {
            client_user_message_id: None,
            content: vec![UserInput::Text {
                text: "path B".into(),
            }],
            entry: UserMessageEntry::TurnStart,
        },
    );

    let mut history = CanonicalHistory {
        items: vec![root, asst, path_a, path_b],
        ..CanonicalHistory::default()
    };
    history
        .tree_edges
        .insert(ItemId::from_string("item_a".into()), None);
    history.tree_edges.insert(
        ItemId::from_string("item_b".into()),
        Some(ItemId::from_string("item_a".into())),
    );
    history.tree_edges.insert(
        ItemId::from_string("item_c".into()),
        Some(ItemId::from_string("item_b".into())),
    );
    history.tree_edges.insert(
        ItemId::from_string("item_d".into()),
        Some(ItemId::from_string("item_b".into())),
    );
    history.leaf_id = Some(ItemId::from_string("item_d".into()));

    let (tree, leaf) = build_session_tree(&history);
    assert_eq!(leaf.as_ref().map(|id| id.as_str()), Some("item_d"));
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].children.len(), 1);
    assert_eq!(tree[0].children[0].children.len(), 2);
    let path = active_path_item_ids(&history);
    assert!(path.contains(&ItemId::from_string("item_a".into())));
    assert!(path.contains(&ItemId::from_string("item_b".into())));
    assert!(path.contains(&ItemId::from_string("item_d".into())));
    assert!(!path.contains(&ItemId::from_string("item_c".into())));
}
