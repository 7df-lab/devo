//! Host-side apply with re-read-before-apply drift checks.

use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::state::{HarnessEntry, HarnessKind, HarnessState, HarnessStateError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefineEditOp {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefineEdit {
    pub op: RefineEditOp,
    pub kind: HarnessKind,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<HarnessEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<HarnessEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefineProposal {
    pub id: String,
    pub trigger: String,
    pub summary: String,
    pub evidence: String,
    pub expected_outcome: String,
    pub edits: Vec<RefineEdit>,
}

#[derive(Debug, Error)]
pub enum ApplyError {
    #[error(transparent)]
    State(#[from] HarnessStateError),
    #[error("entry changed during refinement planning: {id}")]
    Drift { id: String },
    #[error("blocked edit of immutable base prompt id")]
    ImmutableBasePrompt,
    #[error("missing after entry for create/update of {id}")]
    MissingAfter { id: String },
}

fn kind_key(kind: HarnessKind) -> &'static str {
    match kind {
        HarnessKind::Prompt => "prompt",
        HarnessKind::Memory => "memory",
        HarnessKind::Skill => "skill",
        HarnessKind::Subagent => "subagent",
    }
}

/// Re-read `path`, reject per-entry drift vs the proposal baseline, apply, save.
pub fn apply_proposal_re_read(
    path: &Path,
    proposal: &RefineProposal,
) -> Result<HarnessState, ApplyError> {
    let mut state = HarnessState::load(path)?;
    for edit in &proposal.edits {
        if edit.id == "base_system_prompt" {
            return Err(ApplyError::ImmutableBasePrompt);
        }
        let bucket = state
            .entries
            .entry(kind_key(edit.kind).to_string())
            .or_default();
        let current = bucket.get(&edit.id);
        if let Some(baseline) = &edit.before {
            match current {
                Some(cur) if cur == baseline => {}
                _ => {
                    return Err(ApplyError::Drift {
                        id: edit.id.clone(),
                    });
                }
            }
        } else if current.is_some() && matches!(edit.op, RefineEditOp::Create) {
            return Err(ApplyError::Drift {
                id: edit.id.clone(),
            });
        }
        match edit.op {
            RefineEditOp::Create | RefineEditOp::Update => {
                let after = edit.after.clone().ok_or_else(|| ApplyError::MissingAfter {
                    id: edit.id.clone(),
                })?;
                bucket.insert(edit.id.clone(), after);
            }
            RefineEditOp::Delete => {
                bucket.remove(&edit.id);
            }
        }
    }
    let now = Utc::now();
    state.refinements.push(crate::state::RefinementEvent {
        id: proposal.id.clone(),
        trigger: proposal.trigger.clone(),
        changes: proposal
            .edits
            .iter()
            .map(|e| format!("{:?} {:?}:{}", e.op, e.kind, e.id))
            .collect(),
        evidence: proposal.evidence.clone(),
        outcome: proposal.expected_outcome.clone(),
        created_at: now,
    });
    state.save_atomic(path)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::HarnessScope;
    use pretty_assertions::assert_eq;

    fn sample_entry(id: &str, content: &str) -> HarnessEntry {
        let now = Utc::now();
        HarnessEntry {
            id: id.into(),
            kind: HarnessKind::Memory,
            title: id.into(),
            content: content.into(),
            path: None,
            scope: Some(HarnessScope::Local),
            reference: serde_json::json!({}),
            arguments: serde_json::json!({}),
            metadata: serde_json::json!({}),
            source: "test".into(),
            created_at: now,
            updated_at: now,
            version: 1,
        }
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: apply rejects drift when the on-disk entry changed during planning.
    #[test]
    fn apply_rejects_drift() {
        let dir = tempfile::tempdir().unwrap();
        let path = HarnessState::file_path(dir.path());
        let mut state = HarnessState::default();
        let before = sample_entry("m1", "old");
        state
            .entries
            .get_mut("memory")
            .unwrap()
            .insert("m1".into(), before.clone());
        state.save_atomic(&path).unwrap();

        // Mutate after planning baseline was captured.
        let mut drifted = HarnessState::load(&path).unwrap();
        drifted
            .entries
            .get_mut("memory")
            .unwrap()
            .insert("m1".into(), sample_entry("m1", "changed"));
        drifted.save_atomic(&path).unwrap();

        let proposal = RefineProposal {
            id: "refine_1".into(),
            trigger: "test".into(),
            summary: "upd".into(),
            evidence: "e".into(),
            expected_outcome: "o".into(),
            edits: vec![RefineEdit {
                op: RefineEditOp::Update,
                kind: HarnessKind::Memory,
                id: "m1".into(),
                before: Some(before),
                after: Some(sample_entry("m1", "new")),
            }],
        };
        let err = apply_proposal_re_read(&path, &proposal).expect_err("drift");
        assert!(matches!(err, ApplyError::Drift { .. }));
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: create apply persists the new entry.
    #[test]
    fn apply_create_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = HarnessState::file_path(dir.path());
        HarnessState::default().save_atomic(&path).unwrap();
        let after = sample_entry("m2", "hello");
        let proposal = RefineProposal {
            id: "refine_2".into(),
            trigger: "test".into(),
            summary: "add".into(),
            evidence: "e".into(),
            expected_outcome: "o".into(),
            edits: vec![RefineEdit {
                op: RefineEditOp::Create,
                kind: HarnessKind::Memory,
                id: "m2".into(),
                before: None,
                after: Some(after.clone()),
            }],
        };
        let state = apply_proposal_re_read(&path, &proposal).unwrap();
        assert_eq!(state.entries["memory"]["m2"].content, "hello");
    }
}
