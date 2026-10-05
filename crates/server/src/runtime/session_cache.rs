use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use tokio::sync::Mutex;

use devo_protocol::native::ids::SessionId;

use crate::execution::RuntimeSession;
use crate::runtime::ServerRuntime;
use crate::runtime::session_actor::SessionActorState;
use crate::runtime::session_actor::SessionHandle;

pub(crate) const PARENT_SESSION_LRU_CAPACITY: usize = 16;

#[derive(Debug, Default)]
pub(crate) struct SessionLoadGate {
    locks: Mutex<HashMap<SessionId, Arc<Mutex<()>>>>,
}

pub(crate) struct SessionLoadPermit {
    session_id: SessionId,
    lock: Arc<Mutex<()>>,
    gate: Arc<SessionLoadGate>,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl SessionLoadGate {
    pub(crate) async fn acquire(self: &Arc<Self>, session_id: SessionId) -> SessionLoadPermit {
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(session_id)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let guard = lock.clone().lock_owned().await;
        SessionLoadPermit {
            session_id,
            lock,
            gate: Arc::clone(self),
            guard: Some(guard),
        }
    }
}

impl Drop for SessionLoadPermit {
    fn drop(&mut self) {
        drop(self.guard.take());
        if Arc::strong_count(&self.lock) != 2 {
            return;
        }
        let session_id = self.session_id;
        let lock = Arc::clone(&self.lock);
        let gate = Arc::clone(&self.gate);
        tokio::spawn(async move {
            let mut locks = gate.locks.lock().await;
            if locks.get(&session_id).is_some_and(|existing| {
                Arc::ptr_eq(existing, &lock) && Arc::strong_count(existing) == 2
            }) {
                locks.remove(&session_id);
            }
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum LoadSessionError {
    #[error("session not found")]
    SessionNotFound,
    #[error("session metadata exists but rollout file is missing; session cannot be restored")]
    RolloutMissing,
    #[error("session is already open in another Devo process (pid {holder_pid})")]
    SessionLocked { holder_pid: u32 },
    #[error("failed to restore session: {0}")]
    RestoreFailed(String),
}

/// Exclusive cross-process lock on a session rollout.
///
/// Two Devo processes must never run turns against the same session
/// concurrently: their item sequence numbers and turn records interleave into
/// one corrupt history (both processes append user/assistant items under
/// overlapping ids). The lock is an flock on `<rollout>.jsonl.lock`, held by
/// the owning session actor (`SessionActorState::session_file_lock`); dropping
/// the actor (LRU eviction or process exit) releases it. Crash-safe: the OS
/// releases the flock when the holding process dies, so a crashed Devo never
/// leaves a stale lock behind.
pub(crate) struct SessionFileLock {
    _file: std::fs::File,
}

impl std::fmt::Debug for SessionFileLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionFileLock")
    }
}

impl SessionFileLock {
    /// Acquire the exclusive lock for `rollout_path`. On contention, the
    /// current holder's pid (written into the lock file) is returned so the
    /// refusal can name the conflicting process.
    pub(crate) fn acquire(rollout_path: &std::path::Path) -> Result<Self, SessionLockContention> {
        use fs2::FileExt;
        use std::io::Seek as _;
        use std::io::Write as _;

        let lock_path = session_lock_path(rollout_path);
        let file = match std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
        {
            Ok(file) => file,
            Err(error) => {
                // Without a lock file we cannot enforce exclusivity; refusing
                // to open would brick sessions on read-only homes, so proceed
                // unlocked (same behavior as before the lock existed).
                tracing::warn!(
                    path = %lock_path.display(),
                    %error,
                    "cannot open session lock file; continuing without cross-process lock"
                );
                return Ok(Self {
                    _file: open_sentinel_file(),
                });
            }
        };
        if let Err(error) = file.try_lock_exclusive() {
            let mut holder_pid = String::new();
            {
                let mut readable = &file;
                let _ = readable.rewind();
                let _ = std::io::Read::read_to_string(&mut readable, &mut holder_pid);
            }
            let holder_pid = holder_pid.trim().parse::<u32>().unwrap_or(0);
            // Same-process contention (holder pid == ours) is expected:
            // integration tests run sequential runtimes in one process, and
            // an evicted-then-reloaded session re-acquires while the old
            // actor's file description may still be closing. Only a REAL
            // other process is refused.
            if holder_pid != 0 && holder_pid == std::process::id() {
                tracing::debug!(
                    path = %lock_path.display(),
                    "session lock contended within this process; proceeding unlocked"
                );
                return Ok(Self { _file: file });
            }
            tracing::warn!(
                path = %lock_path.display(),
                %error,
                holder_pid,
                "session rollout is locked by another Devo process"
            );
            return Err(SessionLockContention { holder_pid });
        }
        // Record this pid for contention reports; best-effort.
        {
            let mut writable = &file;
            let _ = writable.rewind();
            let _ = writable.set_len(0);
            let _ = writeln!(writable, "{}", std::process::id());
        }
        Ok(Self { _file: file })
    }
}

/// A lock attempt lost to a live holder.
#[derive(Debug)]
pub(crate) struct SessionLockContention {
    pub(crate) holder_pid: u32,
}

/// Stand-in file handle for environments where the lock file cannot be
/// created (keeps the struct shape uniform; grants no lock).
fn open_sentinel_file() -> std::fs::File {
    std::fs::OpenOptions::new()
        .read(true)
        .open("/dev/null")
        .unwrap_or_else(|_| {
            // Windows fallback: an invalid handle never matters because the
            // flock itself was already skipped.
            std::fs::File::create(std::env::temp_dir().join(".devo-session-lock-sentinel"))
                .unwrap_or_else(|_| unreachable!("temp dir must be writable"))
        })
}

fn session_lock_path(rollout_path: &std::path::Path) -> std::path::PathBuf {
    let mut name = rollout_path
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_else(|| std::ffi::OsString::from("session"));
    name.push(".lock");
    rollout_path.with_file_name(name)
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ParentSessionLru {
    order: VecDeque<SessionId>,
    capacity: usize,
}

impl ParentSessionLru {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            order: VecDeque::new(),
            capacity,
        }
    }

    pub(crate) fn touch(&mut self, session_id: SessionId) {
        self.order.retain(|id| id != &session_id);
        self.order.push_front(session_id);
    }

    pub(crate) fn remove(&mut self, session_id: &SessionId) {
        self.order.retain(|id| id != session_id);
    }

    pub(crate) fn len(&self) -> usize {
        self.order.len()
    }
}

impl ServerRuntime {
    pub(crate) async fn get_or_load_parent_session(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> Result<SessionHandle, LoadSessionError> {
        if let Some(handle) = self.session(session_id).await {
            self.touch_root_session_lru_if_needed(&handle).await;
            return Ok(handle);
        }

        // Keep actor hydration from racing a durable metadata write. The
        // metadata handler intentionally does not hydrate the actor, but a
        // subsequent resume must observe any field lines written before it
        // starts rebuilding the runtime session.
        let _metadata_write_permit = self.session_metadata_write_gate.acquire(session_id).await;
        let _load_permit = self.parent_session_load_gate.acquire(session_id).await;
        if let Some(handle) = self.session(session_id).await {
            self.touch_root_session_lru_if_needed(&handle).await;
            return Ok(handle);
        }

        let index = self
            .deps
            .db
            .get_session_index(&session_id)
            .map_err(|error| LoadSessionError::RestoreFailed(error.to_string()))?
            .ok_or(LoadSessionError::SessionNotFound)?;
        let is_subagent = index.session.agent_path.is_some();

        let stored_rollout_path = index.rollout_path.clone();
        let rollout_path = match stored_rollout_path {
            Some(ref path) if path.exists() => path.clone(),
            Some(_) | None => self
                .rollout_store
                .find_rollout_by_session_id(&session_id)
                .map_err(|error| LoadSessionError::RestoreFailed(error.to_string()))?
                .filter(|path| path.exists())
                .ok_or(LoadSessionError::RolloutMissing)?,
        };

        if stored_rollout_path.as_ref() != Some(&rollout_path)
            && let Err(error) = self
                .deps
                .db
                .upsert_rollout_index_session(index.session.clone(), Some(rollout_path.as_path()))
        {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "failed to backfill rollout_path after suffix lookup"
            );
        }

        // Cross-process exclusivity: refuse to hydrate a rollout another live
        // Devo process owns. Concurrent turns from two processes interleave
        // items/turn records into one corrupt history.
        let session_lock = SessionFileLock::acquire(&rollout_path).map_err(|contention| {
            LoadSessionError::SessionLocked {
                holder_pid: contention.holder_pid,
            }
        })?;

        let runtime_session = self
            .hydrate_runtime_session(session_id, &rollout_path)
            .await
            .map_err(|error| LoadSessionError::RestoreFailed(error.to_string()))?;
        // Subagents have their own rollouts and must be openable from Agents
        // View, but stay off the root LRU so eviction still keys off top-level
        // sessions.
        let handle = if is_subagent {
            // Rebuild the parent registry linkage before the actor takes
            // ownership: without it the re-hydrated child cannot resolve its
            // parent (agent_message.send, wake turns, status events).
            self.restore_agent_registry_entry_for_hydrated_child(&runtime_session)
                .await;
            let mut state = SessionActorState::from_runtime_session(runtime_session);
            state.session_file_lock = Some(session_lock);
            self.insert_session_actor(state).await
        } else {
            self.insert_root_session_actor(runtime_session, Some(session_lock))
                .await?
        };
        if let Err(error) = self
            .materialize_abandoned_turn_recovery_if_needed(session_id)
            .await
        {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "failed to materialize abandoned-turn recovery after hydrate"
            );
        }
        Ok(handle)
    }

