use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_ENTRIES: usize = 256;
const MAX_CACHE_BYTES: u64 = 128 * 1024;
const MAX_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const FILE_NAME: &str = "web-probe-cache.json";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum WebProbeEvidence {
    Supported,
    Rejected,
    Inconclusive,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct CacheEntry {
    evidence: WebProbeEvidence,
    expires_at_unix_secs: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct CacheFile {
    version: u8,
    entries: HashMap<String, CacheEntry>,
}

struct PersistenceState {
    entries: HashMap<String, CacheEntry>,
}

/// Durable, bounded cache for terminal hosted-web capability evidence.
///
/// Only hashes of caller-provided opaque keys and terminal classifications are
/// persisted. Cache failures are deliberately best-effort and never block probes.
#[derive(Clone)]
pub(super) struct WebProbePersistence {
    path: PathBuf,
    state: Arc<Mutex<PersistenceState>>,
}

impl WebProbePersistence {
    pub(super) fn open(home: &Path) -> Self {
        let path = home.join(".devo").join(FILE_NAME);
        let entries = load_entries(&path);
        Self {
            path,
            state: Arc::new(Mutex::new(PersistenceState { entries })),
        }
    }

    pub(super) fn get(&self, key: &str) -> Option<WebProbeEvidence> {
        let key = opaque_key(key);
        let now = unix_now();
        let mut state = self.state.lock().ok()?;
        let result = state
            .entries
            .get(&key)
            .filter(|entry| entry.expires_at_unix_secs > now)
            .map(|entry| entry.evidence);
        if result.is_none() {
            state.entries.remove(&key);
        }
        result
    }

    /// Adds or replaces a terminal result and synchronously attempts an atomic
    /// durable write. Callers doing probe work should invoke this in a blocking
    /// task rather than on an async executor thread.
    pub(super) fn put(&self, key: String, evidence: WebProbeEvidence, ttl: Duration) {
        let ttl = ttl.min(MAX_TTL);
        if ttl.is_zero() {
            return;
        }
        let now = unix_now();
        let Some(expires_at_unix_secs) = now.checked_add(ttl.as_secs()) else {
            return;
        };
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state
            .entries
            .retain(|_, entry| entry.expires_at_unix_secs > now);
        state.entries.insert(
            opaque_key(&key),
            CacheEntry {
                evidence,
                expires_at_unix_secs,
            },
        );
        if state.entries.len() > MAX_ENTRIES {
            let mut oldest = state
                .entries
                .iter()
                .map(|(key, entry)| (key.clone(), entry.expires_at_unix_secs))
                .collect::<Vec<_>>();
            oldest.sort_unstable_by_key(|(_, expires_at)| *expires_at);
            for (key, _) in oldest.into_iter().take(state.entries.len() - MAX_ENTRIES) {
                state.entries.remove(&key);
            }
        }
        persist_entries(&self.path, &state.entries);
    }
}

fn opaque_key(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn load_entries(path: &Path) -> HashMap<String, CacheEntry> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return HashMap::new(),
    };
    let mut bytes = Vec::new();
    if file
        .take(MAX_CACHE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > MAX_CACHE_BYTES
    {
        return HashMap::new();
    }
    let Ok(cache) = serde_json::from_slice::<CacheFile>(&bytes) else {
        return HashMap::new();
    };
    if cache.version != 1 {
        return HashMap::new();
    }
    let now = unix_now();
    cache
        .entries
        .into_iter()
        .filter(|(key, entry)| {
            key.len() == 64
                && key.bytes().all(|byte| byte.is_ascii_hexdigit())
                && entry.expires_at_unix_secs > now
        })
        .take(MAX_ENTRIES)
        .collect()
}

fn persist_entries(path: &Path, entries: &HashMap<String, CacheEntry>) {
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let cache = CacheFile {
        version: 1,
        entries: entries.clone(),
    };
    let Ok(bytes) = serde_json::to_vec(&cache) else {
        return;
    };
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_path = path.with_extension(format!("tmp-{}-{unique_suffix}", std::process::id()));
    let write_result = (|| {
        let mut file = File::create(&temp_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp_path, path)?;
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok::<(), std::io::Error>(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(temp_path);
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    use super::{WebProbeEvidence, WebProbePersistence};

    #[test]
    fn cache_survives_reopening_and_does_not_persist_input_key() {
        let home = tempdir().expect("temporary home");
        let key = "connection endpoint and secret should never be written";
        let cache = WebProbePersistence::open(home.path());
        cache.put(
            key.to_string(),
            WebProbeEvidence::Supported,
            Duration::from_secs(60),
        );

        let reopened = WebProbePersistence::open(home.path());
        assert_eq!(reopened.get(key), Some(WebProbeEvidence::Supported));
        let bytes = std::fs::read(home.path().join(".devo/web-probe-cache.json"))
            .expect("cache should be persisted");
        let persisted = String::from_utf8(bytes).expect("cache should be UTF-8");
        assert!(!persisted.contains(key));
        assert!(!persisted.contains("secret"));
    }

    #[test]
    fn expired_entries_are_not_reused_after_reopen() {
        let home = tempdir().expect("temporary home");
        let key = "expiry-key";
        let cache = WebProbePersistence::open(home.path());
        cache.put(
            key.to_string(),
            WebProbeEvidence::Rejected,
            Duration::from_secs(1),
        );
        thread::sleep(Duration::from_millis(1100));

        let reopened = WebProbePersistence::open(home.path());
        assert_eq!(reopened.get(key), None);
    }

    #[test]
    fn unavailable_cache_keeps_in_memory_results() {
        let home = tempdir().expect("temporary home");
        let blocking_file = home.path().join("not-a-directory");
        std::fs::write(&blocking_file, b"file").expect("blocking file fixture");
        let cache = WebProbePersistence::open(&blocking_file);
        cache.put(
            "memory-only".to_string(),
            WebProbeEvidence::Inconclusive,
            Duration::from_secs(60),
        );

        assert_eq!(
            cache.get("memory-only"),
            Some(WebProbeEvidence::Inconclusive)
        );
    }

    #[test]
    fn different_keys_are_invalidated_independently_and_corrupt_files_are_ignored() {
        let home = tempdir().expect("temporary home");
        let cache = WebProbePersistence::open(home.path());
        cache.put(
            "old-binding".to_string(),
            WebProbeEvidence::Supported,
            Duration::from_secs(60),
        );
        assert_eq!(cache.get("new-binding"), None);
        std::fs::write(home.path().join(".devo/web-probe-cache.json"), b"not-json")
            .expect("corrupt cache fixture");

        let reopened = WebProbePersistence::open(home.path());
        assert_eq!(reopened.get("old-binding"), None);
    }
}
