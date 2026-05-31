use async_trait::async_trait;
use devo_protocol::SessionId;
use serde::{Deserialize, Serialize};

use crate::durable_record::DurableRecord;

/// The persistence contract between `server` (caller) and `core` (implementation).
///
/// Server calls these methods to persist and replay session data.
/// Core owns all persistence decisions: file layout, buffering, flush policy.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Append one durable record. Blocks until the record is durable (fsync'd or batched).
    async fn append(&self, session_id: SessionId, record: DurableRecord)
    -> Result<u64, StoreError>;

    /// Replay all records for a session, optionally from a byte offset.
    async fn replay(
        &self,
        session_id: SessionId,
        from_offset: u64,
    ) -> Result<ReplayStream, StoreError>;

    /// Flush any buffered appends to disk.
    async fn flush(&self, session_id: SessionId) -> Result<(), StoreError>;

    /// Return the current file size in bytes (for offset tracking).
    async fn file_size(&self, session_id: SessionId) -> Result<u64, StoreError>;
}

/// A stream of replayed durable records.
#[derive(Debug)]
pub struct ReplayStream {
    pub(crate) records: Vec<crate::durable_record::DurableRecord>,
    pub(crate) position: usize,
}

impl ReplayStream {
    pub async fn collect(&mut self) -> Vec<crate::durable_record::DurableRecord> {
        let remaining = self.records.split_off(self.position);
        self.position = self.records.len();
        remaining
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct StoreError {
    pub code: StoreErrorCode,
    pub message: String,
}

impl StoreError {
    pub fn new(code: StoreErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreErrorCode {
    SessionNotFound,
    FileCorrupted,
    DiskFull,
    PermissionDenied,
    IoError,
}

impl std::fmt::Display for StoreErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotFound => write!(f, "session_not_found"),
            Self::FileCorrupted => write!(f, "file_corrupted"),
            Self::DiskFull => write!(f, "disk_full"),
            Self::PermissionDenied => write!(f, "permission_denied"),
            Self::IoError => write!(f, "io_error"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_error_roundtrip() {
        let err = StoreError::new(StoreErrorCode::DiskFull, "no space left");
        let json = serde_json::to_string(&err).expect("serialize");
        let restored: StoreError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.code, StoreErrorCode::DiskFull);
        assert_eq!(restored.message, "no space left");
    }

    #[test]
    fn store_error_code_display() {
        assert_eq!(
            StoreErrorCode::SessionNotFound.to_string(),
            "session_not_found"
        );
        assert_eq!(StoreErrorCode::FileCorrupted.to_string(), "file_corrupted");
        assert_eq!(
            StoreErrorCode::PermissionDenied.to_string(),
            "permission_denied"
        );
    }

    #[test]
    fn store_error_code_serde_roundtrip() {
        let codes = [
            StoreErrorCode::SessionNotFound,
            StoreErrorCode::FileCorrupted,
            StoreErrorCode::DiskFull,
            StoreErrorCode::PermissionDenied,
            StoreErrorCode::IoError,
        ];
        for code in &codes {
            let json = serde_json::to_string(code).expect("serialize");
            let restored: StoreErrorCode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *code);
        }
    }
}
