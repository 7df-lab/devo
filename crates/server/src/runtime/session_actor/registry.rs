use std::sync::Arc;

use devo_core::TurnId;
use devo_protocol::SessionId;

use super::state::SpawnSnapshot;
use crate::runtime::ServerRuntime;

impl ServerRuntime {
    pub(crate) fn runtime_arc(&self) -> Arc<ServerRuntime> {
        self.self_weak
            .upgrade()
            .expect("server runtime weak reference should remain alive")
    }

    pub(crate) async fn session(&self, session_id: SessionId) -> Option<super::SessionHandle> {
        self.sessions.lock().await.get(&session_id).cloned()
    }

    pub(crate) async fn insert_session_actor(
        &self,
        state: super::SessionActorState,
    ) -> super::SessionHandle {
        let runtime = self.runtime_arc();
        let session_id = state.session_id();
        let handle = super::SessionHandle::spawn(session_id, state, runtime);
        self.sessions
            .lock()
            .await
            .insert(session_id, handle.clone());
        handle
    }

    pub(crate) async fn remove_session_actor(
        &self,
        session_id: SessionId,
    ) -> Option<super::SessionHandle> {
        let handle = self.sessions.lock().await.remove(&session_id)?;
        self.session_interactive.clear_session(session_id).await;
        self.active_spawn_snapshots.lock().await.remove(&session_id);
        Some(handle)
    }

    pub(crate) async fn list_session_handles(&self) -> Vec<super::SessionHandle> {
        self.sessions.lock().await.values().cloned().collect()
    }

    pub(crate) async fn list_session_summaries_from_actors(
        &self,
    ) -> Vec<crate::session::SessionMetadata> {
        let handles = self.list_session_handles().await;
        let mut summaries = Vec::with_capacity(handles.len());
        for handle in handles {
            if let Some(summary) = handle.summary().await {
                summaries.push(summary);
            }
        }
        summaries.sort_by(|left, right| {
            right
                .last_activity_at
                .cmp(&left.last_activity_at)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
        });
        summaries
    }

    pub(crate) async fn register_turn_spawn_snapshot(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        snapshot: Arc<SpawnSnapshot>,
    ) {
        self.active_spawn_snapshots
            .lock()
            .await
            .entry(session_id)
            .or_default()
            .insert(turn_id, snapshot);
    }

    pub(crate) async fn clear_turn_spawn_snapshot(&self, session_id: SessionId, turn_id: TurnId) {
        let mut snapshots = self.active_spawn_snapshots.lock().await;
        if let Some(turns) = snapshots.get_mut(&session_id) {
            turns.remove(&turn_id);
            if turns.is_empty() {
                snapshots.remove(&session_id);
            }
        }
    }

    /// Snapshot registered at turn start while the session actor is busy executing.
    pub(crate) async fn active_spawn_snapshot_for_session(
        &self,
        session_id: SessionId,
    ) -> Option<SpawnSnapshot> {
        let snapshots = self.active_spawn_snapshots.lock().await;
        let turns = snapshots.get(&session_id)?;
        turns.values().next().map(|snapshot| (**snapshot).clone())
    }

    pub(crate) async fn register_active_stream(
        &self,
        session_id: SessionId,
        stream: Arc<tokio::sync::Mutex<super::state::SessionStreamState>>,
    ) {
        self.active_stream_states
            .lock()
            .await
            .insert(session_id, stream);
    }

    pub(crate) async fn unregister_active_stream(&self, session_id: SessionId) {
        self.active_stream_states.lock().await.remove(&session_id);
    }

    pub(crate) async fn active_stream_state(
        &self,
        session_id: SessionId,
    ) -> Option<Arc<tokio::sync::Mutex<super::state::SessionStreamState>>> {
        self.active_stream_states
            .lock()
            .await
            .get(&session_id)
            .cloned()
    }

    pub(crate) async fn session_record_snapshot(
        &self,
        session_id: SessionId,
    ) -> Option<devo_core::SessionRecord> {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return inline.record.clone();
            }
        }
        let handle = self.session(session_id).await?;
        handle.record().await.flatten()
    }

    pub(crate) async fn session_summary_snapshot(
        &self,
        session_id: SessionId,
    ) -> Option<crate::session::SessionMetadata> {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return Some(inline.summary.clone());
            }
        }
        let handle = self.session(session_id).await?;
        handle.summary().await
    }

    /// Reads the session's collaboration mode, preferring the in-flight turn's
    /// inline snapshot over a mailbox round-trip.
    ///
    /// Callers invoked from within `session_id`'s own actor turn (e.g. while
    /// finalizing that turn) must go through this path: the actor's mailbox
    /// is not polled again until the turn finishes, so asking its own
    /// `SessionHandle` for a reply here would deadlock.
    pub(crate) async fn session_collaboration_mode(
        &self,
        session_id: SessionId,
    ) -> Option<devo_protocol::CollaborationMode> {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return Some(inline.collaboration_mode);
            }
        }
        let handle = self.session(session_id).await?;
        handle.collaboration_mode().await
    }

    /// Reads a session's parent id, preferring the in-flight turn inline snapshot
    /// and agent-registry hierarchy over a mailbox round-trip.
    pub(crate) async fn session_parent_id_snapshot(
        &self,
        session_id: SessionId,
    ) -> Option<Option<SessionId>> {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return Some(inline.summary.parent_session_id);
            }
        }
        {
            let registries = self.agent_registries.lock().await;
            for registry in registries.values() {
                if let Some(parent_id) = registry.child_to_parent.get(&session_id).copied() {
                    return Some(Some(parent_id));
                }
            }
        }
        let handle = self.session(session_id).await?;
        handle.parent_session_id().await
    }
}
