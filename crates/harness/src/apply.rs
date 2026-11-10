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
    #[error("invalid refinement: {0}")]
    Invalid(String),
    #[error("refinement history unavailable or already rolled back: {0}")]
    History(String),
}

fn kind_key(kind: HarnessKind) -> &'static str {
    match kind {
        HarnessKind::Prompt => "prompt",
        HarnessKind::Memory => "memory",
        HarnessKind::Skill => "skill",
        HarnessKind::Subagent => "subagent",
    }
}

/// Attach the host-owned planning baseline, never a model-supplied baseline.
pub fn bind_proposal_baseline(
    proposal: &mut RefineProposal,
    baseline: &HarnessState,
) -> Result<(), ApplyError> {
    for edit in &mut proposal.edits {
        edit.before = baseline
            .entries
            .get(kind_key(edit.kind))
            .and_then(|bucket| bucket.get(&edit.id))
            .cloned();
        if matches!(edit.op, RefineEditOp::Create) == edit.before.is_some() {
            return Err(ApplyError::Drift {
                id: edit.id.clone(),
            });
        }
    }
    Ok(())
}

/// Re-read, validate the entire proposal, and commit entries + undo history in
/// one atomic replacement under the shared CRUD/refine lock.
pub fn apply_proposal_re_read(
    path: &Path,
    proposal: &RefineProposal,
) -> Result<HarnessState, ApplyError> {
    crate::store::transact_harness(path, |state| {
        apply_to_state(state, path, proposal, None)?;
        Ok(state.clone())
    })
}

/// Undo a recorded refinement only if all touched entries still match its
/// committed post-image. No partial rollback, legacy-log guessing, or model call.
pub fn rollback_refinement(
    path: &Path,
    target_id: &str,
    rollback_id: &str,
) -> Result<(HarnessState, RefineProposal), ApplyError> {
    crate::store::transact_harness(path, |state| {
        let record = state
            .refinement_history
            .iter()
            .find(|record| record.id == target_id)
            .filter(|record| record.rollback_of.is_none())
            .ok_or_else(|| ApplyError::History(target_id.into()))?;
        if state
            .refinement_history
            .iter()
            .any(|record| record.rollback_of.as_deref() == Some(target_id))
        {
            return Err(ApplyError::History(target_id.into()));
        }
        let proposal = RefineProposal {
            id: rollback_id.into(),
            trigger: "rollback".into(),
            summary: format!("Roll back refinement {target_id}"),
            evidence: format!("Inverse of committed refinement {target_id}"),
            expected_outcome: "Restore prior entry content; effectiveness not evaluated".into(),
            edits: record
                .applied_edits
                .iter()
                .rev()
                .map(|edit| RefineEdit {
                    op: match edit.op {
                        RefineEditOp::Create => RefineEditOp::Delete,
                        RefineEditOp::Update => RefineEditOp::Update,
                        RefineEditOp::Delete => RefineEditOp::Create,
                    },
                    kind: edit.kind,
                    id: edit.id.clone(),
                    before: edit.after.clone(),
                    after: edit.before.clone(),
                })
                .collect(),
        };
        apply_to_state(state, path, &proposal, Some(target_id))?;
        Ok((state.clone(), proposal))
    })
}

fn apply_to_state(
    state: &mut HarnessState,
    path: &Path,
    proposal: &RefineProposal,
    rollback_of: Option<&str>,
) -> Result<(), ApplyError> {
    if proposal.id.trim().is_empty()
        || state
            .refinement_history
            .iter()
            .any(|record| record.id == proposal.id)
    {
        return Err(ApplyError::History(proposal.id.clone()));
    }
    let mut touched = std::collections::HashSet::new();
    let mut committed = proposal.clone();
    let now = Utc::now();
    for edit in &mut committed.edits {
        if edit.id == "base_system_prompt" {
            return Err(ApplyError::ImmutableBasePrompt);
        }
        if edit.id.trim().is_empty() || !touched.insert((edit.kind, edit.id.clone())) {
            return Err(ApplyError::Invalid("empty or duplicate entry id".into()));
        }
        let bucket = state
            .entries
            .entry(kind_key(edit.kind).to_string())
            .or_default();
        let current = bucket.get(&edit.id);
        // Update/Delete require a baseline. Missing is not an unconditional write.
        if current != edit.before.as_ref()
            || (matches!(edit.op, RefineEditOp::Create) != edit.before.is_none())
        {
            return Err(ApplyError::Drift {
                id: edit.id.clone(),
            });
        }
        match edit.op {
            RefineEditOp::Create | RefineEditOp::Update => {
                let after = edit
                    .after
                    .as_mut()
                    .ok_or_else(|| ApplyError::MissingAfter {
                        id: edit.id.clone(),
                    })?;
                if after.id != edit.id || after.kind != edit.kind {
                    return Err(ApplyError::Invalid("entry identity mismatch".into()));
                }
                if let Some(before) = &edit.before {
                    // Consolidation must not silently promote evidence-derived
                    // suggestions into authoritative prompt guidance.
                    if before
                        .metadata
                        .get("trust")
                        .and_then(serde_json::Value::as_str)
                        == Some("unverified_trajectory_lesson")
                    {
                        after.source = before.source.clone();
                        after.metadata = before.metadata.clone();
                    }
                    after.created_at = before.created_at;
                    after.version = before
                        .version
                        .checked_add(1)
                        .ok_or_else(|| ApplyError::Invalid("entry version overflow".into()))?;
                } else if rollback_of.is_some() {
                    after.version = after
                        .version
                        .checked_add(1)
                        .ok_or_else(|| ApplyError::Invalid("entry version overflow".into()))?;
                } else {
                    after.version = 1;
                }
                after.updated_at = now;
                bucket.insert(edit.id.clone(), after.clone());
            }
            RefineEditOp::Delete => {
                if edit.after.is_some() {
                    return Err(ApplyError::Invalid("delete has an after entry".into()));
                }
                bucket.remove(&edit.id);
            }
        }
    }
    state.refinements.push(crate::state::RefinementEvent {
        id: committed.id.clone(),
        trigger: committed.trigger.clone(),
        changes: committed
            .edits
            .iter()
            .map(|e| format!("{:?} {:?}:{}", e.op, e.kind, e.id))
            .collect(),
        evidence: committed.evidence.clone(),
        outcome: committed.expected_outcome.clone(),
        created_at: now,
    });
    let scope = committed
        .edits
        .iter()
        .find_map(|edit| {
            edit.after
                .as_ref()
                .or(edit.before.as_ref())
                .and_then(|entry| entry.scope)
        })
        .map(|scope| match scope {
            crate::HarnessScope::Local => "local",
            crate::HarnessScope::Global => "global",
        });
    let mut record = crate::record_from_proposal(&committed, path, scope);
    record.rollback_of = rollback_of.map(str::to_string);
    state.refinement_history.push(record);
    Ok(())
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
