//! Bounded, version-checked projections of rollout history for paged Native reads.
//!
//! Concurrent first-page requests for the same session share one parse. Later
//! pages reuse the projection while the rollout file version remains stable.
//! Appends, rollbacks, and other durable writes change the file stamp and force
//! a fresh read; failed reads are never cached.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::SystemTime;

use anyhow::Context;
use devo_core::{CanonicalHistory, read_canonical_history};
use devo_protocol::native::ids::SessionId;
use tokio::sync::Mutex;

const MAX_CACHED_HISTORY_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TOTAL_CACHE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileVersion {
    len: u64,
    modified: SystemTime,
}

struct CacheEntry {
    path: PathBuf,
    version: FileVersion,
    history: Arc<CanonicalHistory>,
    last_used: u64,
}

#[derive(Default)]
struct CacheState {
    entries: HashMap<SessionId, CacheEntry>,
    in_flight: HashMap<SessionId, Weak<Mutex<()>>>,
    total_bytes: u64,
    clock: u64,
}

/// Shares canonical history reads while bounding retained rollout projections.
#[derive(Default)]
pub(crate) struct HistoryCache {
    state: Mutex<CacheState>,
    #[cfg(test)]
    parse_count: std::sync::atomic::AtomicUsize,
}

impl HistoryCache {
    pub(crate) async fn load(
        &self,
        session_id: SessionId,
        path: PathBuf,
    ) -> anyhow::Result<Arc<CanonicalHistory>> {
        let version = file_version(&path)?;
        if let Some(version) = version
            && let Some(history) = self.cached(session_id, &path, version).await
        {
            return Ok(history);
        }

        let flight = self.load_gate(session_id).await;
        let _guard = flight.lock().await;
        let version = file_version(&path)?;
        if let Some(version) = version
            && let Some(history) = self.cached(session_id, &path, version).await
        {
            return Ok(history);
        }

        #[cfg(test)]
        self.parse_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let read_path = path.clone();
        let history = tokio::task::spawn_blocking(move || read_canonical_history(&read_path))
            .await
            .context("join canonical history read")??;
        let history = Arc::new(history);

        if let Some(version) = version
            && version.len <= MAX_CACHED_HISTORY_BYTES
            && file_version(&path).is_ok_and(|after| after == Some(version))
        {
            self.insert(session_id, path, version, Arc::clone(&history))
                .await;
        }
        Ok(history)
    }

    async fn cached(
        &self,
        session_id: SessionId,
        path: &Path,
        version: FileVersion,
    ) -> Option<Arc<CanonicalHistory>> {
        let mut state = self.state.lock().await;
        state.clock = state.clock.wrapping_add(1);
        let tick = state.clock;
        let entry = state.entries.get_mut(&session_id)?;
        if entry.path.as_path() != path || entry.version != version {
            return None;
        }
        entry.last_used = tick;
        Some(Arc::clone(&entry.history))
    }

    async fn load_gate(&self, session_id: SessionId) -> Arc<Mutex<()>> {
        let mut state = self.state.lock().await;
        state.in_flight.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = state.in_flight.get(&session_id).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        state.in_flight.insert(session_id, Arc::downgrade(&gate));
        gate
    }

    async fn insert(
        &self,
        session_id: SessionId,
        path: PathBuf,
        version: FileVersion,
        history: Arc<CanonicalHistory>,
    ) {
        let mut state = self.state.lock().await;
        remove_entry(&mut state, session_id);
        while !state.entries.is_empty()
            && (state.entries.len() >= MAX_CACHE_ENTRIES
                || state.total_bytes.saturating_add(version.len) > MAX_TOTAL_CACHE_BYTES)
        {
            let Some(oldest) = state
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(id, _)| *id)
            else {
                break;
            };
            remove_entry(&mut state, oldest);
        }
        state.clock = state.clock.wrapping_add(1);
        let last_used = state.clock;
        state.total_bytes = state.total_bytes.saturating_add(version.len);
        state.entries.insert(
            session_id,
            CacheEntry {
                path,
                version,
                history,
                last_used,
            },
        );
    }

    #[cfg(test)]
    fn parse_count(&self) -> usize {
        self.parse_count.load(std::sync::atomic::Ordering::Relaxed)
    }
}

fn file_version(path: &Path) -> anyhow::Result<Option<FileVersion>> {
    let metadata = std::fs::metadata(path).context("stat canonical history")?;
    Ok(metadata.modified().ok().map(|modified| FileVersion {
        len: metadata.len(),
        modified,
    }))
}

fn remove_entry(state: &mut CacheState, session_id: SessionId) {
    if let Some(entry) = state.entries.remove(&session_id) {
        state.total_bytes = state.total_bytes.saturating_sub(entry.version.len);
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::sync::Arc;

    use devo_core::CanonicalHistory;
    use devo_protocol::native::ids::SessionId;
    use pretty_assertions::assert_eq;
    use tokio::sync::Barrier;

    use super::HistoryCache;

    #[tokio::test]
    async fn reuses_stable_history_and_invalidates_after_append() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("session.jsonl");
        fs::write(&path, b"").expect("empty rollout");
        let cache = HistoryCache::default();
        let session_id = SessionId::new();

        let first = cache
            .load(session_id, path.clone())
            .await
            .expect("initial history");
        let second = cache
            .load(session_id, path.clone())
            .await
            .expect("cached history");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.as_ref(), &CanonicalHistory::default());

        OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open rollout")
            .write_all(b"\n")
            .expect("append blank line");
        let third = cache
            .load(session_id, path)
            .await
            .expect("history after append");
        assert!(!Arc::ptr_eq(&first, &third));
        assert_eq!(third.as_ref(), &CanonicalHistory::default());
        assert_eq!(cache.parse_count(), 2);
    }

    #[tokio::test]
    async fn concurrent_initial_reads_share_one_parse() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("session.jsonl");
        fs::write(&path, b"").expect("empty rollout");
        let cache = Arc::new(HistoryCache::default());
        let session_id = SessionId::new();
        let barrier = Arc::new(Barrier::new(3));
        let mut readers = Vec::new();
        for _ in 0..2 {
            let cache = Arc::clone(&cache);
            let barrier = Arc::clone(&barrier);
            let path = path.clone();
            readers.push(tokio::spawn(async move {
                barrier.wait().await;
                cache.load(session_id, path).await.expect("history read")
            }));
        }
        barrier.wait().await;
        let first = readers.remove(0).await.expect("first reader");
        let second = readers.remove(0).await.expect("second reader");

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.parse_count(), 1);
    }

    #[tokio::test]
    async fn damaged_history_is_not_cached() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("session.jsonl");
        fs::write(&path, b"not a rollout line\n").expect("damaged rollout");
        let cache = HistoryCache::default();
        let session_id = SessionId::new();

        assert!(cache.load(session_id, path.clone()).await.is_err());
        fs::write(&path, b"").expect("repair rollout");
        let history = cache
            .load(session_id, path)
            .await
            .expect("read repaired history");
        assert_eq!(history.as_ref(), &CanonicalHistory::default());
        assert_eq!(cache.parse_count(), 2);
    }
}
