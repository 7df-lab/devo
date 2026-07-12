//! Session-scoped ledger of files successfully read by the agent.
//!
//! Used by the `edit` tool to enforce read-before-edit and detect stale
//! contents (mtime or content hash changed since the last recorded read/write).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Why an edit was rejected by the ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileReadFreshnessError {
    /// No successful full-file read (or subsequent write) recorded for this path.
    NotRead,
    /// File mtime or content no longer matches the ledger entry.
    Stale,
}

#[derive(Debug, Clone)]
struct FileReadRecord {
    content_hash: u64,
    mtime: Option<SystemTime>,
}

/// Concurrent map of canonical paths → last known content fingerprint.
#[derive(Debug, Default)]
pub struct FileReadLedger {
    entries: Mutex<HashMap<PathBuf, FileReadRecord>>,
}

impl FileReadLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a successful full-file read.
    pub fn record_full_read(&self, path: &Path, content: &str, mtime: Option<SystemTime>) {
        self.upsert(path, content, mtime);
    }

    /// Records content after a successful mutating write/edit/patch.
    pub fn record_write(&self, path: &Path, content: &str, mtime: Option<SystemTime>) {
        self.upsert(path, content, mtime);
    }

    /// Drops any ledger entry for `path` (e.g. after delete).
    pub fn invalidate(&self, path: &Path) {
        let key = canonicalize_ledger_path(path);
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(&key);
        }
    }

    /// Returns `Ok(())` when the path was read/written this session and still matches.
    pub fn require_fresh(
        &self,
        path: &Path,
        current_content: &str,
        current_mtime: Option<SystemTime>,
    ) -> Result<(), FileReadFreshnessError> {
        let key = canonicalize_ledger_path(path);
        let entries = self
            .entries
            .lock()
            .expect("file read ledger mutex should not be poisoned");
        let Some(record) = entries.get(&key) else {
            return Err(FileReadFreshnessError::NotRead);
        };
        if let (Some(expected), Some(actual)) = (record.mtime, current_mtime)
            && expected != actual
        {
            return Err(FileReadFreshnessError::Stale);
        }
        if record.content_hash != content_hash(current_content) {
            return Err(FileReadFreshnessError::Stale);
        }
        Ok(())
    }

    fn upsert(&self, path: &Path, content: &str, mtime: Option<SystemTime>) {
        let key = canonicalize_ledger_path(path);
        let record = FileReadRecord {
            content_hash: content_hash(content),
            mtime,
        };
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(key, record);
        }
    }
}

fn content_hash(content: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

fn canonicalize_ledger_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn require_fresh_fails_when_never_read() {
        let ledger = FileReadLedger::new();
        let err = ledger
            .require_fresh(Path::new("missing.txt"), "content", None)
            .expect_err("not read");
        assert_eq!(err, FileReadFreshnessError::NotRead);
    }

    #[test]
    fn require_fresh_passes_after_full_read() {
        let ledger = FileReadLedger::new();
        let path = Path::new("file.txt");
        ledger.record_full_read(path, "hello", None);
        assert_eq!(ledger.require_fresh(path, "hello", None), Ok(()));
    }

    #[test]
    fn require_fresh_detects_content_change() {
        let ledger = FileReadLedger::new();
        let path = Path::new("file.txt");
        ledger.record_full_read(path, "hello", None);
        let err = ledger
            .require_fresh(path, "hello!", None)
            .expect_err("stale");
        assert_eq!(err, FileReadFreshnessError::Stale);
    }

    #[test]
    fn write_updates_ledger_for_subsequent_edit() {
        let ledger = FileReadLedger::new();
        let path = Path::new("file.txt");
        ledger.record_full_read(path, "old", None);
        ledger.record_write(path, "new", None);
        assert_eq!(ledger.require_fresh(path, "new", None), Ok(()));
    }

    #[test]
    fn invalidate_removes_entry() {
        let ledger = FileReadLedger::new();
        let path = Path::new("file.txt");
        ledger.record_full_read(path, "hello", None);
        ledger.invalidate(path);
        assert_eq!(
            ledger.require_fresh(path, "hello", None),
            Err(FileReadFreshnessError::NotRead)
        );
    }
}
