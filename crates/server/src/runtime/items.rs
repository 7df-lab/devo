use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use chrono::Utc;
use devo_protocol::native::ids::{
    ItemId as NativeItemId, SessionId as NativeSessionId, TurnId as NativeTurnId,
};
use devo_protocol::native::item::Item as NativeItem;
use devo_protocol::native::item::ItemEnvelope;
use devo_protocol::native::item::ItemState;
use devo_protocol::native::item::UserInput;
use devo_protocol::native::item::UserMessageEntry;
use devo_protocol::native::wire_projector::{item_started_state, typed_item_envelope};

use super::*;

/// Build a Native user-message item for first-party emit (no legacy payload bag).
///
/// Images are listed before text so the TUI projects
/// `[image:path] describe it.` rather than `describe it.[image:path]`.
pub(crate) fn native_user_message_item(
    text: impl Into<String>,
    image_paths: &[PathBuf],
    entry: UserMessageEntry,
) -> NativeItem {
    native_user_message_item_with_identity(text, image_paths, entry, None)
}

/// Preserves a caller-supplied domain ID for downstream message replay checks.
pub(crate) fn native_user_message_item_with_client_message_id(
    text: impl Into<String>,
    image_paths: &[PathBuf],
    entry: UserMessageEntry,
    client_user_message_id: String,
) -> NativeItem {
    native_user_message_item_with_identity(text, image_paths, entry, Some(client_user_message_id))
}

fn native_user_message_item_with_identity(
    text: impl Into<String>,
    image_paths: &[PathBuf],
    entry: UserMessageEntry,
    client_user_message_id: Option<String>,
) -> NativeItem {
    let text = text.into();
    let mut content = Vec::with_capacity(1 + image_paths.len());
    for path in image_paths {
        content.push(UserInput::LocalImage {
            path: path.clone(),
            detail: None,
        });
    }
    if !text.trim().is_empty() {
        content.push(UserInput::Text { text });
    }
    NativeItem::UserMessage {
        client_user_message_id,
        content,
        entry,
    }
}

/// Used only when a turn event stream is active but inline state is missing.
/// Avoids mailbox round-trips that deadlock the session actor.
fn next_fallback_item_seq() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1 << 32);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn legacy_session_for_lookup(session_id: &NativeSessionId) -> Option<NativeSessionId> {
    Some(*session_id)
}

impl ServerRuntime {
    /// Persist session summary to SQLite if the session is durable.
    /// The rollout file is the authoritative store, so failures here are
    /// logged as warnings rather than propagated.
    pub(super) async fn persist_session_summary_if_persistent(
        &self,
        session_id: SessionId,
        summary: &crate::runtime_session_summary::RuntimeSessionSummary,
    ) {
        if !summary.ephemeral
            && let Err(err) = self.deps.db.upsert_session(summary, None)
        {
            tracing::warn!(
                session_id = %session_id,
                error = %err,
                "failed to persist session metadata to database"
            );
        }
    }

    pub(super) async fn emit_turn_native_item(
        &self,
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        native_item: NativeItem,
    ) {
        let (item_id, item_seq) = self
            .start_native_item(session_id, turn_id, native_item.clone())
            .await;
        self.complete_native_item(session_id, turn_id, item_id, item_seq, native_item)
            .await;
    }

    pub(super) async fn start_native_item(
        &self,
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        native_item: NativeItem,
    ) -> (NativeItemId, u64) {
        let item_id = NativeItemId::new();
        let item_seq = self.allocate_item_sequence(&session_id).await;
        self.remember_item_started_at(&session_id, item_id).await;
        self.emit_native_item_started(session_id, turn_id, item_id, Some(item_seq), native_item)
            .await;
        (item_id, item_seq)
    }

    async fn remember_item_started_at(&self, session_id: &NativeSessionId, item_id: NativeItemId) {
        let Some(legacy) = legacy_session_for_lookup(session_id) else {
            return;
        };
        let Some(stream) = self.active_stream_state(legacy).await else {
            return;
        };
        let mut stream = stream.lock().await;
        if let Some(inline) = stream.turn_inline.as_mut() {
            inline.item_started_at.insert(item_id, chrono::Utc::now());
        }
    }

    async fn take_item_started_at(
        &self,
        session_id: &NativeSessionId,
        item_id: &NativeItemId,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        let legacy = legacy_session_for_lookup(session_id)?;
        let stream = self.active_stream_state(legacy).await?;
        let mut stream = stream.lock().await;
        stream
            .turn_inline
            .as_mut()
            .and_then(|inline| inline.item_started_at.remove(item_id))
    }

    pub(super) async fn emit_native_item_started(
        &self,
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        item_id: NativeItemId,
        item_seq: Option<u64>,
        native_item: NativeItem,
    ) {
        let state = item_started_state(&native_item);
        let envelope = typed_item_envelope(
            session_id,
            turn_id,
            item_id,
            item_seq.unwrap_or(0),
            &native_item,
            state,
            Utc::now(),
            None,
        );
        self.broadcast_item_lifecycle(envelope, /*completed*/ false)
            .await;
    }

