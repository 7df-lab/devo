//! Append-only `refinements.jsonl` undo log (session-local).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::apply::{RefineEdit, RefineProposal};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefinementResultRecord {
    pub id: String,
    pub summary: String,
    pub rationale: String,
    pub expected_outcome: String,
    pub applied_edits: Vec<RefineEdit>,
    pub harness_state_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Debug, Error)]
pub enum RefinementLogError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub fn refinements_log_path(session_dir: &Path) -> PathBuf {
    session_dir.join("harness").join("refinements.jsonl")
}

pub fn kernel_snapshot_path(session_dir: &Path) -> PathBuf {
    session_dir.join("kernel.dill")
}

/// JSON manifest written next to [`kernel_snapshot_path`] (must be a distinct path).
pub fn kernel_snapshot_manifest_path(session_dir: &Path) -> PathBuf {
    session_dir.join("kernel.json")
}

pub fn append_refinement(
    session_dir: &Path,
    record: &RefinementResultRecord,
) -> Result<(), RefinementLogError> {
    let path = refinements_log_path(session_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, record)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

pub fn record_from_proposal(
    proposal: &RefineProposal,
    harness_state_path: &Path,
    scope: Option<&str>,
) -> RefinementResultRecord {
    RefinementResultRecord {
        id: proposal.id.clone(),
        summary: proposal.summary.clone(),
        rationale: proposal.evidence.clone(),
        expected_outcome: proposal.expected_outcome.clone(),
        applied_edits: proposal.edits.clone(),
        harness_state_path: harness_state_path.display().to_string(),
        rollback_of: None,
        scope: scope.map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::{RefineEdit, RefineEditOp};
    use crate::state::HarnessKind;
    use pretty_assertions::assert_eq;

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: refinements.jsonl appends one JSON object per line.
    #[test]
    fn append_writes_jsonl_line() {
        let dir = tempfile::tempdir().unwrap();
        let record = RefinementResultRecord {
            id: "r1".into(),
            summary: "s".into(),
            rationale: "e".into(),
            expected_outcome: "o".into(),
            applied_edits: vec![RefineEdit {
                op: RefineEditOp::Delete,
                kind: HarnessKind::Memory,
                id: "m1".into(),
                before: None,
                after: None,
            }],
            harness_state_path: "harness/harness_state.json".into(),
            rollback_of: None,
            scope: Some("local".into()),
        };
        append_refinement(dir.path(), &record).unwrap();
        let text = std::fs::read_to_string(refinements_log_path(dir.path())).unwrap();
        let line = text.lines().next().unwrap();
        let back: RefinementResultRecord = serde_json::from_str(line).unwrap();
        assert_eq!(back.id, "r1");
    }

    #[test]
    fn kernel_snapshot_paths_are_canonical() {
        let dir = std::path::Path::new("/tmp/artifacts/sess");
        assert_eq!(kernel_snapshot_path(dir), dir.join("kernel.dill"));
        assert_eq!(kernel_snapshot_manifest_path(dir), dir.join("kernel.json"));
    }
}