    async fn touch_root_session_lru_if_needed(&self, handle: &SessionHandle) {
        let Some(summary) = handle.summary().await else {
            return;
        };
        if summary.agent_path.is_some() {
            return;
        }
        self.touch_parent_session_lru(summary.session_id()).await;
    }

    pub(crate) async fn insert_root_session_actor(
        self: &Arc<Self>,
        runtime_session: RuntimeSession,
        session_file_lock: Option<SessionFileLock>,
    ) -> Result<SessionHandle, LoadSessionError> {
        let session_id = runtime_session.summary.session_id();
        // Restart durability for the auto-refine trigger: re-derive the
        // successful-turn counter from the journal instead of restarting at
        // zero (see seed_auto_refine_turn_count).
        super::refine::seed_auto_refine_turn_count(
            &runtime_session.summary.native.id,
            &runtime_session.persisted_turn_items,
        );
        let mut state = SessionActorState::from_runtime_session(runtime_session);
        state.session_file_lock = session_file_lock;
        let handle = self.insert_session_actor(state).await;
        self.touch_parent_session_lru(session_id).await;
        self.evict_parent_sessions_if_needed(Some(session_id)).await;
        self.resume_pending_queue_if_idle(session_id).await;
        Ok(handle)
    }

    pub(crate) async fn after_root_session_insert(self: &Arc<Self>, session_id: SessionId) {
        self.touch_parent_session_lru(session_id).await;
        self.evict_parent_sessions_if_needed(Some(session_id)).await;
    }

