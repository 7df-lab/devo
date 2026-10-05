//! Native `search/*` (connection-local composer reference search).
//!
//! The search is ephemeral and connection-scoped — it deliberately does NOT
//! ride the durable `subscription/*` selector model. These types are the
//! canonical first-party search model.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

/// Shared UUID newtype for connection-local searches.
pub type SearchId = crate::ReferenceSearchId;

// ── search/start ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchStartParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchStartResult {
    pub snapshot: SearchSnapshot,
}

// ── search/update ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchUpdateParams {
    pub search_id: SearchId,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchUpdateResult {
    pub snapshot: SearchSnapshot,
}

// ── search/cancel ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchCancelParams {
    pub search_id: SearchId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchCancelResult {}

// ── shared snapshot types ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchSnapshot {
    pub search_id: SearchId,
    pub query: String,
    pub results: Vec<SearchResult>,
    pub total_file_match_count: usize,
    pub scanned_file_count: usize,
    pub file_search_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub kind: SearchResultKind,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub insert_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mention_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_indices: Option<Vec<usize>>,
    #[serde(default)]
    pub is_disabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum SearchResultKind {
    Skill,
    Mcp,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchFailedPayload {
    pub search_id: SearchId,
    pub query: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    /// Trace: L2-DES-CLIENT-002
    /// Verifies: canonical search snapshots round-trip with camelCase fields.
    #[test]
    fn search_snapshot_roundtrips() {
        let snapshot = SearchSnapshot {
            search_id: SearchId::new(),
            query: "docs".to_string(),
            results: vec![SearchResult {
                kind: SearchResultKind::Mcp,
                display_name: "Docs".to_string(),
                description: Some("docs".to_string()),
                insert_text: "@mcp:docs".to_string(),
                mention_path: Some("mcp://server/docs".to_string()),
                file_path: None,
                match_indices: Some(vec![0, 1, 2]),
                is_disabled: false,
                disabled_reason: None,
            }],
            total_file_match_count: 0,
            scanned_file_count: 0,
            file_search_complete: true,
        };

        let json = serde_json::to_string(&snapshot).expect("serialize");
        assert!(json.contains("\"searchId\""));
        let restored: SearchSnapshot = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(restored, snapshot);
    }
}
