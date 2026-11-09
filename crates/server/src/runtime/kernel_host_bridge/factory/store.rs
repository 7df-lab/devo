//! Versioned recovery data, not child lifecycle or permission authority.

use std::collections::BTreeSet;
use std::io::{Read, Write};

use anyhow::{Context, Result, ensure};
use devo_protocol::native::ids::SessionId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::files::{Directory, FileMode};

pub(super) const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
const MAX_RUNS: usize = 128;
const MAX_FILE_BYTES: usize = MAX_SNAPSHOT_BYTES + 4096;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredRun {
    store_version: u32,
    owner_session_id: SessionId,
    pub run_id: String,
    pub revision: u64,
    pub snapshot: Value,
}

impl StoredRun {
    pub fn response(&self) -> Value {
        json!({"run_id":self.run_id,"revision":self.revision,"snapshot":self.snapshot})
    }
}

pub(super) fn validate_id(id: &str) -> Result<()> {
    ensure!(
        id.len() == 32
            && id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Factory run_id must contain exactly 32 lowercase hexadecimal characters"
    );
    Ok(())
}

pub(super) fn validate_snapshot(run_id: &str, snapshot: &Value) -> Result<()> {
    ensure!(
        serde_json::to_vec(snapshot)?.len() <= MAX_SNAPSHOT_BYTES,
        "Factory snapshot exceeds 1 MiB"
    );
    ensure!(
        snapshot.is_object() && snapshot["format_version"].as_u64() == Some(1),
        "Factory snapshot format_version must be 1"
    );
    let spec = snapshot
        .get("spec")
        .and_then(Value::as_object)
        .context("Factory snapshot spec must be an object")?;
    ensure!(
        matches!(spec.get("version").and_then(Value::as_u64), Some(1 | 2)),
        "Factory spec version must be 1 or 2"
    );
    let nodes = spec
        .get("nodes")
        .and_then(Value::as_array)
        .context("Factory spec nodes must be an array")?;
    ensure!(
        (1..=128).contains(&nodes.len()),
        "Factory spec must contain 1 to 128 nodes"
    );
    let mut ids = BTreeSet::new();
    for node in nodes {
        let id = node
            .get("id")
            .and_then(Value::as_str)
            .context("Factory spec node id must be a string")?;
        ensure!(
            !id.is_empty() && id.len() <= 32 && ids.insert(id),
            "Factory spec node ids must be unique and bounded"
        );
        ensure!(
            node.get("prompt")
                .and_then(Value::as_str)
                .is_some_and(
                    |prompt| !prompt.trim().is_empty() && prompt.chars().count() <= 128_000
                ),
            "Factory spec node prompt must contain 1 to 128000 characters"
        );
    }
    let run = snapshot
        .get("run")
        .and_then(Value::as_object)
        .context("Factory snapshot run must be an object")?;
    ensure!(
        run.get("id").and_then(Value::as_str) == Some(run_id),
        "Factory snapshot run.id must match run_id"
    );
    ensure!(
        run.get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| !status.is_empty() && status.len() <= 64),
        "Factory run status must be a bounded string"
    );
    ensure!(
        run.get("stop_requested").and_then(Value::as_bool).is_some()
            && run.get("failed").and_then(Value::as_bool).is_some(),
        "Factory run stop_requested and failed must be booleans"
    );
    let states = run
        .get("nodes")
        .and_then(Value::as_object)
        .context("Factory run nodes must be an object")?;
    ensure!(
        states.keys().map(String::as_str).collect::<BTreeSet<_>>() == ids,
        "Factory run nodes must match spec node ids"
    );
    for (id, node) in states {
        ensure!(
            node.is_object() && node.get("id").and_then(Value::as_str) == Some(id),
            "Factory run node id must match its key"
        );
        for key in ["name", "status"] {
            ensure!(
                node.get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty() && text.len() <= 128),
                "Factory node {key} must be a bounded string"
            );
        }
        ensure!(
            node.get("needs_cleanup").and_then(Value::as_bool).is_some(),
            "Factory node needs_cleanup must be a boolean"
        );
        for key in ["child_id", "output", "error", "cleanup_error"] {
            ensure!(
                node.get(key)
                    .is_some_and(|value| value.is_null() || value.is_string()),
                "Factory node {key} must be null or a string"
            );
        }
        ensure!(
            node["child_id"]
                .as_str()
                .is_none_or(|id| !id.trim().is_empty() && id.len() <= 256),
            "Factory child_id must be nonempty and bounded"
        );
    }
    Ok(())
}

fn validate_transition(previous: &Value, next: &Value) -> Result<()> {
    ensure!(
        previous["spec"] == next["spec"],
        "Factory run spec is immutable"
    );
    let previous_nodes = previous["run"]["nodes"]
        .as_object()
        .context("invalid previous nodes")?;
    for (id, previous) in previous_nodes {
        let next = &next["run"]["nodes"][id];
        ensure!(
            previous["name"] == next["name"],
            "Factory node name is immutable: {id}"
        );
        ensure!(
            previous["child_id"].is_null() || previous["child_id"] == next["child_id"],
            "Factory child binding is immutable: {id}"
        );
        ensure!(
            previous["status"] == "pending" || next["status"] != "pending",
            "Factory admitted or settled node cannot return to pending: {id}"
        );
    }
    Ok(())
}

