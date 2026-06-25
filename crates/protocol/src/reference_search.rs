use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(transparent)]
pub struct ReferenceSearchId(pub uuid::Uuid);

impl Default for ReferenceSearchId {
    fn default() -> Self {
        Self::new()
    }
}

impl ReferenceSearchId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl std::fmt::Display for ReferenceSearchId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReferenceSearchStartParams {
    pub cwd: Option<PathBuf>,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReferenceSearchStartResult {
    pub snapshot: ReferenceSearchSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReferenceSearchUpdateParams {
    pub search_id: ReferenceSearchId,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReferenceSearchUpdateResult {
    pub snapshot: ReferenceSearchSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReferenceSearchCancelParams {
    pub search_id: ReferenceSearchId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, Default)]
pub struct ReferenceSearchCancelResult {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReferenceSearchSnapshot {
    pub search_id: ReferenceSearchId,
    pub query: String,
    pub results: Vec<ReferenceSearchResult>,
    pub total_file_match_count: usize,
    pub scanned_file_count: usize,
    pub file_search_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReferenceSearchResult {
    pub kind: ReferenceSearchResultKind,
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
#[serde(rename_all = "snake_case")]
pub enum ReferenceSearchResultKind {
    Skill,
    Mcp,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReferenceSearchFailedPayload {
    pub search_id: ReferenceSearchId,
    pub query: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn reference_search_snapshot_roundtrips() {
        let snapshot = ReferenceSearchSnapshot {
            search_id: ReferenceSearchId::new(),
            query: "docs".to_string(),
            results: vec![ReferenceSearchResult {
                kind: ReferenceSearchResultKind::Mcp,
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
        let restored: ReferenceSearchSnapshot = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(restored, snapshot);
    }
}
