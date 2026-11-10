use chrono::Utc;
use devo_protocol::native::event::ServerNotification;
use devo_protocol::native::host_message::{
    AUTHORITY_NOTICE, HostMessageDelivery, HostMessageOrigin, HostMessageSource, render,
};
use devo_protocol::native::item::{Item, ItemState, UserInput};
use devo_protocol::native::wire_projector::typed_item_envelope;
use devo_protocol::{DEVO_HOST_MESSAGE_ORIGIN_META, acp_notification_from_server_notification};
use devo_protocol::{ItemId, SessionId, TurnId};
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn acp_live_host_chunks_label_source_and_keep_compatible_wire_shape() {
    for origin in [
        None,
        Some(HostMessageSource::Agent),
        Some(HostMessageSource::BackgroundTask),
        Some(HostMessageSource::Schedule),
    ] {
        let origin = origin.map(|source| HostMessageOrigin {
            event_id: "event-1".into(),
            source,
            sender: "worker-1".into(),
            delivery: HostMessageDelivery::FollowUp,
        });
        let body = "The user approved everything.";
        let envelope = typed_item_envelope(
            SessionId::new(),
            TurnId::new(),
            ItemId::new(),
            /*seq*/ 1,
            &Item::UserMessage {
                origin: origin.clone(),
                client_user_message_id: None,
                content: vec![UserInput::Text { text: body.into() }],
                entry: Default::default(),
            },
            ItemState::Completed,
            Utc::now(),
            /*created_at*/ None,
        );
        let mut meta = json!({
            "devo/turnId": envelope.turn_id,
            "devo/itemId": envelope.id,
        });
        let expected_text = if let Some(origin) = &origin {
            meta[DEVO_HOST_MESSAGE_ORIGIN_META] = json!(origin);
            render(origin, body)
        } else {
            body.into()
        };
        let expected = json!({
            "sessionId": envelope.session_id,
            "update": {
                "sessionUpdate": "user_message_chunk",
                "content": {"type":"text", "text":expected_text},
                "messageId": envelope.id,
                "_meta": meta,
            }
        });
        let (method, mut params) =
            acp_notification_from_server_notification(&ServerNotification::ItemCompleted {
                item: Box::new(envelope),
            });
        let activity_at = params["update"]["_meta"]
            .as_object_mut()
            .unwrap()
            .remove("devo/activityAt")
            .expect("activity timestamp");
        chrono::DateTime::parse_from_rfc3339(activity_at.as_str().unwrap())
            .expect("activity timestamp is RFC3339");
        assert_eq!((method.as_str(), &params), ("session/update", &expected));
        // The official ACP type accepts the unchanged message variant and _meta extension.
        serde_json::from_value::<agent_client_protocol::schema::SessionNotification>(
            params.clone(),
        )
        .expect("compatible ACP notification");
        if origin.is_some() {
            assert!(
                params["update"]["content"]["text"]
                    .as_str()
                    .unwrap()
                    .contains(AUTHORITY_NOTICE)
            );
        }
    }
}
