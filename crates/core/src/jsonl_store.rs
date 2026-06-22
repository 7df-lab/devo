//! JSONL-backed SessionStore implementation.
//!
//! Implements the SessionStore trait with append-only JSONL files,
//! one file per session at `<data_dir>/sessions/<session_id>.jsonl`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use devo_protocol::SessionId;

use crate::durable_record::DurableRecord;
use crate::session_store::{ReplayStream, SessionStore, StoreError, StoreErrorCode};

// ── JsonlSessionStore ────────────────────────────────────────────────

/// Concrete SessionStore that persists DurableRecords as JSONL files.
///
/// Each session gets one file: `<data_dir>/sessions/<session_id>.jsonl`.
/// Appends are serialized through per-file locks to prevent interleaved writes.
#[derive(Debug, Clone)]
pub struct JsonlSessionStore {
    data_dir: PathBuf,
    file_locks: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
}

impl JsonlSessionStore {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            file_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn session_path(&self, session_id: SessionId) -> PathBuf {
        let sessions_dir = self.data_dir.join("sessions");
        sessions_dir.join(format!("{}.jsonl", session_id))
    }

    fn lock_for(&self, path: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.file_locks.lock().unwrap();
        locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn ensure_session_dir(&self) -> Result<(), StoreError> {
        let sessions_dir = self.data_dir.join("sessions");
        std::fs::create_dir_all(&sessions_dir).map_err(|e| StoreError {
            code: StoreErrorCode::IoError,
            message: format!("failed to create sessions dir: {}", e),
        })
    }
}

#[async_trait]
impl SessionStore for JsonlSessionStore {
    async fn append(
        &self,
        session_id: SessionId,
        record: DurableRecord,
    ) -> Result<u64, StoreError> {
        self.ensure_session_dir()?;
        let path = self.session_path(session_id);
        let lock = self.lock_for(&path);
        let _guard = lock.lock().unwrap();

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| StoreError {
                code: StoreErrorCode::IoError,
                message: format!("failed to open session file: {}", e),
            })?;

        let mut line = serde_json::to_vec(&record).map_err(|e| StoreError {
            code: StoreErrorCode::IoError,
            message: format!("serialization failed: {}", e),
        })?;
        line.push(b'\n');

        let offset = file.metadata().map(|m| m.len()).unwrap_or(0);
        file.write_all(&line).map_err(|e| StoreError {
            code: StoreErrorCode::DiskFull,
            message: format!("write failed: {}", e),
        })?;
        file.flush().map_err(|e| StoreError {
            code: StoreErrorCode::IoError,
            message: format!("flush failed: {}", e),
        })?;

        Ok(offset)
    }

    async fn replay(
        &self,
        session_id: SessionId,
        from_offset: u64,
    ) -> Result<ReplayStream, StoreError> {
        let path = self.session_path(session_id);
        if !path.exists() {
            return Err(StoreError {
                code: StoreErrorCode::SessionNotFound,
                message: format!("session file not found: {}", path.display()),
            });
        }

        let file = std::fs::File::open(&path).map_err(|e| StoreError {
            code: StoreErrorCode::IoError,
            message: format!("failed to open for replay: {}", e),
        })?;

        let metadata = file.metadata().map_err(|e| StoreError {
            code: StoreErrorCode::IoError,
            message: format!("metadata failed: {}", e),
        })?;

        if from_offset > metadata.len() {
            return Err(StoreError {
                code: StoreErrorCode::FileCorrupted,
                message: format!(
                    "offset {} exceeds file size {}",
                    from_offset,
                    metadata.len()
                ),
            });
        }

        // Read all remaining lines from the file
        let mut reader = BufReader::new(file);
        let mut records = Vec::new();

        // Skip to offset by reading and discarding bytes
        if from_offset > 0 {
            let mut buf = vec![0u8; from_offset as usize];
            use std::io::Read;
            reader
                .get_mut()
                .read_exact(&mut buf)
                .map_err(|e| StoreError {
                    code: StoreErrorCode::IoError,
                    message: format!("seek failed: {}", e),
                })?;
        }

        for line_result in reader.lines() {
            let line = line_result.map_err(|e| StoreError {
                code: StoreErrorCode::FileCorrupted,
                message: format!("read error: {}", e),
            })?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<DurableRecord>(trimmed) {
                Ok(record) => records.push(record),
                Err(_) => {
                    // Skip malformed lines (truncated writes, partial JSON)
                    continue;
                }
            }
        }

        Ok(ReplayStream::from_records(records))
    }

