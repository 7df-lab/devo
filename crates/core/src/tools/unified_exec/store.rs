use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use rand::Rng;
use tracing::warn;

use super::process::UnifiedExecProcess;
use super::{MAX_PROCESSES, WARNING_PROCESSES};

struct ProcessEntry {
    process: Arc<UnifiedExecProcess>,
    last_used: std::time::Instant,
}

#[derive(Clone, Copy)]
struct ProcessPruneMeta {
    process_id: i32,
    last_used: std::time::Instant,
    exited: bool,
}

const PROTECTED_RECENT_PROCESSES: usize = 8;

#[derive(Default)]
struct StoreState {
    processes: HashMap<i32, ProcessEntry>,
    reserved_process_ids: HashMap<i32, Arc<()>>,
}

pub struct ProcessStore {
    // All accounting transitions are synchronous and atomic, including cancellation cleanup.
    state: Mutex<StoreState>,
}

/// Owns a capacity slot until insertion or drop, including cancellation across awaits.
pub struct ProcessReservation<'a> {
    store: &'a ProcessStore,
    id: i32,
    token: Arc<()>,
}

impl ProcessReservation<'_> {
    pub fn id(&self) -> i32 {
        self.id
    }

    pub fn insert(self, process: Arc<UnifiedExecProcess>) {
        let mut state = self.store.state.lock().expect("process store lock");
        if !state
            .reserved_process_ids
            .get(&self.id)
            .is_some_and(|token| Arc::ptr_eq(token, &self.token))
        {
            process.terminate();
            return;
        }
        state.reserved_process_ids.remove(&self.id);
        state.processes.insert(
            self.id,
            ProcessEntry {
                process,
                last_used: std::time::Instant::now(),
            },
        );
    }
}

impl Drop for ProcessReservation<'_> {
    fn drop(&mut self) {
        let mut state = self.store.state.lock().expect("process store lock");
        // terminate_all/remove can invalidate a reservation. Never release a newer slot
        // if its random process ID happens to be reused while this guard is still alive.
        if state
            .reserved_process_ids
            .get(&self.id)
            .is_some_and(|token| Arc::ptr_eq(token, &self.token))
        {
            state.reserved_process_ids.remove(&self.id);
        }
    }
}

impl ProcessStore {
    pub fn new() -> Self {
        ProcessStore {
            state: Mutex::new(StoreState::default()),
        }
    }

    pub async fn allocate(&self, process: Arc<UnifiedExecProcess>) -> i32 {
        let Some(reservation) = self.reserve_process_id().await else {
            process.terminate();
            return 0;
        };
        let id = reservation.id();
        reservation.insert(process);
        id
    }

