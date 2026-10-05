//! Self-aware compaction host façades (`compact.status` / `compact.run`).
//!
//! These helpers sit behind kernel `host_request` and reuse existing
//! `context/usage/read` occupancy + `session/compact/start` execution. They
//! never run summarization mid-ipython: `compact.run` only sets a pending
//! flag consumed at turn end (before refine). See `docs/rlm-native-api.md`.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use chrono::Utc;
use serde::Serialize;
use serde_json::json;

use devo_protocol::native::ids::SessionId;
use devo_protocol::native::item::ContextOccupancy;

/// Last-write-wins agent-requested compaction keyed by session id (in-memory).
static PENDING_COMPACTS: LazyLock<Mutex<HashMap<String, PendingCompact>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
pub(crate) struct PendingCompact {
    pub instructions: Option<String>,
    #[allow(dead_code)] // telemetry / future digest strip timing
    pub requested_at: chrono::DateTime<Utc>,
}

/// Model-facing `compact.status` payload (wire snake_case keys).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompactStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<f64>,
    pub scheduled: bool,
}

/// Model-facing `compact.run` result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompactRunResult {
    pub scheduled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Build status from a `context/usage/read`-shaped occupancy snapshot.
///
/// When `usage_known` is false (e.g. immediately after compaction, before the
/// next assistant usage), `tokens` / `percent` are omitted/`null` while
/// `context_window` may still be reported from the model limit.
pub fn compact_status_from_occupancy(
    occupancy: Option<&ContextOccupancy>,
    scheduled: bool,
    usage_known: bool,
) -> CompactStatus {
    let context_window = occupancy
        .map(|o| o.context_window_tokens)
        .filter(|&w| w > 0);
    if !usage_known {
        return CompactStatus {
            tokens: None,
            context_window,
            percent: None,
            scheduled,
        };
    }
    let Some(occupancy) = occupancy else {
        return CompactStatus {
            tokens: None,
            context_window: None,
            percent: None,
            scheduled,
        };
    };
    let tokens = Some(occupancy.total_tokens);
    let percent =
        context_window.map(|window| (occupancy.total_tokens as f64 / window as f64) * 100.0);
    CompactStatus {
        tokens,
        context_window,
        percent,
        scheduled,
    }
}

/// JSON object with snake_case keys for `host_reply`.
pub fn compact_status_json(status: &CompactStatus) -> serde_json::Value {
    json!({
        "tokens": status.tokens,
        "context_window": status.context_window,
        "percent": status.percent,
        "scheduled": status.scheduled,
    })
}

pub(crate) fn peek_pending_compact(session_id: &SessionId) -> bool {
    PENDING_COMPACTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(session_id.as_str())
}

pub(crate) fn take_pending_compact(session_id: &SessionId) -> Option<PendingCompact> {
    PENDING_COMPACTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_id.as_str())
}

/// Clear a scheduled agent-requested compact (abort / interrupt path).
#[allow(dead_code)]
pub(crate) fn clear_pending_compact(session_id: &SessionId) {
    let _ = take_pending_compact(session_id);
}

/// Schedule compaction for turn-end. Does not invoke `session/compact/start`.
///
/// `turn_active` must be true (streaming / in-turn). Mid-ipython is fine: this
/// only sets the pending flag; summarization runs after the turn boundary.
pub fn schedule_compact_run(
    session_id: &SessionId,
    instructions: Option<String>,
    turn_active: bool,
) -> CompactRunResult {
    if !turn_active {
        return CompactRunResult {
            scheduled: false,
            note: None,
            reason: Some(
                "no active turn; compaction can only be requested while a turn is running".into(),
            ),
        };
    }
    let pending = PendingCompact {
        instructions,
        requested_at: Utc::now(),
    };
    PENDING_COMPACTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_id.as_str().to_string(), pending);
    CompactRunResult {
        scheduled: true,
        note: Some(
            "Compaction runs when the current turn ends; the turn then stays ended and you are not resumed automatically. Before ending, record the state the summary must carry (plan, handles, file paths, the concrete next step) and tell the user how to pick the work back up."
                .into(),
        ),
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use uuid::Uuid;

    fn unique_session() -> SessionId {
        SessionId::from_string(format!("ses_{}", Uuid::new_v4()))
    }

    /// Trace: L2-DES-CONTEXT-002, L1-REQ-RLM-001
    /// Verifies: compact.status shape exposes tokens, context_window, percent, scheduled.
    #[test]
    fn status_shape_from_occupancy() {
        let occupancy = ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 100_000, /*base*/ 10_000, /*skills*/ 0,
            /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 40_000,
        );
        let status = compact_status_from_occupancy(Some(&occupancy), false, true);
        assert_eq!(
            status,
            CompactStatus {
                tokens: Some(50_000),
                context_window: Some(100_000),
                percent: Some(50.0),
                scheduled: false,
            }
        );
        let wire = compact_status_json(&status);
        assert_eq!(wire["tokens"], 50_000);
        assert_eq!(wire["context_window"], 100_000);
        assert_eq!(wire["percent"], 50.0);
        assert_eq!(wire["scheduled"], false);
    }

    /// Trace: L2-DES-CONTEXT-002
    /// Verifies: after compaction before next usage, tokens/percent are null.
    #[test]
    fn status_nulls_when_usage_unknown() {
        let occupancy = ContextOccupancy::empty(128_000);
        let status = compact_status_from_occupancy(Some(&occupancy), true, false);
        assert_eq!(status.tokens, None);
        assert_eq!(status.percent, None);
        assert_eq!(status.context_window, Some(128_000));
        assert!(status.scheduled);
        let wire = compact_status_json(&status);
        assert!(wire["tokens"].is_null());
        assert!(wire["percent"].is_null());
    }

    /// Trace: L2-DES-CONTEXT-002, L1-REQ-RLM-001
    /// Verifies: compact.run sets pending only while a turn is active.
    #[test]
    fn schedule_requires_active_turn_and_sets_flag() {
        let id = unique_session();
        let idle = schedule_compact_run(&id, None, false);
        assert!(!idle.scheduled);
        assert!(idle.reason.is_some());
        assert!(!peek_pending_compact(&id));

        let active = schedule_compact_run(&id, Some("keep goals".into()), true);
        assert!(active.scheduled);
        assert!(active.note.is_some());
        assert!(peek_pending_compact(&id));

        let taken = take_pending_compact(&id).expect("pending");
        assert_eq!(taken.instructions.as_deref(), Some("keep goals"));
        assert!(!peek_pending_compact(&id));
    }

    /// Trace: L2-DES-CONTEXT-002
    /// Verifies: last-write-wins pending + abort clear.
    #[test]
    fn schedule_last_write_wins_and_clear() {
        let id = unique_session();
        assert!(schedule_compact_run(&id, Some("first".into()), true).scheduled);
        assert!(schedule_compact_run(&id, Some("second".into()), true).scheduled);
        let taken = take_pending_compact(&id).expect("pending");
        assert_eq!(taken.instructions.as_deref(), Some("second"));

        assert!(schedule_compact_run(&id, None, true).scheduled);
        clear_pending_compact(&id);
        assert!(!peek_pending_compact(&id));
    }
}
