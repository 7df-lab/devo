//! Validated mutations of host-owned harness state.

use std::path::Path;

use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;

use crate::{
    HarnessEntry, HarnessKind, HarnessScope, HarnessState, HarnessStateError, RefinementEvent,
};

#[derive(Debug, Deserialize)]
pub struct EntryInput {
    pub kind: HarnessKind,
    pub id: String,
    pub title: String,
    pub content: String,
    pub path: Option<String>,
    pub reference: Option<serde_json::Map<String, Value>>,
    pub arguments: Option<serde_json::Map<String, Value>>,
    pub metadata: Option<serde_json::Map<String, Value>>,
    #[serde(default = "default_source")]
    pub source: String,
}

fn default_source() -> String {
    "agent".into()
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HarnessCommand {
    Create(EntryInput),
    Update(EntryInput),
    Upsert(EntryInput),
    Delete {
        kind: HarnessKind,
        id: String,
    },
    RecordRefinement {
        id: Option<String>,
        trigger: String,
        changes: Vec<String>,
        #[serde(default)]
        evidence: String,
        #[serde(default)]
        outcome: String,
    },
}

#[derive(Debug, Error)]
pub enum HarnessCommandError {
    #[error(transparent)]
    State(#[from] HarnessStateError),
    #[error("{0}")]
    Invalid(String),
}

fn kind_key(kind: HarnessKind) -> &'static str {
    match kind {
        HarnessKind::Prompt => "prompt",
        HarnessKind::Memory => "memory",
        HarnessKind::Skill => "skill",
        HarnessKind::Subagent => "subagent",
    }
}

impl HarnessCommand {
    /// Apply only to the supplied state. Persistence belongs to the transaction.
    fn apply(
        self,
        state: &mut HarnessState,
        scope: HarnessScope,
    ) -> Result<Value, HarnessCommandError> {
        let (input, requires_existing, create_only) = match self {
            Self::Create(input) => (input, false, true),
            Self::Update(input) => (input, true, false),
            Self::Upsert(input) => (input, false, false),
            Self::Delete { kind, id } => {
                reject_base_prompt(&id)?;
                let deleted = state
                    .entries
                    .entry(kind_key(kind).into())
                    .or_default()
                    .remove(&id)
                    .is_some();
                return Ok(json!({"deleted": deleted}));
            }
            Self::RecordRefinement {
                id,
                trigger,
                changes,
                evidence,
                outcome,
            } => {
                let event = RefinementEvent {
                    id: id.unwrap_or_else(|| format!("refine_{:04}", state.refinements.len() + 1)),
                    trigger,
                    changes,
                    evidence,
                    outcome,
                    created_at: Utc::now(),
                };
                state.refinements.push(event.clone());
                return Ok(json!({"event": event}));
            }
        };
        reject_base_prompt(&input.id)?;
        if input.id.is_empty() {
            return Err(HarnessCommandError::Invalid(
                "entry id must not be empty".into(),
            ));
        }
        let bucket = state
            .entries
            .entry(kind_key(input.kind).into())
            .or_default();
        let existing = bucket.get(&input.id);
        if create_only && existing.is_some() {
            return Err(HarnessCommandError::Invalid(format!(
                "entry {:?} already exists",
                input.id
            )));
        }
        if requires_existing && existing.is_none() {
            return Err(HarnessCommandError::Invalid(format!(
                "entry {:?} does not exist",
                input.id
            )));
        }
        let now = Utc::now();
        let mut entry = existing.cloned().unwrap_or_else(|| HarnessEntry {
            id: input.id.clone(),
            kind: input.kind,
            title: String::new(),
            content: String::new(),
            path: Some("general".into()),
            scope: Some(scope),
            reference: json!({}),
            arguments: json!({}),
            metadata: json!({}),
            source: String::new(),
            created_at: now,
            updated_at: now,
            version: 0,
        });
        entry.title = input.title;
        entry.content = input.content;
        entry.source = input.source;
        entry.scope = Some(scope);
        entry.updated_at = now;
        entry.version = entry
            .version
            .checked_add(1)
            .ok_or_else(|| HarnessCommandError::Invalid("entry version exhausted".into()))?;
        if let Some(path) = input.path {
            entry.path = Some(path);
        }
        if let Some(reference) = input.reference {
            entry.reference = Value::Object(reference);
        }
        if let Some(arguments) = input.arguments {
            entry.arguments = Value::Object(arguments);
        }
        if let Some(metadata) = input.metadata {
            entry.metadata = Value::Object(metadata);
        }
        if entry.kind == HarnessKind::Skill {
            let has_string = |key| {
                entry
                    .reference
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
            };
            if entry.reference.get("type").and_then(Value::as_str) != Some("python")
                || !(has_string("import") || has_string("python_import"))
                || !(has_string("callable") || has_string("call_pattern"))
            {
                return Err(HarnessCommandError::Invalid(
                    "skill entries require a Python reference with import and callable or call_pattern".into(),
                ));
            }
        }
        bucket.insert(input.id, entry.clone());
        Ok(json!({"entry": entry}))
    }
}

fn reject_base_prompt(id: &str) -> Result<(), HarnessCommandError> {
    if id == "base_system_prompt" {
        return Err(HarnessCommandError::Invalid(
            "blocked edit of immutable base prompt id".into(),
        ));
    }
    Ok(())
}

/// Serialize a mutation with all other host CRUD and refinement operations.
pub fn mutate_harness(
    path: &Path,
    scope: HarnessScope,
    command: HarnessCommand,
) -> Result<Value, HarnessCommandError> {
    crate::store::transact_harness(path, |state| {
        let mut result = command.apply(state, scope)?;
        result["state"] = json!(state);
        Ok(result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn command(value: Value) -> HarnessCommand {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn update_preserves_contract_and_rejects_duplicate_create() {
        let dir = tempfile::tempdir().unwrap();
        let path = HarnessState::file_path(dir.path());
        let create = json!({"op":"create", "kind":"skill", "id":"helper", "title":"Helper", "content":"old",
            "path":"tools", "reference":{"type":"python","import":"helper","callable":"run"}, "arguments":{"x":1}});
        mutate_harness(&path, HarnessScope::Local, command(create.clone())).unwrap();
        assert!(mutate_harness(&path, HarnessScope::Local, command(create)).is_err());
        let updated = mutate_harness(
            &path,
            HarnessScope::Local,
            command(json!({
                "op":"update", "kind":"skill", "id":"helper", "title":"Helper 2", "content":"new"
            })),
        )
        .unwrap();
        let mut expected = HarnessState::load(&path).unwrap().entries["skill"]["helper"].clone();
        expected.title = "Helper 2".into();
        expected.content = "new".into();
        expected.path = Some("tools".into());
        expected.reference = json!({"type":"python","import":"helper","callable":"run"});
        expected.arguments = json!({"x":1});
        expected.version = 2;
        assert_eq!(updated["entry"], json!(expected));
    }

    #[test]
    fn invalid_mutations_leave_state_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = HarnessState::file_path(dir.path());
        crate::initialize_harness(&path).unwrap();
        for value in [
            json!({"op":"create","kind":"memory","id":"base_system_prompt","title":"bad","content":"bad"}),
            json!({"op":"delete","kind":"prompt","id":"base_system_prompt"}),
            json!({"op":"update","kind":"memory","id":"absent","title":"bad","content":"bad"}),
            json!({"op":"create","kind":"skill","id":"invalid","title":"bad","content":"bad"}),
        ] {
            assert!(mutate_harness(&path, HarnessScope::Local, command(value)).is_err());
            assert_eq!(HarnessState::load(&path).unwrap(), HarnessState::default());
        }
    }

    #[test]
    fn concurrent_refinement_and_crud_share_one_transaction_boundary() {
        use std::sync::{Arc, Barrier};
        let dir = tempfile::tempdir().unwrap();
        let path = HarnessState::file_path(dir.path());
        let barrier = Arc::new(Barrier::new(12));
        std::thread::scope(|scope| {
            for index in 0..12 {
                let path = &path;
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    let id = format!("entry_{index:02}");
                    barrier.wait();
                    if index % 2 == 0 {
                        mutate_harness(path, HarnessScope::Global, command(json!({
                            "op":"create", "kind":"memory", "id":id, "title":id, "content":"value"
                        }))).unwrap();
                    } else {
                        let now = Utc::now();
                        crate::apply_proposal_re_read(
                            path,
                            &crate::RefineProposal {
                                id: format!("refine_{index}"),
                                trigger: "test".into(),
                                summary: String::new(),
                                evidence: String::new(),
                                expected_outcome: String::new(),
                                edits: vec![crate::RefineEdit {
                                    op: crate::RefineEditOp::Create,
                                    kind: HarnessKind::Memory,
                                    id: id.clone(),
                                    before: None,
                                    after: Some(HarnessEntry {
                                        id,
                                        kind: HarnessKind::Memory,
                                        title: "Refined".into(),
                                        content: "value".into(),
                                        path: None,
                                        scope: Some(HarnessScope::Global),
                                        reference: json!({}),
                                        arguments: json!({}),
                                        metadata: json!({}),
                                        source: "test".into(),
                                        created_at: now,
                                        updated_at: now,
                                        version: 1,
                                    }),
                                }],
                            },
                        )
                        .unwrap();
                    }
                });
            }
        });
        let state = HarnessState::load(&path).unwrap();
        assert_eq!(
            state.entries["memory"].keys().cloned().collect::<Vec<_>>(),
            (0..12)
                .map(|index| format!("entry_{index:02}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(state.refinements.len(), 6);
    }
}
