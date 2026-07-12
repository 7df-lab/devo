//! Dedicated background worker for warming workspace code indexes.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

use devo_code_search::{CodeSearchService, ContentFilter};

const WARMUP_QUEUE_CAPACITY: usize = 16;

struct WarmupJob {
    key: String,
    root: PathBuf,
    service: Arc<CodeSearchService>,
}

pub(crate) struct CodeIndexWarmup {
    sender: Mutex<Option<mpsc::SyncSender<WarmupJob>>>,
    queued: Arc<Mutex<HashSet<String>>>,
}

impl CodeIndexWarmup {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel::<WarmupJob>(WARMUP_QUEUE_CAPACITY);
        let queued = Arc::new(Mutex::new(HashSet::new()));
        let worker_queued = Arc::clone(&queued);
        let sender = match std::thread::Builder::new()
            .name("devo-code-index".to_string())
            .spawn(move || run_worker(receiver, worker_queued))
        {
            Ok(_) => Some(sender),
            Err(error) => {
                tracing::warn!(%error, "failed to start code-index warmup worker");
                None
            }
        };
        Self {
            sender: Mutex::new(sender),
            queued,
        }
    }

    pub(crate) fn enqueue(&self, root: PathBuf, service: Arc<CodeSearchService>) {
        let root_key = root.to_string_lossy().into_owned();
        let key = format!("{:p}|{root_key}", Arc::as_ptr(&service));
        let Ok(mut queued) = self.queued.lock() else {
            tracing::warn!("code-index warmup queue lock poisoned");
            return;
        };
        if !queued.insert(key.clone()) {
            return;
        }
        let job = WarmupJob {
            key: key.clone(),
            root,
            service,
        };
        let send_result = self
            .sender
            .lock()
            .ok()
            .and_then(|sender| sender.as_ref().map(|sender| sender.try_send(job)));
        if !matches!(send_result, Some(Ok(()))) {
            queued.remove(&key);
            if matches!(send_result, Some(Err(mpsc::TrySendError::Full(_)))) {
                tracing::debug!(root = %root_key, "code-index warmup queue is full");
            }
        }
    }

    pub(crate) fn shutdown(&self) {
        if let Ok(mut sender) = self.sender.lock() {
            sender.take();
        }
    }

    #[cfg(test)]
    fn queued_len(&self) -> usize {
        self.queued
            .lock()
            .map(|queued| queued.len())
            .unwrap_or_default()
    }
}

fn run_worker(receiver: mpsc::Receiver<WarmupJob>, queued: Arc<Mutex<HashSet<String>>>) {
    while let Ok(job) = receiver.recv() {
        tracing::info!(root = %job.root.display(), "warming code-search index");
        match job.service.prewarm(&job.root, ContentFilter::Code) {
            Ok(stats) => tracing::info!(
                root = %job.root.display(),
                indexed_files = stats.indexed_files,
                total_chunks = stats.total_chunks,
                "code-search index warmup completed"
            ),
            Err(error) => tracing::warn!(
                root = %job.root.display(),
                %error,
                "code-search index warmup failed; first search will retry"
            ),
        }
        if let Ok(mut queued) = queued.lock() {
            queued.remove(&job.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use devo_code_search::{CodeSearchError, EmbeddingProvider, HashEmbeddingProvider};
    use pretty_assertions::assert_eq;

    use super::*;

    struct BlockingProvider {
        inner: HashEmbeddingProvider,
        started: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        calls: Arc<AtomicUsize>,
    }

    impl EmbeddingProvider for BlockingProvider {
        fn model_id(&self) -> &str {
            "blocking-test"
        }

        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CodeSearchError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.send(()).expect("signal embedding start");
            self.release
                .lock()
                .expect("release lock")
                .recv()
                .expect("release embedding");
            self.inner.embed(texts)
        }
    }

    #[test]
    fn warmup_is_nonblocking_and_deduplicates_an_active_workspace() {
        let root = tempfile::tempdir().expect("workspace");
        let cache = tempfile::tempdir().expect("cache");
        std::fs::write(root.path().join("lib.rs"), "pub fn alpha() {}\n").expect("write");
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let service = Arc::new(CodeSearchService::new(
            Arc::new(BlockingProvider {
                inner: HashEmbeddingProvider::new("blocking-test", 16),
                started: started_tx,
                release: Mutex::new(release_rx),
                calls: Arc::clone(&calls),
            }),
            cache.path().to_path_buf(),
        ));
        let warmup = CodeIndexWarmup::new();

        warmup.enqueue(root.path().to_path_buf(), Arc::clone(&service));
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("background embedding started");
        warmup.enqueue(root.path().to_path_buf(), Arc::clone(&service));

        assert_eq!(warmup.queued_len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        warmup.shutdown();
        release_tx.send(()).expect("release embedding");

        let deadline = Instant::now() + Duration::from_secs(5);
        while service.needs_index_build(root.path(), ContentFilter::Code)
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert!(!service.needs_index_build(root.path(), ContentFilter::Code));
    }
}
