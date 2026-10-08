//! Lightweight subscription snapshots and reconnect text recovery.
use super::super::ServerRuntime;
use devo_protocol::native::event::{DeltaChannel, LiveItemSnapshot, StreamSelector};
use devo_protocol::native::item::Item;
use devo_protocol::native::session::SessionStatus;

impl ServerRuntime {
    /// Folder snapshots use the startup-maintained index, never replay every
    /// transcript while holding the connection barrier. Active status comes
    /// from the registry without waiting on actor mailboxes.
    pub(super) async fn native_sessions_for_cwd(
        &self,
        cwd: &std::path::Path,
    ) -> anyhow::Result<Vec<devo_protocol::native::session::Session>> {
        let active = self.active_turns.active_turn_ids().await;
        Ok(self
            .deps
            .db
            .list_sessions()?
            .into_iter()
            .filter(|row| !row.ephemeral && row.cwd == cwd)
            .map(|row| {
                let mut session = row.into_native_session();
                session.active_turn_id = active.get(&session.id).copied();
                session.status = if session.active_turn_id.is_some() {
                    SessionStatus::Active
                } else {
                    SessionStatus::Idle
                };
                session.sync_activity();
                session
            })
            .collect())
    }

    /// Clone transient text after registering the subscriber. Never await an
    /// actor or stream lock while holding the connections barrier: turns use
    /// that barrier to publish the events needed to complete their work.
    pub(super) async fn recovery_snapshots(
        &self,
        selectors: &[StreamSelector],
    ) -> Vec<LiveItemSnapshot> {
        let mut snapshots = Vec::new();
        for selector in selectors {
            let StreamSelector::Session { session_id } = selector else {
                continue;
            };
            let Some(stream) = self.active_stream_state(*session_id).await else {
                continue;
            };
            let (turn_id, assistant, reasoning) = {
                let stream = stream.lock().await;
                let Some(inline) = stream.turn_inline.as_ref() else {
                    continue;
                };
                (
                    inline.turn_id,
                    stream.deferred_assistant.clone(),
                    stream.deferred_reasoning.clone(),
                )
            };
            if let Some((item_id, seq, text)) = assistant {
                snapshots.push(super::native_surface::live_item_snapshot(
                    *session_id,
                    turn_id,
                    item_id,
                    seq,
                    Item::AssistantMessage { text: text.clone() },
                    DeltaChannel::AssistantMessage,
                    text,
                ));
            }
            if let Some((item_id, seq, text)) = reasoning {
                snapshots.push(super::native_surface::live_item_snapshot(
                    *session_id,
                    turn_id,
                    item_id,
                    seq,
                    Item::Reasoning {
                        text: text.clone(),
                        provider_payload_ref: None,
                    },
                    DeltaChannel::Reasoning,
                    text,
                ));
            }
        }
        snapshots
    }
}