    pub(super) async fn emit_native_item_completed(
        &self,
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        item_id: NativeItemId,
        item_seq: Option<u64>,
        native_item: NativeItem,
    ) {
        let started_at = self.take_item_started_at(&session_id, &item_id).await;
        let envelope = typed_item_envelope(
            session_id,
            turn_id,
            item_id,
            item_seq.unwrap_or(0),
            &native_item,
            ItemState::Completed,
            Utc::now(),
            started_at,
        );
        self.broadcast_item_lifecycle(envelope, /*completed*/ true)
            .await;
    }

    /// Native wire from emit-site `ServerNotification`; ACP projects from the
    /// same notification at fan-out.
    async fn broadcast_item_lifecycle(&self, envelope: ItemEnvelope, completed: bool) {
        use devo_protocol::native::wire_projector::item_lifecycle_server_notification;

        let notification = item_lifecycle_server_notification(&envelope, completed);
        self.broadcast_notification(notification).await;
    }

    pub(super) async fn complete_native_item(
        &self,
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        item_id: NativeItemId,
        item_seq: u64,
        native_item: NativeItem,
    ) {
        let started_at = self.take_item_started_at(&session_id, &item_id).await;
        self.persist_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            native_item.clone(),
            started_at,
        )
        .await;
        let envelope = typed_item_envelope(
            session_id,
            turn_id,
            item_id,
            item_seq,
            &native_item,
            ItemState::Completed,
            Utc::now(),
            started_at,
        );
        self.broadcast_item_lifecycle(envelope, /*completed*/ true)
            .await;
    }

    pub(super) async fn persist_item(
        &self,
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        item_id: NativeItemId,
        item_seq: u64,
        native_item: NativeItem,
        started_at: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        use crate::persisted_native_item::PersistedNativeItem;
        use crate::persisted_native_item::history_entry_from_native_item;

        let Some(legacy_session_id) = legacy_session_for_lookup(&session_id) else {
            tracing::warn!(
                session_id = %session_id,
                "persist_item skipped: Native session id is not bridgeable to legacy lookup key"
            );
            return;
        };

        let history_entry = history_entry_from_native_item(&native_item);
        if let Some(stream) = self.active_stream_state(legacy_session_id).await {
            // Mutate inline state under the lock, then release before any
            // blocking rollout I/O so the event stream cannot pin the async
            // mutex across synchronous disk writes.
            let inline_rollout = {
                let mut stream = stream.lock().await;
                stream.turn_inline.as_mut().map(|inline| {
                    if inline.turn_id == turn_id
                        && let Some(history_entry) = history_entry.clone()
                    {
                        inline.history_items.push(history_entry);
                    }
                    if inline.turn_id == turn_id {
                        inline.persisted_turn_items.push(PersistedNativeItem::new(
                            turn_id,
                            inline.turn_kind,
                            item_id,
                            native_item.clone(),
                        ));
                    }
                    let parent_id = inline.transcript_leaf_id;
                    inline.transcript_leaf_id = Some(item_id);
                    inline.leaf_epoch = inline.leaf_epoch.saturating_add(1);
                    (
                        inline.rollout_path.clone(),
                        parent_id,
                        inline.leaf_epoch,
                        inline.summary.session_id(),
                    )
                })
            };
            if let Some((rollout_path, parent_id, leaf_epoch, persist_session_id)) = inline_rollout
            {
                if let Some(rollout_path) = rollout_path {
                    let mut envelope = typed_item_envelope(
                        session_id,
                        turn_id,
                        item_id,
                        item_seq,
                        &native_item,
                        ItemState::Completed,
                        Utc::now(),
                        started_at,
                    );
                    envelope.parent_id = parent_id;
                    if let Err(error) = self.rollout_store.append_tree_edge_at(
                        &rollout_path,
                        persist_session_id,
                        item_id,
                        parent_id,
                    ) {
                        tracing::warn!(session_id = %legacy_session_id, error = %error, "failed to persist tree edge");
                    }
                    if let Err(error) = self
                        .rollout_store
                        .append_canonical_item_at(&rollout_path, envelope)
                    {
                        tracing::warn!(session_id = %legacy_session_id, error = %error, "failed to persist item line");
                    }
                    if let Err(error) = self.rollout_store.append_session_leaf_at(
                        &rollout_path,
                        persist_session_id,
                        Some(item_id),
                        leaf_epoch,
                    ) {
                        tracing::warn!(session_id = %legacy_session_id, error = %error, "failed to persist session leaf");
                    }
                }
                return;
            }
            // Active stream is registered but inline state is missing. The session
            // actor is not polling its mailbox until the stream finishes, so we
            // must not fall through to blocking actor commands.
            tracing::warn!(
                session_id = %legacy_session_id,
                turn_id = %turn_id,
                "persist_item skipped: active turn stream has no inline state"
            );
            return;
        }
        let Some(session_handle) = self.session(legacy_session_id).await else {
            return;
        };
        if let Some(history_entry) = history_entry {
            session_handle.append_history_item(history_entry).await;
        }
        let legacy_turn_id = turn_id;
        let Some(prep) = session_handle.prepare_persist_item(legacy_turn_id).await else {
            return;
        };
        session_handle
            .append_persisted_item(PersistedNativeItem::new(
                turn_id,
                prep.turn_kind,
                item_id,
                native_item.clone(),
            ))
            .await;
        if let Some(rollout_path) = prep.rollout_path {
            let parent_id = prep.transcript_leaf_id;
            let leaf_epoch = prep.leaf_epoch.saturating_add(1);
            let mut envelope = typed_item_envelope(
                session_id,
                turn_id,
                item_id,
                item_seq,
                &native_item,
                ItemState::Completed,
                Utc::now(),
                started_at,
            );
            envelope.parent_id = parent_id;
            if let Err(error) = self.rollout_store.append_tree_edge_at(
                &rollout_path,
                legacy_session_id,
                item_id,
                parent_id,
            ) {
                tracing::warn!(session_id = %legacy_session_id, error = %error, "failed to persist tree edge");
            }
            if let Err(error) = self
                .rollout_store
                .append_canonical_item_at(&rollout_path, envelope)
            {
                tracing::warn!(session_id = %legacy_session_id, error = %error, "failed to persist item line");
            }
            if let Err(error) = self.rollout_store.append_session_leaf_at(
                &rollout_path,
                legacy_session_id,
                Some(item_id),
                leaf_epoch,
            ) {
                tracing::warn!(session_id = %legacy_session_id, error = %error, "failed to persist session leaf");
            }
            session_handle
                .set_transcript_leaf(
                    Some(item_id),
                    leaf_epoch,
                    /*scoped_item_ids*/ None,
                    /*compaction_snapshot*/ None,
                )
                .await;
        }
    }

    pub(super) async fn allocate_item_sequence(&self, session_id: &NativeSessionId) -> u64 {
        let Some(legacy_session_id) = legacy_session_for_lookup(session_id) else {
            return next_fallback_item_seq();
        };
        if let Some(stream) = self.active_stream_state(legacy_session_id).await {
            let mut stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_mut() {
                return inline.allocate_item_seq();
            }
            // Same deadlock constraint as persist_item: never wait on the actor
            // mailbox while its turn event stream is registered.
            return next_fallback_item_seq();
        }
        if let Some(handle) = self.session(legacy_session_id).await
            && let Some(item_seq) = handle.allocate_item_seq().await
        {
            return item_seq;
        }
        1
    }
}

