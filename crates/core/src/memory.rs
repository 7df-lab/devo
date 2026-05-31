//! Persistent memory extraction and consolidation.
//!
//! Implements L3-BEH-CORE-007. Phase 1 extracts memories from completed
//! sessions into a local state database. Phase 2 consolidates selected
//! memories into the git-backed memory workspace for model read-path injection.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use devo_protocol::SessionId;

// ── Memory Workspace ────────────────────────────────────────────────

/// Root directory for the persistent memory workspace.
#[derive(Debug, Clone)]
pub struct MemoryWorkspace {
    pub root: PathBuf,
}

impl MemoryWorkspace {
    pub fn default_root() -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".devo").join("memories")
    }

    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn memory_summary_path(&self) -> PathBuf {
        self.root.join("memory_summary.md")
    }

    pub fn raw_memories_path(&self) -> PathBuf {
        self.root.join("raw_memories.md")
    }

    pub fn rollout_summaries_dir(&self) -> PathBuf {
        self.root.join("rollout_summaries")
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    pub fn ad_hoc_notes_dir(&self) -> PathBuf {
        self.root.join("extensions").join("ad_hoc").join("notes")
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.rollout_summaries_dir())?;
        std::fs::create_dir_all(self.skills_dir())?;
        std::fs::create_dir_all(self.ad_hoc_notes_dir())?;
        Ok(())
    }
}

// ── Stage 1: Extraction ─────────────────────────────────────────────

/// Output of Phase 1 extraction for a single session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stage1Output {
    pub session_id: SessionId,
    pub source_rollout_path: String,
    pub source_updated_at: DateTime<Utc>,
    pub raw_memory: String,
    pub rollout_summary: String,
    pub rollout_slug: Option<String>,
    pub redaction_state: RedactionState,
    pub usage_count: u64,
    pub last_usage_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub expires_after: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionState {
    Clean,
    Redacted,
    Blocked,
}

// ── Stage 2: Consolidation ──────────────────────────────────────────

/// Configuration for Phase 2 consolidation.
#[derive(Debug, Clone)]
pub struct ConsolidationConfig {
    pub max_input_sessions: usize,
    pub max_raw_memory_bytes: usize,
    pub consolidation_model: Option<String>,
    pub auto_consolidate: bool,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            max_input_sessions: 50,
            max_raw_memory_bytes: 262_144, // 256KB
            consolidation_model: None,
            auto_consolidate: true,
        }
    }
}

/// Result of a Phase 2 consolidation run.
#[derive(Debug, Clone)]
pub struct ConsolidationResult {
    pub sessions_processed: usize,
    pub memories_extracted: usize,
    pub memory_summary_updated: bool,
    pub skills_created: Vec<String>,
    pub skills_updated: Vec<String>,
    pub errors: Vec<String>,
}

// ── Read Path ───────────────────────────────────────────────────────

/// Filter for memory read-path queries.
#[derive(Debug, Clone)]
pub struct MemoryReadFilter {
    pub max_entries: usize,
    pub relevance_keywords: Vec<String>,
    pub exclude_expired: bool,
}

impl Default for MemoryReadFilter {
    fn default() -> Self {
        Self {
            max_entries: 10,
            relevance_keywords: vec![],
            exclude_expired: true,
        }
    }
}

/// Trait for reading and writing persistent memories.
pub trait MemoryStore: Send + Sync {
    /// Read relevant memories for a workspace context.
    fn read_relevant(
        &self,
        workspace_root: &str,
        filter: &MemoryReadFilter,
    ) -> Result<Option<String>, MemoryError>;

    /// Write a Stage 1 extraction output.
    fn write_stage1(&self, output: Stage1Output) -> Result<(), MemoryError>;

    /// List pending sessions that need Stage 1 extraction.
    fn pending_sessions(&self) -> Result<Vec<SessionId>, MemoryError>;

    /// Record an ad-hoc memory note from a user request.
    fn write_ad_hoc_note(
        &self,
        workspace_root: &str,
        slug: &str,
        content: &str,
    ) -> Result<(), MemoryError>;
}

// ── Job Coordination ────────────────────────────────────────────────

/// A memory processing job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryJob {
    pub kind: MemoryJobKind,
    pub job_key: String,
    pub status: JobStatus,
    pub ownership_token: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub lease_until: Option<DateTime<Utc>>,
    pub retry_at: Option<DateTime<Utc>>,
    pub retry_remaining: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryJobKind {
    MemoryStage1,
    MemoryConsolidateGlobal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Running,
    Done,
    Error,
}

// ── Error ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, thiserror::Error)]
pub enum MemoryError {
    #[error("memory workspace not found: {0}")]
    WorkspaceNotFound(String),
    #[error("stage1 extraction failed: {0}")]
    ExtractionFailed(String),
    #[error("consolidation failed: {0}")]
    ConsolidationFailed(String),
    #[error("read error: {0}")]
    ReadError(String),
    #[error("write error: {0}")]
    WriteError(String),
    #[error("job conflict: {0}")]
    JobConflict(String),
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn memory_workspace_creates_dirs() {
        let tmp = TempDir::new().expect("tempdir");
        let ws = MemoryWorkspace::new(tmp.path().join("memories"));
        ws.ensure_dirs().expect("create dirs");
        assert!(ws.rollout_summaries_dir().exists());
        assert!(ws.ad_hoc_notes_dir().exists());
    }

    #[test]
    fn stage1_output_serde_roundtrip() {
        let output = Stage1Output {
            session_id: SessionId::new(),
            source_rollout_path: "/tmp/sessions/abc.jsonl".into(),
            source_updated_at: Utc::now(),
            raw_memory: "User prefers TDD with real databases.".into(),
            rollout_summary: "Set up auth module with integration tests".into(),
            rollout_slug: Some("auth-module-tests".into()),
            redaction_state: RedactionState::Clean,
            usage_count: 0,
            last_usage_at: None,
            created_at: Utc::now(),
            expires_after: None,
        };
        let json = serde_json::to_string(&output).expect("serialize");
        let restored: Stage1Output = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.raw_memory, "User prefers TDD with real databases.");
        assert_eq!(restored.redaction_state, RedactionState::Clean);
    }

    #[test]
    fn memory_job_serde_roundtrip() {
        let job = MemoryJob {
            kind: MemoryJobKind::MemoryStage1,
            job_key: "test-job-key".into(),
            status: JobStatus::Pending,
            ownership_token: None,
            started_at: None,
            lease_until: None,
            retry_at: None,
            retry_remaining: 3,
            last_error: None,
        };
        let json = serde_json::to_string(&job).expect("serialize");
        let restored: MemoryJob = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.kind, MemoryJobKind::MemoryStage1);
        assert_eq!(restored.retry_remaining, 3);
    }

    #[test]
    fn default_consolidation_config() {
        let config = ConsolidationConfig::default();
        assert_eq!(config.max_input_sessions, 50);
        assert_eq!(config.max_raw_memory_bytes, 262_144);
        assert!(config.auto_consolidate);
    }

    #[test]
    fn memory_read_filter_defaults() {
        let filter = MemoryReadFilter::default();
        assert_eq!(filter.max_entries, 10);
        assert!(filter.exclude_expired);
    }
}
