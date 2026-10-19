//! Harness state JSON schema (shared by kernel and host).

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessKind {
    Prompt,
    Memory,
    Skill,
    Subagent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessScope {
    Local,
    Global,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessEntry {
    pub id: String,
    pub kind: HarnessKind,
    pub title: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<HarnessScope>,
    #[serde(default)]
    pub reference: serde_json::Value,
    #[serde(default)]
    pub arguments: serde_json::Value,
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefinementEvent {
    pub id: String,
    pub trigger: String,
    pub changes: Vec<String>,
    pub evidence: String,
    pub outcome: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessState {
    pub schema: u32,
    pub entries: BTreeMap<String, BTreeMap<String, HarnessEntry>>,
    #[serde(default)]
    pub refinements: Vec<RefinementEvent>,
}

#[derive(Debug, Error)]
pub enum HarnessStateError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("corrupt harness_state.json (fail closed): {0}")]
    Corrupt(String),
}

impl Default for HarnessState {
    fn default() -> Self {
        let mut entries = BTreeMap::new();
        for kind in [
            HarnessKind::Prompt,
            HarnessKind::Memory,
            HarnessKind::Skill,
            HarnessKind::Subagent,
        ] {
            entries.insert(kind_key(kind).to_string(), BTreeMap::new());
        }
        Self {
            schema: SCHEMA_VERSION,
            entries,
            refinements: Vec::new(),
        }
    }
}

impl HarnessState {
    /// Merge `global` entries into a copy of `self` for prompt digests.
    ///
    /// Global entries are cross-session guidance; session entries win on id
    /// collision (more specific). Refinement history is not merged — the
    /// digest only formats entries.
    pub fn merged_with_global(&self, global: &HarnessState) -> HarnessState {
        let mut merged = self.clone();
        for (kind, global_bucket) in &global.entries {
            let bucket = merged.entries.entry(kind.clone()).or_default();
            for (id, entry) in global_bucket {
                bucket.entry(id.clone()).or_insert(entry.clone());
            }
        }
        merged
    }

    pub fn load(path: &Path) -> Result<Self, HarnessStateError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let mut file = fs::File::open(path)?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        let value: serde_json::Value = serde_json::from_str(&buf)
            .map_err(|e| HarnessStateError::Corrupt(format!("invalid JSON: {e}")))?;
        if !value.is_object() {
            return Err(HarnessStateError::Corrupt(
                "root must be a JSON object".into(),
            ));
        }
        let state: Self = serde_json::from_value(value)
            .map_err(|e| HarnessStateError::Corrupt(format!("schema parse failed: {e}")))?;
        Ok(state)
    }

    pub fn save_atomic(&self, path: &Path) -> Result<(), HarnessStateError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(self)?;
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&data)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn file_path(session_dir: &Path) -> PathBuf {
        session_dir.join("harness").join("harness_state.json")
    }
}

fn kind_key(kind: HarnessKind) -> &'static str {
    match kind {
        HarnessKind::Prompt => "prompt",
        HarnessKind::Memory => "memory",
        HarnessKind::Skill => "skill",
        HarnessKind::Subagent => "subagent",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: corrupt harness JSON fails closed (no silent empty wipe).
    #[test]
    fn corrupt_json_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("harness_state.json");
        fs::write(&path, b"not-json").unwrap();
        let err = HarnessState::load(&path).expect_err("must fail");
        assert!(matches!(err, HarnessStateError::Corrupt(_)));
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: round-trip save/load preserves entries.
    #[test]
    fn save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = HarnessState::file_path(dir.path());
        let mut state = HarnessState::default();
        let now = Utc::now();
        state.entries.get_mut("memory").unwrap().insert(
            "m1".into(),
            HarnessEntry {
                id: "m1".into(),
                kind: HarnessKind::Memory,
                title: "note".into(),
                content: "hello".into(),
                path: None,
                scope: Some(HarnessScope::Local),
                reference: serde_json::json!({}),
                arguments: serde_json::json!({}),
                metadata: serde_json::json!({}),
                source: "test".into(),
                created_at: now,
                updated_at: now,
                version: 1,
            },
        );
        state.save_atomic(&path).unwrap();
        let loaded = HarnessState::load(&path).unwrap();
        assert_eq!(loaded.entries["memory"]["m1"].content, "hello");
    }
}