    pub async fn reserve_process_id(&self) -> Option<ProcessReservation<'_>> {
        let mut state = self.state.lock().expect("process store lock");
        let StoreState {
            processes: map,
            reserved_process_ids: reserved,
        } = &mut *state;
        if map.len() + reserved.len() >= MAX_PROCESSES {
            self.prune_process_if_needed(map);
        }
        if map.len() + reserved.len() >= MAX_PROCESSES {
            warn!("max unified exec processes ({MAX_PROCESSES}) reached; cannot allocate process");
            return None;
        }
        if map.len() + reserved.len() >= WARNING_PROCESSES {
            warn!(
                "unified exec processes at {}/{} (warning threshold)",
                map.len() + reserved.len(),
                MAX_PROCESSES
            );
        }
        let id = loop {
            let candidate = rand::rng().random_range(1_000..100_000);
            if !map.contains_key(&candidate) && !reserved.contains_key(&candidate) {
                break candidate;
            }
        };
        let token = Arc::new(());
        reserved.insert(id, Arc::clone(&token));
        Some(ProcessReservation {
            store: self,
            id,
            token,
        })
    }

    pub async fn get(&self, id: i32) -> Option<Arc<UnifiedExecProcess>> {
        let mut state = self.state.lock().expect("process store lock");
        let map = &mut state.processes;
        map.get_mut(&id).map(|entry| {
            entry.last_used = std::time::Instant::now();
            Arc::clone(&entry.process)
        })
    }

    pub async fn remove(&self, id: i32) {
        let mut state = self.state.lock().expect("process store lock");
        state.reserved_process_ids.remove(&id);
        let map = &mut state.processes;
        if let Some(entry) = map.remove(&id) {
            entry.process.terminate();
        }
    }

    pub async fn terminate_all(&self) {
        let processes = {
            let mut state = self.state.lock().expect("process store lock");
            state.reserved_process_ids.clear();
            state
                .processes
                .drain()
                .map(|(_id, entry)| entry.process)
                .collect::<Vec<_>>()
        };
        for process in processes {
            process.terminate();
        }
    }

    pub async fn len(&self) -> usize {
        self.state
            .lock()
            .expect("process store lock")
            .processes
            .len()
    }

    pub async fn is_empty(&self) -> bool {
        self.state
            .lock()
            .expect("process store lock")
            .processes
            .is_empty()
    }

    pub async fn prune_exited(&self) {
        let mut state = self.state.lock().expect("process store lock");
        let map = &mut state.processes;
        self.prune_locked(map);
    }

    fn prune_locked(&self, map: &mut HashMap<i32, ProcessEntry>) {
        let to_remove: Vec<i32> = map
            .iter()
            .filter(|(_, e)| !e.process.is_running())
            .map(|(id, _)| *id)
            .collect();
        for id in to_remove {
            if let Some(entry) = map.remove(&id) {
                entry.process.terminate();
            }
        }
    }

    fn prune_process_if_needed(&self, map: &mut HashMap<i32, ProcessEntry>) {
        // The caller checks combined process and reservation occupancy.
        let meta = map
            .iter()
            .map(|(process_id, entry)| ProcessPruneMeta {
                process_id: *process_id,
                last_used: entry.last_used,
                exited: !entry.process.is_running(),
            })
            .collect::<Vec<_>>();
        if let Some(process_id) = process_id_to_prune_from_meta(&meta)
            && let Some(entry) = map.remove(&process_id)
        {
            entry.process.terminate();
        }
    }
}

fn process_id_to_prune_from_meta(meta: &[ProcessPruneMeta]) -> Option<i32> {
    if meta.is_empty() {
        return None;
    }

    // Keep the most recently touched processes even if they have already exited:
    // callers often poll immediately after command completion to collect output.
    let mut by_recency = meta.iter().collect::<Vec<_>>();
    by_recency.sort_by_key(|entry| Reverse(entry.last_used));
    let protected = by_recency
        .into_iter()
        .take(PROTECTED_RECENT_PROCESSES)
        .map(|entry| entry.process_id)
        .collect::<Vec<_>>();

    let mut lru = meta.iter().collect::<Vec<_>>();
    lru.sort_by_key(|entry| entry.last_used);

    if let Some(entry) = lru
        .iter()
        .find(|entry| !protected.contains(&entry.process_id) && entry.exited)
    {
        return Some(entry.process_id);
    }

    lru.into_iter()
        .find(|entry| !protected.contains(&entry.process_id))
        .map(|entry| entry.process_id)
}