    pub(crate) async fn touch_parent_session_lru(&self, session_id: SessionId) {
        let mut lru = self.session_lru.lock().await;
        lru.touch(session_id);
    }

    async fn evict_parent_sessions_if_needed(self: &Arc<Self>, exclude: Option<SessionId>) {
        let candidates: Vec<SessionId> = {
            let lru = self.session_lru.lock().await;
            if lru.len() <= lru.capacity {
                return;
            }
            lru.order.iter().rev().cloned().collect()
        };
        for session_id in candidates {
            if exclude.as_ref() == Some(&session_id) {
                continue;
            }
            if self.is_parent_session_pinned(session_id).await {
                continue;
            }
            self.evict_parent_session_cascade(session_id).await;
            return;
        }
    }

    pub(crate) async fn is_parent_session_pinned(&self, session_id: SessionId) -> bool {
        if self.active_turns.has_session(session_id).await {
            return true;
        }
        let connections = self.connections.lock().await;
        for connection in connections.values() {
            if connection
                .subscriptions
                .iter()
                .any(|subscription| subscription.session_id.as_ref() == Some(&session_id))
            {
                return true;
            }
        }
        false
    }

    pub(crate) async fn evict_parent_session_cascade(&self, parent_session_id: SessionId) {
        let child_session_ids = {
            let registries = self.agent_registries.lock().await;
            registries
                .get(&parent_session_id)
                .map(|registry| registry.children_of(&parent_session_id))
                .unwrap_or_default()
        };

        for child_session_id in child_session_ids {
            if let Some(handle) = self.remove_session_actor(child_session_id).await {
                handle.shutdown().await;
            }
            self.goal_stores.lock().await.remove(&child_session_id);
        }

        if let Some(handle) = self.remove_session_actor(parent_session_id).await {
            handle.shutdown().await;
        }
        self.goal_stores.lock().await.remove(&parent_session_id);
        self.session_lru.lock().await.remove(&parent_session_id);
        self.agent_registries
            .lock()
            .await
            .remove(&parent_session_id);
        self.agent_mailboxes.lock().await.remove(&parent_session_id);
        self.agent_output_buffers
            .lock()
            .await
            .remove(&parent_session_id);
        self.agent_wait_cursors
            .lock()
            .await
            .remove(&parent_session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_lock_allows_same_process_and_reacquires_after_drop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rollout = dir.path().join("ses_locktest.jsonl");
        std::fs::write(&rollout, "").expect("rollout file");

        let first = SessionFileLock::acquire(&rollout).expect("first acquire");
        // The lock file records the holder pid for contention reports.
        let holder_pid = std::fs::read_to_string(session_lock_path(&rollout)).expect("lock file");
        assert_eq!(holder_pid.trim(), std::process::id().to_string());

        // flock is per open-file-description, so a second in-process acquire
        // contends on the lock itself — and the recorded holder pid is our
        // own. Same-process contention deliberately proceeds unlocked
        // (sequential runtimes in one process, evicted-then-reloaded
        // sessions); only a real other process is refused.
        let second = SessionFileLock::acquire(&rollout).expect("same-process acquire proceeds");
        drop(second);
        drop(first);
        let reacquired = SessionFileLock::acquire(&rollout).expect("re-acquire after drop");
        drop(reacquired);
        // Dropping again releases; a third acquire still succeeds.
        SessionFileLock::acquire(&rollout).expect("third acquire after second drop");
    }

    #[test]
    fn parent_session_lru_tracks_recency_for_eviction() {
        let mut lru = ParentSessionLru::new(2);
        let first = SessionId::new();
        let second = SessionId::new();
        let third = SessionId::new();
        lru.touch(first);
        lru.touch(second);
        lru.touch(third);
        assert_eq!(lru.len(), 3);
        lru.touch(second);
        assert_eq!(lru.len(), 3);
    }

    #[test]
    fn parent_session_lru_capacity_allows_exact_capacity() {
        let mut lru = ParentSessionLru::new(PARENT_SESSION_LRU_CAPACITY);
        for _ in 0..PARENT_SESSION_LRU_CAPACITY {
            lru.touch(SessionId::new());
        }
        assert_eq!(lru.len(), PARENT_SESSION_LRU_CAPACITY);
    }
}
