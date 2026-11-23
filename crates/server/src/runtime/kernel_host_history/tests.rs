use super::*;
use devo_protocol::native::ids::{ItemId, TurnId};
use devo_protocol::native::item::{ItemState, ToolSource};
use devo_protocol::native::wire_projector::typed_item_envelope;
use pretty_assertions::assert_eq;

#[test]
fn read_and_search_share_the_session_branch_and_unicode_offsets() {
    let session = SessionId::new();
    let turn = TurnId::new();
    let row = |seq: u64, item: Item, parent: Option<ItemId>| {
        let mut envelope = typed_item_envelope(
            session,
            turn,
            ItemId::new(),
            seq,
            &item,
            ItemState::Completed,
            chrono::Utc::now(),
            /*created_at*/ None,
        );
        envelope.parent_id = parent;
        envelope
    };
    let call = row(
        1,
        Item::ToolCall {
            call_id: "c1".into(),
            tool_name: "python".into(),
            source: ToolSource::Builtin,
            server_name: None,
            input: Some(json!({"code":"compute()"})),
        },
        None,
    );
    let archived = row(
        2,
        Item::ToolResult {
            call_id: "c1".into(),
            output: json!("discarded"),
            display_content: None,
            truncated: false,
            is_error: false,
        },
        Some(call.id),
    );
    let kept = row(
        3,
        Item::ToolResult {
            call_id: "c1".into(),
            output: json!("字🙂needle尾"),
            display_content: Some("short UI text".into()),
            truncated: true,
            is_error: false,
        },
        Some(call.id),
    );
    let mut foreign = row(
        4,
        Item::AssistantMessage {
            text: "foreign".into(),
        },
        Some(kept.id),
    );
    foreign.session_id = SessionId::new();
    let history = devo_core::CanonicalHistory {
        items: vec![call.clone(), archived.clone(), kept.clone(), foreign],
        leaf_id: Some(kept.id),
        leaf_epoch: 1,
        ..Default::default()
    };
    let rows = recallable_rows(&history, session);
    let found = search(
        &rows,
        serde_json::from_value(json!({"query":"needle","kind":"tool_result"})).unwrap(),
    )
    .unwrap();
    let mut hit = metadata(&kept);
    hit["offset"] = json!(2);
    hit["preview"] = json!("字🙂needle尾");
    assert_eq!(found, json!({"matches":[hit],"next_before":null}));
    let record = read(
        &rows,
        serde_json::from_value(json!({"item_id":"call:c1","offset":2,"max_chars":6,"around":1}))
            .unwrap(),
    )
    .unwrap();
    let mut expected = metadata(&kept);
    expected["text"] = json!("needle");
    expected["offset"] = json!(2);
    expected["next_offset"] = json!(8);
    let call_preview = preview(&call, &body(&call.item), 0);
    expected["tool_call"] = call_preview.clone();
    expected["neighbors"] = json!([call_preview]);
    assert_eq!(record, expected);
    // A reused provider ID must never resolve to an arbitrary result.
    let mut repeated = kept.clone();
    repeated.id = ItemId::new();
    for next_turn in [turn, TurnId::new()] {
        repeated.turn_id = next_turn;
        let collisions = [&call, &kept, &repeated];
        let alias = serde_json::from_value(json!({"item_id":"call:c1"})).unwrap();
        assert!(
            read(&collisions, alias)
                .unwrap_err()
                .contains("ambiguous call id")
        );
        let by_id = serde_json::from_value(json!({"item_id":repeated.id})).unwrap();
        assert_eq!(
            read(&collisions, by_id).unwrap()["item_id"],
            json!(repeated.id)
        );
    }
    let read_id = |id: ItemId| serde_json::from_value(json!({"item_id":id})).unwrap();
    assert!(read(&rows, read_id(archived.id)).is_err());
    let other_session = recallable_rows(&history, SessionId::new());
    assert!(other_session.is_empty());
    let root = devo_core::CanonicalHistory {
        leaf_id: None,
        ..history
    };
    assert!(recallable_rows(&root, session).is_empty());
    assert!(serde_json::from_value::<Search>(json!({"session_id":"other"})).is_err());
    assert!(
        serde_json::from_value::<Read>(json!({"item_id":kept.id,"path":"other.jsonl"})).is_err()
    );
}

#[test]
fn oldest_of_ten_thousand_results_is_searchable_and_pages_are_bounded() {
    let session = SessionId::new();
    let turn = TurnId::new();
    let history = devo_core::CanonicalHistory {
        items: (0..10_000)
            .map(|seq| {
                typed_item_envelope(
                    session,
                    turn,
                    ItemId::new(),
                    seq,
                    &Item::AssistantMessage {
                        text: format!("observation #{seq}."),
                    },
                    ItemState::Completed,
                    chrono::Utc::now(),
                    /*created_at*/ None,
                )
            })
            .collect(),
        ..Default::default()
    };
    let rows = recallable_rows(&history, session);
    let page = |params| search(&rows, serde_json::from_value(params).unwrap()).unwrap();
    let oldest = page(json!({"query":"#0."}));
    assert_eq!(oldest["matches"][0]["item_id"], json!(history.items[0].id));
    let newest = page(json!({"limit":2}));
    assert_eq!(newest["next_before"], json!(history.items[9998].id));
    let older = page(json!({"limit":1,"before":newest["next_before"]}));
    assert_eq!(
        older["matches"][0]["item_id"],
        json!(history.items[9997].id)
    );
}