impl Default for ProcessStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unified_exec::process::UnifiedExecProcess;
    use pretty_assertions::assert_eq;
    use std::path::Path;
    use std::time::Duration;

    async fn spawn_echo() -> UnifiedExecProcess {
        let (proc, _rx) = UnifiedExecProcess::spawn(
            1,
            "echo test",
            Path::new("."),
            /*shell*/ None,
            /*login*/ false,
            /*tty*/ false,
        )
        .await
        .expect("spawn should succeed");
        proc
    }

    #[tokio::test]
    async fn reservations_enforce_capacity_and_drop_reopens_slot() {
        let store = ProcessStore::new();
        let mut slots = Vec::new();
        for _ in 0..MAX_PROCESSES {
            slots.push(store.reserve_process_id().await.expect("available slot"));
        }
        assert!(store.reserve_process_id().await.is_none());
        assert_eq!(store.len().await, 0);
        drop(slots.pop());
        slots.push(store.reserve_process_id().await.expect("released slot"));
        assert!(store.reserve_process_id().await.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_reservations_do_not_exceed_capacity() {
        let store = Arc::new(ProcessStore::new());
        let barrier = Arc::new(tokio::sync::Barrier::new(MAX_PROCESSES * 2));
        let mut tasks = Vec::new();
        for _ in 0..MAX_PROCESSES * 2 {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                let reservation = store.reserve_process_id().await;
                // Keep all admitted slots alive until every task has attempted admission.
                barrier.wait().await;
                reservation.as_ref().map(ProcessReservation::id)
            }));
        }
        let mut ids = std::collections::HashSet::new();
        for task in tasks {
            if let Some(id) = task.await.unwrap() {
                assert!(ids.insert(id));
            }
        }
        assert_eq!(ids.len(), MAX_PROCESSES);
        assert!(store.state.lock().unwrap().reserved_process_ids.is_empty());
    }

    #[tokio::test]
    async fn cancelled_reservation_returns_capacity_without_async_cleanup() {
        let store = ProcessStore::new();
        for _ in 0..MAX_PROCESSES + 1 {
            let work = async {
                let _reservation = store.reserve_process_id().await.expect("available slot");
                std::future::pending::<()>().await;
            };
            let mut work = Box::pin(work);
            assert!(futures::poll!(&mut work).is_pending());
            drop(work);
            assert!(store.state.lock().unwrap().reserved_process_ids.is_empty());
        }
    }

    #[tokio::test]
    async fn invalidated_reservation_cannot_insert_or_release_reused_id() {
        let store = ProcessStore::new();
        let reservation = store.reserve_process_id().await.unwrap();
        let id = reservation.id();
        store.terminate_all().await;
        let token = Arc::new(());
        store
            .state
            .lock()
            .unwrap()
            .reserved_process_ids
            .insert(id, Arc::clone(&token));
        let proc = Arc::new(spawn_echo().await);
        reservation.insert(Arc::clone(&proc));
        assert!(store.is_empty().await);
        assert!(!proc.is_running());
        assert!(Arc::ptr_eq(
            store
                .state
                .lock()
                .unwrap()
                .reserved_process_ids
                .get(&id)
                .unwrap(),
            &token
        ));
    }

    #[tokio::test]
    async fn mixed_capacity_prunes_only_outside_recent_processes() {
        let store = ProcessStore::new();
        for _ in 0..PROTECTED_RECENT_PROCESSES + 1 {
            store.allocate(Arc::new(spawn_echo().await)).await;
        }
        let mut slots = Vec::new();
        for _ in PROTECTED_RECENT_PROCESSES + 1..MAX_PROCESSES {
            slots.push(store.reserve_process_id().await.unwrap());
        }
        slots.push(
            store
                .reserve_process_id()
                .await
                .expect("one unprotected process"),
        );
        assert_eq!(store.len().await, PROTECTED_RECENT_PROCESSES);
        assert!(store.reserve_process_id().await.is_none());
    }

    #[tokio::test]
    async fn store_allocate_and_get() {
        let store = ProcessStore::new();
        let proc = spawn_echo().await;
        let id = store.allocate(Arc::new(proc)).await;
        assert!(store.get(id).await.is_some());
        assert!(store.get(9999).await.is_none());
    }

    #[tokio::test]
    async fn store_reserved_insert_preserves_process_id() {
        let store = ProcessStore::new();
        let reservation = store.reserve_process_id().await.expect("reserve id");
        let id = reservation.id();
        let (proc, _rx) = UnifiedExecProcess::spawn(
            id,
            "echo test",
            Path::new("."),
            /*shell*/ None,
            /*login*/ false,
            /*tty*/ false,
        )
        .await
        .expect("spawn should succeed");

        reservation.insert(Arc::new(proc));
        let proc = store.get(id).await.expect("stored process");

        assert_eq!(proc.process_id(), id);
    }

    #[tokio::test]
    async fn store_remove_terminates() {
        let store = ProcessStore::new();
        let proc = spawn_echo().await;
        let id = store.allocate(Arc::new(proc)).await;
        store.remove(id).await;
        assert!(store.get(id).await.is_none());
    }

    #[tokio::test]
    async fn store_terminate_all_drains_processes_and_reservations() {
        let store = ProcessStore::new();
        let reservation = store.reserve_process_id().await.expect("reserve id");
        let proc = Arc::new(spawn_echo().await);
        let id = store.allocate(Arc::clone(&proc)).await;

        store.terminate_all().await;

        assert_eq!(store.len().await, 0);
        assert!(store.get(id).await.is_none());
        assert!(!proc.is_running());
        assert!(store.reserve_process_id().await.is_some());
        drop(reservation);
    }

    #[tokio::test]
    async fn store_len() {
        let store = ProcessStore::new();
        assert_eq!(store.len().await, 0);

        let proc = spawn_echo().await;
        store.allocate(Arc::new(proc)).await;
        assert_eq!(store.len().await, 1);

        let proc = spawn_echo().await;
        store.allocate(Arc::new(proc)).await;
        assert_eq!(store.len().await, 2);
    }

    #[tokio::test]
    async fn store_default_is_empty() {
        let store = ProcessStore::default();
        assert_eq!(store.len().await, 0);
    }

    #[tokio::test]
    async fn store_concurrent_allocate_and_get() {
        let store = Arc::new(ProcessStore::new());
        let mut handles = Vec::new();

        for _i in 0..10 {
            let s = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                let proc = spawn_echo().await;
                let id = s.allocate(Arc::new(proc)).await;
                assert!(s.get(id).await.is_some());
                id
            }));
        }

        let ids: Vec<i32> = futures::future::join_all(handles)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(ids.len(), 10);
        // All IDs should be unique
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 10);
        assert_eq!(store.len().await, 10);
    }

    #[tokio::test]
    async fn store_prune_exited_removes_nothing_for_running() {
        // This test is best-effort since processes might exit quickly
        let store = ProcessStore::new();
        let proc = spawn_echo().await;
        let _id = store.allocate(Arc::new(proc)).await;

        // Don't prune immediately — allow process to potentially still be running
        let count_before = store.len().await;
        store.prune_exited().await;
        let count_after = store.len().await;

        // Process may have exited, but at minimum len should not increase
        assert_eq!(count_before, count_after);
        assert!(count_before >= 1);
    }

    #[test]
    fn process_prune_prefers_exited_lru_outside_protected_recent_set() {
        let now = std::time::Instant::now();
        let meta = (0..MAX_PROCESSES)
            .map(|index| ProcessPruneMeta {
                process_id: index as i32,
                last_used: now + Duration::from_millis(index as u64),
                exited: index == 10 || index == 20,
            })
            .collect::<Vec<_>>();

        assert_eq!(process_id_to_prune_from_meta(&meta), Some(10));
    }

    #[test]
    fn process_prune_protects_recent_exited_processes() {
        let now = std::time::Instant::now();
        let meta = (0..MAX_PROCESSES)
            .map(|index| ProcessPruneMeta {
                process_id: index as i32,
                last_used: now + Duration::from_millis(index as u64),
                exited: index == MAX_PROCESSES - 1,
            })
            .collect::<Vec<_>>();

        assert_eq!(process_id_to_prune_from_meta(&meta), Some(0));
    }
}
