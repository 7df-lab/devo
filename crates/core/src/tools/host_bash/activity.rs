//! Read-only snapshots for host-owned Python bash handles. Independent of the
//! control-handle quota so a client can inspect completion after Python forgets it.
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use super::{Entry, HANDLES, RETENTION};

const COMPLETED_LIMIT: usize = 64;
const OUTPUT_LIMIT: usize = 16 * 1024;

/// A session-owned host shell observation, not a process-control capability.
/// Host bash is non-persistent; its opaque handle is never an OS PID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostBashActivity {
    pub handle_id: String,
    pub session_id: String,
    pub command: String,
    pub turn_id: Option<devo_protocol::TurnId>,
    pub cancelled: bool,
    pub cwd: PathBuf,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub running: bool,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
    pub output: String,
}

static COMPLETED: LazyLock<Mutex<VecDeque<(Instant, HostBashActivity)>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

impl HostBashActivity {
    fn from_entry(handle_id: &str, entry: &Entry) -> Self {
        let elapsed = entry
            .finished
            .unwrap_or_else(Instant::now)
            .duration_since(entry.started);
        let tail = entry.tail.snapshot(OUTPUT_LIMIT);
        Self {
            handle_id: handle_id.to_owned(),
            session_id: entry.session_id.clone(),
            command: entry.command.clone(),
            turn_id: entry.turn_id,
            cancelled: entry.cancelled,
            cwd: entry.cwd.clone(),
            created_at: entry.created_at,
            updated_at: entry.created_at + chrono::Duration::from_std(elapsed).unwrap_or_default(),
            running: entry.finished.is_none(),
            exit_code: entry.exit_code,
            error: entry.error.as_deref().map(bounded_tail),
            output: bounded_tail(if tail.is_empty() || entry.error.is_some() {
                &entry.output
            } else {
                &tail
            }),
        }
    }
}

fn bounded_tail(text: &str) -> String {
    let mut start = text.len().saturating_sub(OUTPUT_LIMIT);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}

pub(super) fn retain_completed(handle_id: &str, entry: &Entry) {
    let Some(finished) = entry.finished else {
        return;
    };
    let mut completed = COMPLETED.lock().unwrap();
    completed.retain(|(at, snapshot)| at.elapsed() < RETENTION && snapshot.handle_id != handle_id);
    completed.push_back((finished, HostBashActivity::from_entry(handle_id, entry)));
    while completed.len() > COMPLETED_LIMIT {
        completed.pop_front();
    }
}

pub(super) fn clear_session(session_id: &str) {
    COMPLETED
        .lock()
        .unwrap()
        .retain(|(_, snapshot)| snapshot.session_id != session_id);
}

/// Snapshot current and retained host bash work owned by exactly this session.
/// Does not execute Python, wake a kernel, or mutate command lifetime.
pub fn session_activities(session_id: &str) -> Vec<HostBashActivity> {
    // Keep lock order identical to forget: handles then completed.
    let handles = HANDLES.lock().unwrap();
    let mut snapshots: Vec<_> = handles
        .iter()
        .filter_map(|(id, entry)| {
            let entry = entry.lock().unwrap();
            if entry.session_id == session_id {
                Some(HostBashActivity::from_entry(id, &entry))
            } else {
                None
            }
        })
        .collect();
    let mut completed = COMPLETED.lock().unwrap();
    completed.retain(|(at, _)| at.elapsed() < RETENTION);
    snapshots.extend(
        completed
            .iter()
            .filter(|(_, snapshot)| snapshot.session_id == session_id)
            .map(|(_, snapshot)| snapshot.clone()),
    );
    snapshots.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.handle_id.cmp(&b.handle_id))
    });
    snapshots
}

/// Resolve one opaque activity handle for Native task/read. Callers expose it
/// read-only; stdin, resizing and process interruption remain separate APIs.
pub fn activity_snapshot(handle_id: &str) -> Option<HostBashActivity> {
    let handles = HANDLES.lock().unwrap();
    if let Some(entry) = handles.get(handle_id) {
        return Some(HostBashActivity::from_entry(
            handle_id,
            &entry.lock().unwrap(),
        ));
    }
    let mut completed = COMPLETED.lock().unwrap();
    completed.retain(|(at, _)| at.elapsed() < RETENTION);
    completed
        .iter()
        .find(|(_, snapshot)| snapshot.handle_id == handle_id)
        .map(|(_, snapshot)| snapshot.clone())
}

#[cfg(test)]
mod tests;