pub(super) fn load(directory: &Directory, owner: SessionId, run_id: &str) -> Result<StoredRun> {
    let file = directory
        .open_file(&format!("{run_id}.json"), FileMode::Read)
        .with_context(|| format!("Factory run not found or unreadable: {run_id}"))?;
    let mut bytes = Vec::new();
    file.take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_FILE_BYTES,
        "Factory run file exceeds size limit"
    );
    let record: StoredRun =
        serde_json::from_slice(&bytes).context("corrupt Factory run file (fail closed)")?;
    ensure!(
        record.store_version == 1
            && record.owner_session_id == owner
            && record.run_id == run_id
            && record.revision > 0,
        "corrupt Factory run envelope or session ownership mismatch"
    );
    validate_snapshot(run_id, &record.snapshot).context("corrupt Factory run snapshot")?;
    Ok(record)
}

pub(super) fn list(directory: &Directory, owner: SessionId) -> Result<Value> {
    let mut runs = Vec::new();
    for name in directory.entries()? {
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        validate_id(id).context("invalid Factory run filename")?;
        ensure!(runs.len() < MAX_RUNS, "Factory stored run limit exceeded");
        let run = load(directory, owner, id)?;
        runs.push(json!({"run_id":run.run_id,"revision":run.revision,"status":run.snapshot["run"]["status"]}));
    }
    runs.sort_by(|a, b| a["run_id"].as_str().cmp(&b["run_id"].as_str()));
    Ok(json!({"runs":runs}))
}

pub(super) fn save(
    directory: &Directory,
    owner: SessionId,
    run_id: String,
    expected_revision: u64,
    snapshot: Value,
) -> Result<Value> {
    validate_id(&run_id)?;
    validate_snapshot(&run_id, &snapshot)?;
    let lock = directory.open_file("store.lock", FileMode::Lock)?;
    fs2::FileExt::lock_exclusive(&lock)?;
    save_locked(directory, owner, run_id, expected_revision, snapshot)
}

/// A host effect holds the same lock as checkpoint/claim writes until child
/// admission or close returns. No runtime actor/map lock is held by this gate.
pub(super) struct EffectTransaction {
    directory: Directory,
    _lock: std::fs::File,
}

impl EffectTransaction {
    pub fn open(directory: Directory) -> Result<Self> {
        let lock = directory.open_file("store.lock", FileMode::Lock)?;
        fs2::FileExt::lock_exclusive(&lock)?;
        Ok(Self {
            directory,
            _lock: lock,
        })
    }

    pub fn load(&self, owner: SessionId, run_id: &str) -> Result<StoredRun> {
        load(&self.directory, owner, run_id)
    }

    pub fn reserve(&self, owner: SessionId, run: StoredRun) -> Result<Value> {
        save_locked(
            &self.directory,
            owner,
            run.run_id,
            run.revision,
            run.snapshot,
        )
    }
}

fn save_locked(
    directory: &Directory,
    owner: SessionId,
    run_id: String,
    expected_revision: u64,
    snapshot: Value,
) -> Result<Value> {
    // Caller owns the stable sidecar lock, not the replaceable destination inode.
    let filename = format!("{run_id}.json");
    let current = match directory.open_file(&filename, FileMode::Read) {
        Ok(_) => Some(load(directory, owner, &run_id)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let current_revision = current.as_ref().map_or(0, |run| run.revision);
    ensure!(
        current_revision == expected_revision,
        "Factory revision conflict: expected {expected_revision}, found {current_revision}"
    );
    if let Some(current) = &current {
        validate_transition(&current.snapshot, &snapshot)?;
    }
    if current_revision == 0 {
        let count = list(directory, owner)?["runs"]
            .as_array()
            .expect("list returns array")
            .len();
        ensure!(
            count < MAX_RUNS,
            "Factory session run limit reached ({MAX_RUNS})"
        );
    }
    let revision = current_revision
        .checked_add(1)
        .context("Factory revision exhausted")?;
    let record = StoredRun {
        store_version: 1,
        owner_session_id: owner,
        run_id,
        revision,
        snapshot,
    };
    let temporary_name = format!(".{}.tmp", uuid::Uuid::new_v4().simple());
    let result = (|| {
        let mut temporary = directory.open_file(&temporary_name, FileMode::Create)?;
        serde_json::to_writer(&mut temporary, &record)?;
        temporary.flush()?;
        temporary.sync_all()?;
        drop(temporary);
        directory.replace(&temporary_name, &filename)?;
        Ok::<_, anyhow::Error>(record.response())
    })();
    if result.is_err() {
        let _ = directory.remove(&temporary_name);
    }
    // A rename/fsync error has an uncertain commit outcome. Callers must reload,
    // never silently retry the expected revision or assume the old file survived.
    result
}

#[cfg(test)]
mod tests;
