//! Fuzzy search provider trait and session types.
//!
//! Implements L3-BEH-CORE-010. Defines the SearchProvider trait for
//! incremental file search with background indexing, fuzzy matching,
//! cancellation, and result snapshots.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Trait for fuzzy file search providers.
///
/// Implementations use `nucleo` for fuzzy matching with background
/// walker and matcher workers.
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Start a new search session for the given workspace roots.
    async fn start_session(
        &self,
        search_id: SearchId,
        roots: Vec<PathBuf>,
        config: SearchConfig,
    ) -> Result<FileSearchSession, SearchError>;

    /// Update the query for an existing search session.
    async fn update_query(
        &self,
        search_id: SearchId,
        query: String,
    ) -> Result<SearchSnapshot, SearchError>;

    /// Cancel and clean up a search session.
    async fn cancel_session(&self, search_id: SearchId) -> Result<(), SearchError>;
}

/// Unique identifier for a search session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SearchId(pub uuid::Uuid);

impl Default for SearchId {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl std::fmt::Display for SearchId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Configuration for a search session.
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// Maximum number of files to index.
    pub max_indexed_files: usize,
    /// Whether to follow symlinks.
    pub follow_symlinks: bool,
    /// Whether to respect gitignore rules.
    pub respect_gitignore: bool,
    /// Additional exclude patterns.
    pub exclude_patterns: Vec<String>,
    /// Maximum results per snapshot.
    pub max_results: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            max_indexed_files: 100000,
            follow_symlinks: false,
            respect_gitignore: true,
            exclude_patterns: vec![],
            max_results: 20,
        }
    }
}

/// A live file search session with background workers.
#[derive(Debug)]
pub struct FileSearchSession {
    pub search_id: SearchId,
    pub roots: Vec<PathBuf>,
    pub config: SearchConfig,
    pub status: SearchSessionStatus,
}

/// Status of a search session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSessionStatus {
    Indexing,
    Ready,
    Cancelled,
    Error,
}

/// A snapshot of current search results.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchSnapshot {
    pub search_id: SearchId,
    pub query: String,
    pub results: Vec<SearchResult>,
    pub total_indexed: usize,
    pub is_complete: bool,
}

/// A single search result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    /// Workspace-relative path.
    pub path: String,
    /// Match score (0.0 to 1.0).
    pub score: f64,
    /// Match highlight ranges in the path.
    pub highlights: Vec<HighlightRange>,
}

/// Range of characters to highlight in a result.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct HighlightRange {
    pub start: usize,
    pub end: usize,
}

/// Errors from search operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SearchError {
    #[error("search session not found: {0}")]
    SessionNotFound(SearchId),
    #[error("search already active for roots")]
    AlreadyActive,
    #[error("indexing failed: {0}")]
    IndexingFailed(String),
    #[error("search cancelled")]
    Cancelled,
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_id_is_unique() {
        let a = SearchId::new();
        let b = SearchId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn search_id_serde_roundtrip() {
        let id = SearchId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        let restored: SearchId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, id);
    }

    #[test]
    fn default_search_config() {
        let config = SearchConfig::default();
        assert_eq!(config.max_indexed_files, 100000);
        assert!(!config.follow_symlinks);
        assert!(config.respect_gitignore);
        assert_eq!(config.max_results, 20);
    }

    #[test]
    fn search_snapshot_serde_roundtrip() {
        let snapshot = SearchSnapshot {
            search_id: SearchId::new(),
            query: "main.rs".into(),
            results: vec![SearchResult {
                path: "src/main.rs".into(),
                score: 0.95,
                highlights: vec![HighlightRange { start: 4, end: 8 }],
            }],
            total_indexed: 100,
            is_complete: true,
        };
        let json = serde_json::to_string(&snapshot).expect("serialize");
        let restored: SearchSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.results.len(), 1);
        assert_eq!(restored.results[0].path, "src/main.rs");
    }

    #[test]
    fn search_session_status_serde_roundtrip() {
        for status in &[
            SearchSessionStatus::Indexing,
            SearchSessionStatus::Ready,
            SearchSessionStatus::Cancelled,
            SearchSessionStatus::Error,
        ] {
            let json = serde_json::to_string(status).expect("serialize");
            let restored: SearchSessionStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, *status);
        }
    }
}