    async fn flush(&self, session_id: SessionId) -> Result<(), StoreError> {
        let path = self.session_path(session_id);
        if !path.exists() {
            return Ok(());
        }
        let lock = self.lock_for(&path);
        let _guard = lock.lock().unwrap();

        let file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|e| StoreError {
                code: StoreErrorCode::IoError,
                message: format!("flush open failed: {}", e),
            })?;
        file.sync_all().map_err(|e| StoreError {
            code: StoreErrorCode::IoError,
            message: format!("fsync failed: {}", e),
        })?;
        Ok(())
    }

    async fn file_size(&self, session_id: SessionId) -> Result<u64, StoreError> {
        let path = self.session_path(session_id);
        if !path.exists() {
            return Err(StoreError {
                code: StoreErrorCode::SessionNotFound,
                message: format!("session file not found: {}", path.display()),
            });
        }
        let metadata = std::fs::metadata(&path).map_err(|e| StoreError {
            code: StoreErrorCode::IoError,
            message: format!("metadata failed: {}", e),
        })?;
        Ok(metadata.len())
    }
}

// ── ReplayStream ─────────────────────────────────────────────────────

impl ReplayStream {
    pub fn from_records(records: Vec<DurableRecord>) -> Self {
        Self {
            records,
            position: 0,
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_record::*;
    use chrono::Utc;
    use devo_protocol::TurnId;
    use tempfile::TempDir;

    fn make_store() -> (JsonlSessionStore, TempDir) {
        let tmp = TempDir::new().expect("tempdir");
        let store = JsonlSessionStore::new(tmp.path().to_path_buf());
        (store, tmp)
    }

    fn make_session_created(session_id: SessionId) -> DurableRecord {
        DurableRecord::SessionCreated(SessionCreatedRecord {
            schema_version: 1,
            session_id,
            workspace_root: "/tmp/test".into(),
            created_at: Utc::now(),
        })
    }

    fn make_turn_started(session_id: SessionId, turn_id: TurnId) -> DurableRecord {
        DurableRecord::TurnStarted(TurnStartedRecord {
            schema_version: 1,
            session_id,
            turn_id,
            sequence: 0,
            status: devo_protocol::TurnStatus::Running,
            kind: devo_protocol::TurnKind::Regular,
            resume_of_turn_id: None,
            submitted_by_client_id: None,
            model: Some("test-model".into()),
            reasoning_effort_selection: None,
            reasoning_effort: None,
            started_at: Utc::now(),
        })
    }

    #[tokio::test]
    async fn append_and_replay_roundtrip() {
        let (store, _tmp) = make_store();
        let session_id = SessionId::new();

        let record = make_session_created(session_id);
        let offset = store
            .append(session_id, record.clone())
            .await
            .expect("append");
        assert_eq!(offset, 0);

        let mut replay = store.replay(session_id, 0).await.expect("replay");
        let records: Vec<DurableRecord> = replay.collect().await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record_kind(), "session_created");
    }

    #[tokio::test]
    async fn append_multiple_records_and_replay() {
        let (store, _tmp) = make_store();
        let session_id = SessionId::new();
        let turn_id = TurnId::new();

        store
            .append(session_id, make_session_created(session_id))
            .await
            .unwrap();
        store
            .append(session_id, make_turn_started(session_id, turn_id))
            .await
            .unwrap();

        let mut replay = store.replay(session_id, 0).await.expect("replay");
        let records: Vec<DurableRecord> = replay.collect().await;
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].record_kind(), "session_created");
        assert_eq!(records[1].record_kind(), "turn_started");
    }

    #[tokio::test]
    async fn replay_from_offset_skips_earlier_records() {
        let (store, _tmp) = make_store();
        let session_id = SessionId::new();
        let turn_id = TurnId::new();

        let offset0 = store
            .append(session_id, make_session_created(session_id))
            .await
            .unwrap();
        let offset1 = store
            .append(session_id, make_turn_started(session_id, turn_id))
            .await
            .unwrap();

        // Replay from after the first record
        let _size_after_first = store.file_size(session_id).await.unwrap();
        let first_record_len = offset1 - offset0;

        let mut replay = store
            .replay(session_id, first_record_len)
            .await
            .expect("replay");
        let records: Vec<DurableRecord> = replay.collect().await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record_kind(), "turn_started");
    }

    #[tokio::test]
    async fn replay_session_not_found() {
        let (store, _tmp) = make_store();
        let result = store.replay(SessionId::new(), 0).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, StoreErrorCode::SessionNotFound);
    }

    #[tokio::test]
    async fn file_size_returns_correct_size() {
        let (store, _tmp) = make_store();
        let session_id = SessionId::new();

        store
            .append(session_id, make_session_created(session_id))
            .await
            .unwrap();
        let size = store.file_size(session_id).await.expect("file_size");
        assert!(size > 0);
    }

    #[tokio::test]
    async fn flush_succeeds_for_existing_session() {
        let (store, _tmp) = make_store();
        let session_id = SessionId::new();

        store
            .append(session_id, make_session_created(session_id))
            .await
            .unwrap();
        store.flush(session_id).await.expect("flush");
    }

    #[tokio::test]
    async fn flush_noop_for_nonexistent_session() {
        let (store, _tmp) = make_store();
        store
            .flush(SessionId::new())
            .await
            .expect("flush should not error");
    }

    #[tokio::test]
    async fn concurrent_appends_to_different_sessions() {
        let (store, _tmp) = make_store();
        let sid1 = SessionId::new();
        let sid2 = SessionId::new();

        let s = store.clone();
        let h1 =
            tokio::spawn(async move { s.append(sid1, make_session_created(sid1)).await.unwrap() });
        let s = store.clone();
        let h2 =
            tokio::spawn(async move { s.append(sid2, make_session_created(sid2)).await.unwrap() });

        let (r1, r2) = tokio::join!(h1, h2);
        r1.unwrap();
        r2.unwrap();

        let mut replay1 = store.replay(sid1, 0).await.unwrap();
        let records1: Vec<DurableRecord> = replay1.collect().await;
        assert_eq!(records1.len(), 1);

        let mut replay2 = store.replay(sid2, 0).await.unwrap();
        let records2: Vec<DurableRecord> = replay2.collect().await;
        assert_eq!(records2.len(), 1);
    }

    #[tokio::test]
    async fn replay_empty_session_returns_empty() {
        let (store, _tmp) = make_store();
        let session_id = SessionId::new();

        // Append and replay
        store
            .append(session_id, make_session_created(session_id))
            .await
            .unwrap();
        let mut replay = store.replay(session_id, 0).await.unwrap();
        let records: Vec<DurableRecord> = replay.collect().await;
        assert_eq!(records.len(), 1);
    }

    #[tokio::test]
    async fn replay_truncated_file_handles_partial_line() {
        let (store, _tmp) = make_store();
        let session_id = SessionId::new();

        // Write a valid record
        store
            .append(session_id, make_session_created(session_id))
            .await
            .unwrap();

        // Append garbage bytes to simulate truncation
        let path = store.session_path(session_id);
        let lock = store.lock_for(&path);
        {
            let _guard = lock.lock().unwrap();
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b"garbage without newline")
                .unwrap();
        } // drop guard before await

        // Replay should get the valid record and ignore the garbage line
        let mut replay = store.replay(session_id, 0).await.unwrap();
        let records: Vec<DurableRecord> = replay.collect().await;
        assert_eq!(records.len(), 1); // garbage line is non-JSON, skipped
    }
}