pub(crate) fn render_input_items(
    input: &[devo_protocol::native::item::UserInput],
) -> Option<String> {
    use devo_protocol::native::item::UserInput;
    let mut rendered = String::new();
    for item in input {
        let part = match item {
            UserInput::Text { text } => {
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                Cow::Borrowed(text)
            }
            UserInput::Skill { name } => Cow::Owned(format!("[skill:{name}]")),
            UserInput::LocalImage { path, .. } => Cow::Owned(format!("[image:{}]", path.display())),
            UserInput::Mention { uri } => Cow::Owned(format!("[mention:{uri}]")),
            UserInput::Image { uri, .. } => Cow::Owned(format!("[image:{uri}]")),
            UserInput::Audio { uri, .. } => Cow::Owned(format!("[audio:{uri}]")),
        };
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str(&part);
    }
    (!rendered.is_empty()).then_some(rendered)
}

/// User-bubble text without image placeholders. Image parts stay as structured
/// `LocalImage` / `Image` content so clients do not render `[image:…]` twice.
pub(crate) fn render_input_text_without_images(
    input: &[devo_protocol::native::item::UserInput],
) -> String {
    use devo_protocol::native::item::UserInput;
    let mut rendered = String::new();
    for item in input {
        let part = match item {
            UserInput::Text { text } => {
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                Cow::Borrowed(text)
            }
            UserInput::Skill { name } => Cow::Owned(format!("[skill:{name}]")),
            UserInput::Mention { uri } => Cow::Owned(format!("[mention:{uri}]")),
            UserInput::LocalImage { .. } | UserInput::Image { .. } | UserInput::Audio { .. } => {
                continue;
            }
        };
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str(&part);
    }
    rendered
}

#[cfg(test)]
mod client_message_id_tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn user_message_identity_is_preserved_in_the_native_item() {
        assert_eq!(
            native_user_message_item_with_client_message_id(
                "hello",
                &[],
                UserMessageEntry::TurnStart,
                "schedule:job:occurrence-1".to_string(),
            ),
            NativeItem::UserMessage {
                client_user_message_id: Some("schedule:job:occurrence-1".to_string()),
                content: vec![UserInput::Text {
                    text: "hello".to_string(),
                }],
                entry: UserMessageEntry::TurnStart,
            }
        );
        assert_eq!(
            native_user_message_item("hello", &[], UserMessageEntry::TurnStart),
            NativeItem::UserMessage {
                client_user_message_id: None,
                content: vec![UserInput::Text {
                    text: "hello".to_string(),
                }],
                entry: UserMessageEntry::TurnStart,
            }
        );
    }
}
