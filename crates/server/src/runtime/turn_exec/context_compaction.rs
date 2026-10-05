use std::sync::Arc;

use chrono::Utc;
use devo_core::ResponseItem;
use devo_protocol::native::event::ServerNotification;
use devo_protocol::native::ids::{
    ItemId as NativeItemId, SessionId as NativeSessionId, TurnId as NativeTurnId,
};
use devo_protocol::native::item::ItemState;
use devo_protocol::native::item::{CompactionTrigger, ContextUsage, Item};
use devo_protocol::native::wire_projector::typed_item_envelope;

use super::super::ServerRuntime;

#[derive(Default)]
pub(super) struct ContextCompactionLifecycle {
    item_id: Option<NativeItemId>,
}

impl ContextCompactionLifecycle {
    pub(super) async fn start(
        &mut self,
        runtime: &Arc<ServerRuntime>,
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
    ) {
        if self.item_id.is_some() {
            self.fail(
                runtime,
                session_id,
                turn_id,
                "compaction restarted before the previous lifecycle completed".to_string(),
            )
            .await;
        }
        let item_id = NativeItemId::new();
        self.item_id = Some(item_id);
        runtime
            .emit_native_item_started(
                session_id,
                turn_id,
                item_id,
                None,
                compaction_started_item(),
            )
            .await;
    }

    pub(super) async fn complete(
        &mut self,
        runtime: &Arc<ServerRuntime>,
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        compacted_items: Vec<ResponseItem>,
    ) {
        let Some(item_id) = self.item_id.take() else {
            return;
        };
        let item_seq = runtime
            .persist_in_turn_compaction(session_id, turn_id, item_id, &compacted_items)
            .await;
        runtime
            .emit_native_item_completed(
                session_id,
                turn_id,
                item_id,
                item_seq,
                compaction_completed_item(),
            )
            .await;
    }

    pub(super) async fn fail(
        &mut self,
        runtime: &Arc<ServerRuntime>,
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        message: String,
    ) {
        if let Some(item_id) = self.item_id.take() {
            runtime
                .broadcast_notification(failed_item_notification(
                    session_id, turn_id, item_id, &message,
                ))
                .await;
        }
        runtime
            .broadcast_notification(ServerNotification::ContextCompactionFailed {
                session_id,
                message,
            })
            .await;
    }

    pub(super) async fn close_if_open(
        &mut self,
        runtime: &Arc<ServerRuntime>,
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
    ) {
        if self.item_id.is_some() {
            self.fail(
                runtime,
                session_id,
                turn_id,
                "compaction lifecycle ended before completion".to_string(),
            )
            .await;
        }
    }
}

fn compaction_usage() -> ContextUsage {
    ContextUsage {
        measured: false,
        ..ContextUsage::default()
    }
}

fn compaction_started_item() -> Item {
    Item::ContextCompaction {
        trigger: CompactionTrigger::AutoThreshold,
        before: compaction_usage(),
        after: None,
        summary: Some("Compaction started".to_string()),
    }
}

fn compaction_completed_item() -> Item {
    Item::ContextCompaction {
        trigger: CompactionTrigger::AutoThreshold,
        before: compaction_usage(),
        after: None,
        summary: Some("Context compacted".to_string()),
    }
}

fn compaction_failed_item(message: &str) -> Item {
    Item::ContextCompaction {
        trigger: CompactionTrigger::AutoThreshold,
        before: compaction_usage(),
        after: None,
        summary: Some(format!("Compaction failed: {message}")),
    }
}

fn manual_compaction_started_item() -> Item {
    Item::ContextCompaction {
        trigger: CompactionTrigger::Manual,
        before: compaction_usage(),
        after: None,
        summary: Some("Compaction started".to_string()),
    }
}

fn manual_compaction_completed_item() -> Item {
    Item::ContextCompaction {
        trigger: CompactionTrigger::Manual,
        before: compaction_usage(),
        after: None,
        summary: Some("Context compacted".to_string()),
    }
}

#[cfg(test)]
pub(super) fn started_event(
    session_id: devo_core::SessionId,
    turn_id: devo_core::TurnId,
    item_id: devo_core::ItemId,
) -> ServerNotification {
    item_notification_from_legacy(
        session_id,
        turn_id,
        item_id,
        None,
        /*completed*/ false,
        compaction_started_item(),
        ItemState::Running,
    )
}

#[cfg(test)]
pub(super) fn completed_event(
    session_id: devo_core::SessionId,
    turn_id: devo_core::TurnId,
    item_id: devo_core::ItemId,
    item_seq: Option<u64>,
) -> ServerNotification {
    item_notification_from_legacy(
        session_id,
        turn_id,
        item_id,
        item_seq,
        /*completed*/ true,
        compaction_completed_item(),
        ItemState::Completed,
    )
}

pub(super) fn failed_item_notification(
    session_id: NativeSessionId,
    turn_id: NativeTurnId,
    item_id: NativeItemId,
    message: &str,
) -> ServerNotification {
    item_notification_from_native_ids(
        session_id,
        turn_id,
        item_id,
        None,
        /*completed*/ true,
        compaction_failed_item(message),
        ItemState::Completed,
    )
}

pub(crate) fn manual_compaction_started_event(
    session_id: NativeSessionId,
    turn_id: NativeTurnId,
    item_id: NativeItemId,
    item_seq: Option<u64>,
) -> ServerNotification {
    item_notification_from_native_ids(
        session_id,
        turn_id,
        item_id,
        item_seq,
        /*completed*/ false,
        manual_compaction_started_item(),
        ItemState::Running,
    )
}

pub(crate) fn manual_compaction_completed_event(
    session_id: NativeSessionId,
    turn_id: NativeTurnId,
    item_id: NativeItemId,
    item_seq: u64,
) -> ServerNotification {
    item_notification_from_native_ids(
        session_id,
        turn_id,
        item_id,
        Some(item_seq),
        /*completed*/ true,
        manual_compaction_completed_item(),
        ItemState::Completed,
    )
}

pub(crate) fn manual_compaction_item_failed_event(
    session_id: NativeSessionId,
    turn_id: NativeTurnId,
    item_id: NativeItemId,
    message: String,
) -> ServerNotification {
    item_notification_from_native_ids(
        session_id,
        turn_id,
        item_id,
        None,
        /*completed*/ true,
        Item::ContextCompaction {
            trigger: CompactionTrigger::Manual,
            before: compaction_usage(),
            after: None,
            summary: Some(format!("Compaction failed: {message}")),
        },
        ItemState::Completed,
    )
}

#[cfg(test)]
fn item_notification_from_legacy(
    session_id: devo_core::SessionId,
    turn_id: devo_core::TurnId,
    item_id: devo_core::ItemId,
    item_seq: Option<u64>,
    completed: bool,
    native_item: Item,
    state: ItemState,
) -> ServerNotification {
    // test fixture: bare UUID wire form via from_legacy_uuid
    item_notification_from_native_ids(
        session_id,
        turn_id,
        item_id,
        item_seq,
        completed,
        native_item,
        state,
    )
}

fn item_notification_from_native_ids(
    session_id: NativeSessionId,
    turn_id: NativeTurnId,
    item_id: NativeItemId,
    item_seq: Option<u64>,
    completed: bool,
    native_item: Item,
    state: ItemState,
) -> ServerNotification {
    use devo_protocol::native::wire_projector::item_lifecycle_server_notification;

    item_lifecycle_server_notification(
        &typed_item_envelope(
            session_id,
            turn_id,
            item_id,
            item_seq.unwrap_or(0),
            &native_item,
            state,
            Utc::now(),
            None,
        ),
        completed,
    )
}
