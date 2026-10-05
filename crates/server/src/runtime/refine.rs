//! Continual Harness refine host entry (`session/refine/run`).
//!
//! Mid-turn / mid-ipython only **schedules** (never applies). Apply runs in the
//! post-`MergeTurn` pipeline via [`apply_pending_refine_at_boundary`] using
//! `apply_proposal_re_read`. Planning prefers LLM `UsagePurpose::Refine` with
//! heuristic fallback ([`plan_refine_proposal`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use super::*;
use devo_harness::{
    AutoRefineSettings, HarnessEntry, HarnessKind, HarnessScope, HarnessState, RefineEdit,
    RefineEditOp, RefineProposal, append_refinement, apply_proposal_re_read, record_from_proposal,
    should_auto_refine,
};
use devo_protocol::native::item::{Item, ItemEnvelope, ItemState};
use devo_protocol::native::rpc_session::{SessionRefineRunParams, SessionRefineRunResult};

/// Last-write-wins pending refine requests keyed by session id (in-memory).
static PENDING_REFINES: LazyLock<Mutex<HashMap<String, PendingRefine>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Successful user-visible assistant turns since last auto-refine (root only).
static AUTO_REFINE_TURN_COUNTS: LazyLock<Mutex<HashMap<String, u32>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Instructions value stamped on interval-scheduled refines.
///
/// This is a scheduling **sentinel** (it doubles as the durable `trigger`
/// label on refinement records), never guidance: both planners must map it
/// away before any model sees it, or the sentinel itself gets stored as a
/// memory.
pub(crate) const AUTO_REFINE_INSTRUCTION: &str = "auto-interval";

#[derive(Debug, Clone)]
#[allow(dead_code)] // global / requested_at reserved for planner + telemetry
pub(crate) struct PendingRefine {
    pub proposal_id: String,
    pub instructions: Option<String>,
    pub global: bool,
    pub rollback_id: Option<String>,
    pub requested_at: chrono::DateTime<Utc>,
    /// True when scheduled by root auto-interval (Plan Mode skips apply).
    pub autonomous: bool,
}

/// Ensure the session harness directory exists with a valid (or empty) state file.
pub(crate) fn ensure_session_harness(session_dir: &Path) -> Result<PathBuf, String> {
    let path = HarnessState::file_path(session_dir);
    if path.exists() {
        HarnessState::load(&path).map_err(|e| e.to_string())?;
        return Ok(path);
    }
    let state = HarnessState::default();
    state.save_atomic(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Harness scope a refine applies to, from the request's `global_` flag.
pub(crate) fn refine_target_scope(pending: &PendingRefine) -> HarnessScope {
    if pending.global {
        HarnessScope::Global
    } else {
        HarnessScope::Local
    }
}

/// Scope label for refinement records (`record_from_proposal`).
pub(crate) fn refine_scope_label(pending: &PendingRefine) -> &'static str {
    if pending.global { "global" } else { "local" }
}

/// Harness state path a refine applies to: the session-local store, or — for
/// `global_=True` requests — the global continual-harness store under the
/// Devo home (`<home>/harness/harness_state.json`, the same directory the
/// kernel env `RLM_GLOBAL_HARNESS_STATE_DIR` and the kernel fence grant point
/// at). Global refinements used to be applied to the session-local store
/// because nothing consumed `PendingRefine::global`.
pub(crate) fn refine_target_harness_path(
    session_dir: &Path,
    pending: &PendingRefine,
) -> Result<PathBuf, String> {
    if !pending.global {
        return ensure_session_harness(session_dir);
    }
    let home = devo_util_paths::find_devo_home().map_err(|e| e.to_string())?;
    ensure_session_harness(&home)
}

pub(crate) fn take_pending_refine(
    session_id: &devo_protocol::native::ids::SessionId,
) -> Option<PendingRefine> {
    PENDING_REFINES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_id.as_str())
}

pub(crate) fn peek_pending_refine(session_id: &devo_protocol::native::ids::SessionId) -> bool {
    PENDING_REFINES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(session_id.as_str())
}

/// Abort / interrupt: drop pending refine (and compact, via compact_host).
pub(crate) fn clear_pending_refine(session_id: &devo_protocol::native::ids::SessionId) {
    let _ = take_pending_refine(session_id);
}

/// Seed the in-memory auto-refine turn counter from durable history when a
/// root session actor is (re-)inserted.
///
/// The counter normally lives in [`AUTO_REFINE_TURN_COUNTS`]; without seeding,
/// every server restart resets it to zero, so with the default interval of 25
/// a session whose server restarts inside the window never reaches the
/// auto-refine threshold. Deriving the count from the persisted journal
/// (successful-root-turn approximation: distinct regular turns recorded after
/// the most recent applied refinement) keeps the trigger firing on schedule
/// across restarts. A refine that is scheduled but not yet applied has no
/// journal marker yet — seeding is skipped while one is pending so it is not
/// double-counted.
pub(crate) fn seed_auto_refine_turn_count(
    session_id: &devo_protocol::native::ids::SessionId,
    persisted_turn_items: &[crate::execution::PersistedTurnItem],
) {
    if peek_pending_refine(session_id) {
        return;
    }
    let count = turns_since_last_refinement(persisted_turn_items);
    AUTO_REFINE_TURN_COUNTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_id.as_str().to_string(), count);
}

/// Distinct regular turns journaled after the most recent refinement item
/// (or after the session start when none was ever applied).
pub(crate) fn turns_since_last_refinement(
    persisted_turn_items: &[crate::execution::PersistedTurnItem],
) -> u32 {
    let after = persisted_turn_items
        .iter()
        .rposition(|item| {
            matches!(
                item.item,
                devo_protocol::native::item::Item::Refinement { .. }
            )
        })
        .map_or(0, |index| index + 1);
    let mut distinct_turns = std::collections::HashSet::new();
    for item in &persisted_turn_items[after..] {
        if item.turn_kind == devo_protocol::native::turn::TurnKind::Regular {
            distinct_turns.insert(item.turn_id);
        }
    }
    distinct_turns.len() as u32
}

pub(crate) fn auto_refine_settings_from_session(
    enabled: Option<bool>,
    interval: Option<u32>,
) -> AutoRefineSettings {
    let mut settings = AutoRefineSettings::default();
    if let Some(enabled) = enabled {
        settings.enabled = enabled;
    }
    if let Some(interval) = interval {
        settings.turn_interval = interval.max(1);
    }
    settings
}

/// Bump the root user-visible turn counter; schedule auto-refine when due.
pub(crate) fn maybe_schedule_auto_refine(
    session_id: &devo_protocol::native::ids::SessionId,
    settings: &AutoRefineSettings,
    is_root: bool,
    is_goal_continuation: bool,
    turn_succeeded: bool,
    session_dir: Option<&Path>,
) -> bool {
    if !turn_succeeded || !is_root || is_goal_continuation {
        return false;
    }
    let count = {
        let mut map = AUTO_REFINE_TURN_COUNTS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = map.entry(session_id.as_str().to_string()).or_insert(0);
        *entry = entry.saturating_add(1);
        *entry
    };
    if !should_auto_refine(settings, count, is_root, is_goal_continuation) {
        return false;
    }
    let params = SessionRefineRunParams {
        session_id: *session_id,
        instructions: Some(AUTO_REFINE_INSTRUCTION.into()),
        global: false,
        rollback_id: None,
    };
    let result = schedule_refine_run_inner(
        session_id,
        &params,
        /*is_root*/ true,
        session_dir,
        /*autonomous*/ true,
    );
    if result.scheduled {
        let mut map = AUTO_REFINE_TURN_COUNTS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.insert(session_id.as_str().to_string(), 0);
    }
    result.scheduled
}

/// Schedule a refine for apply at the next turn boundary / idle.
pub(crate) fn schedule_refine_run(
    session_id: &devo_protocol::native::ids::SessionId,
    params: &SessionRefineRunParams,
    is_root: bool,
    session_dir: Option<&Path>,
) -> SessionRefineRunResult {
    schedule_refine_run_inner(
        session_id,
        params,
        is_root,
        session_dir,
        /*autonomous*/ false,
    )
}

fn schedule_refine_run_inner(
    session_id: &devo_protocol::native::ids::SessionId,
    params: &SessionRefineRunParams,
    is_root: bool,
    session_dir: Option<&Path>,
    autonomous: bool,
) -> SessionRefineRunResult {
    if !is_root {
        return SessionRefineRunResult {
            scheduled: false,
            note: None,
            reason: Some("refine.run is not available on child sessions".into()),
            refinement_id: None,
        };
    }
    if let Some(dir) = session_dir
        && let Err(error) = ensure_session_harness(dir)
    {
        return SessionRefineRunResult {
            scheduled: false,
            note: None,
            reason: Some(format!("harness state unavailable: {error}")),
            refinement_id: None,
        };
    }
    let proposal_id = format!("refine_{}", Uuid::new_v4());
    let pending = PendingRefine {
        proposal_id: proposal_id.clone(),
        instructions: params.instructions.clone(),
        global: params.global,
        rollback_id: params.rollback_id.clone(),
        requested_at: Utc::now(),
        autonomous,
    };
    PENDING_REFINES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_id.as_str().to_string(), pending);
    SessionRefineRunResult {
        scheduled: true,
        note: Some(
            "Refinement runs when the current turn ends (or immediately if idle); applied edits appear as a Refinement item."
                .into(),
        ),
        reason: None,
        refinement_id: Some(proposal_id),
    }
}

/// Build a no-op proposal placeholder used when instructions are absent.
pub(crate) fn placeholder_proposal(pending: &PendingRefine) -> RefineProposal {
    RefineProposal {
        id: pending.proposal_id.clone(),
        trigger: pending
            .instructions
            .clone()
            .unwrap_or_else(|| "manual".into()),
        summary: "Refine scheduled (no instruction edits)".into(),
        evidence: String::new(),
        expected_outcome: String::new(),
        edits: Vec::new(),
    }
}

fn truncate_for_harness(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Heuristic refine planner: with non-empty instructions, emit a memory Create
/// edit. Used as fallback when LLM planning is unavailable.
pub(crate) fn plan_refine_proposal(pending: &PendingRefine, harness_path: &Path) -> RefineProposal {
    let _ = HarnessState::load(harness_path);
    let Some(instructions) = pending
        .instructions
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    else {
        return placeholder_proposal(pending);
    };
    if instructions == AUTO_REFINE_INSTRUCTION {
        // The auto-interval sentinel is not guidance, and the heuristic has no
        // trajectory to distill — echoing the sentinel would store noise.
        return placeholder_proposal(pending);
    }

    let uuid_hex = Uuid::new_v4().as_simple().to_string();
    let mem_id = format!("mem_refine_{}", &uuid_hex[..8]);
    let title = truncate_for_harness(instructions, 80);
    let content = truncate_for_harness(instructions, 500);
    let now = Utc::now();
    let entry = HarnessEntry {
        id: mem_id.clone(),
        kind: HarnessKind::Memory,
        title: title.clone(),
        content: content.clone(),
        path: None,
        scope: Some(refine_target_scope(pending)),
        reference: json!({}),
        arguments: json!({}),
        metadata: json!({ "source": "refine_heuristic" }),
        source: "refine".into(),
        created_at: now,
        updated_at: now,
        version: 1,
    };

    RefineProposal {
        id: pending.proposal_id.clone(),
        trigger: instructions.to_string(),
        summary: format!("Record refine instruction as memory: {title}"),
        evidence: format!("User/agent refine instructions: {content}"),
        expected_outcome: format!("Harness memory `{mem_id}` captures the refine focus."),
        edits: vec![RefineEdit {
            op: RefineEditOp::Create,
            kind: HarnessKind::Memory,
            id: mem_id,
            before: None,
            after: Some(entry),
        }],
    }
}

#[allow(dead_code)] // host_request refine.status
pub(crate) fn pending_status_json(
    session_id: &devo_protocol::native::ids::SessionId,
) -> serde_json::Value {
    json!({
        "pending": peek_pending_refine(session_id),
        "inFlight": false,
    })
}

/// Apply pending refine at the turn boundary (heuristic planner).
///
/// Used by unit tests; production post-turn uses
/// [`ServerRuntime::apply_pending_refine_at_boundary`].
#[cfg(test)]
pub(crate) fn apply_pending_refine_at_boundary(
    session_id: &devo_protocol::native::ids::SessionId,
    session_dir: &Path,
    plan_mode: bool,
) -> Option<AppliedRefine> {
    apply_pending_refine_with_proposal(session_id, session_dir, plan_mode, plan_refine_proposal)
}

#[cfg(test)]
fn apply_pending_refine_with_proposal(
    session_id: &devo_protocol::native::ids::SessionId,
    session_dir: &Path,
    plan_mode: bool,
    plan: impl FnOnce(&PendingRefine, &Path) -> RefineProposal,
) -> Option<AppliedRefine> {
    let pending = take_pending_refine(session_id)?;
    if plan_mode && pending.autonomous {
        // Re-queue autonomous refine until Plan Mode ends.
        PENDING_REFINES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_id.as_str().to_string(), pending);
        return None;
    }

    let harness_path = match refine_target_harness_path(session_dir, &pending) {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(%error, "refine apply skipped: harness unavailable");
            return None;
        }
    };

    let proposal = plan(&pending, &harness_path);
    match apply_proposal_re_read(&harness_path, &proposal) {
        Ok(_state) => {
            let mut record =
                record_from_proposal(&proposal, &harness_path, Some(refine_scope_label(&pending)));
            record.rollback_of = pending.rollback_id.clone();
            if let Err(error) = append_refinement(session_dir, &record) {
                tracing::warn!(%error, "failed to append refinements.jsonl");
            }
            Some(AppliedRefine {
                proposal,
                autonomous: pending.autonomous,
            })
        }
        Err(error) => {
            tracing::warn!(%error, "refine apply failed");
            None
        }
    }
}

impl ServerRuntime {
    /// Apply pending refine with LLM planning (`UsagePurpose::Refine`) and
    /// heuristic fallback.
    pub(crate) async fn apply_pending_refine_at_boundary(
        self: &Arc<Self>,
        session_id: SessionId,
        native_session_id: &devo_protocol::native::ids::SessionId,
        session_dir: &Path,
        plan_mode: bool,
    ) -> Option<AppliedRefine> {
        let pending = take_pending_refine(native_session_id)?;
        if plan_mode && pending.autonomous {
            PENDING_REFINES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(native_session_id.as_str().to_string(), pending);
            return None;
        }
        let harness_path = match refine_target_harness_path(session_dir, &pending) {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(%error, "refine apply skipped: harness unavailable");
                return None;
            }
        };
        let proposal = self
            .plan_refine_proposal_llm(session_id, &pending, &harness_path)
            .await;
        // Re-insert was already consumed; apply directly.
        match apply_proposal_re_read(&harness_path, &proposal) {
            Ok(_state) => {
                let mut record = record_from_proposal(
                    &proposal,
                    &harness_path,
                    Some(refine_scope_label(&pending)),
                );
                record.rollback_of = pending.rollback_id.clone();
                if let Err(error) = append_refinement(session_dir, &record) {
                    tracing::warn!(%error, "failed to append refinements.jsonl");
                }
                Some(AppliedRefine {
                    proposal,
                    autonomous: pending.autonomous,
                })
            }
            Err(error) => {
                tracing::warn!(%error, "refine apply failed");
                None
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AppliedRefine {
    pub proposal: RefineProposal,
    #[allow(dead_code)]
    pub autonomous: bool,
}

impl AppliedRefine {
    pub fn native_item(&self) -> Item {
        Item::Refinement {
            refinement_id: self.proposal.id.clone(),
            trigger: self.proposal.trigger.clone(),
            summary: self.proposal.summary.clone(),
            changes: self
                .proposal
                .edits
                .iter()
                .map(|e| format!("{:?} {:?}:{}", e.op, e.kind, e.id))
                .collect(),
            evidence: (!self.proposal.evidence.is_empty()).then(|| self.proposal.evidence.clone()),
            outcome: (!self.proposal.expected_outcome.is_empty())
                .then(|| self.proposal.expected_outcome.clone()),
        }
    }
}

impl ServerRuntime {
    /// Native `session/refine/run` (L2-DES-HARNESS-001).
    pub(super) async fn handle_native_session_refine_run(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionRefineRunParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/refine/run params: {error}"),
                );
            }
        };
        let legacy_id = params.session_id;
        let (is_root, session_dir) = {
            let sessions = self.sessions.lock().await;
            let Some(handle) = sessions.get(&legacy_id) else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionNotFound,
                    format!("session {} not found", params.session_id.as_str()),
                );
            };
            let summary = handle.summary().await;
            let is_root = summary.as_ref().is_none_or(|s| s.agent_path.is_none());
            let session_dir = handle.rollout_path().await.flatten().and_then(|path| {
                crate::persistence::RolloutStore::rlm_session_dir_for_rollout(&path)
            });
            (is_root, session_dir)
        };
        let result =
            schedule_refine_run(&params.session_id, &params, is_root, session_dir.as_deref());
        if result.scheduled
            && self.runtime_active_turn_id(legacy_id).await.is_none()
            && let Some(session_dir) = session_dir
        {
            // The promised contract is "applies immediately when idle", but
            // the boundary apply only runs in the post-turn pipeline — an
            // idle session would sit on the pending refine until the user's
            // next turn. Apply now (fire-and-forget so the RPC stays snappy;
            // a turn that starts meanwhile simply loses the race and the
            // boundary apply no-ops on the already-taken pending).
            let runtime = Arc::clone(self);
            let native_session_id = params.session_id;
            let legacy_session_id = legacy_id;
            tokio::spawn(async move {
                let Some(applied) = runtime
                    .apply_pending_refine_at_boundary(
                        legacy_session_id,
                        &native_session_id,
                        &session_dir,
                        /*plan_mode*/ false,
                    )
                    .await
                else {
                    return;
                };
                runtime
                    .persist_applied_refinement(legacy_session_id, &applied)
                    .await;
            });
        }
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result,
        })
        .expect("serialize session/refine/run response")
    }

    /// Persist `Item::Refinement` after a successful boundary apply.
    pub(crate) async fn persist_applied_refinement(
        self: &Arc<Self>,
        session_id: SessionId,
        applied: &AppliedRefine,
    ) {
        let Some(handle) = self.session(session_id).await else {
            return;
        };
        let Some(rollout_path) = handle.rollout_path().await.flatten() else {
            return;
        };
        let resume_snapshot = handle.resume_snapshot().await;
        let native_session_id = if let Some(snap) = resume_snapshot.as_ref() {
            snap.summary.native.id
        } else if let Some(session) = handle.native_session().await {
            session.id
        } else {
            // boundary: session actor has no native session projection
            session_id
        };
        let native_turn_id = resume_snapshot
            .and_then(|snap| snap.latest_turn.map(|turn| turn.native.id))
            .unwrap_or_else(|| {
                // boundary: no RuntimeTurn on actor when refinement applied
                TurnId::new()
            });
        let seq = handle.allocate_item_seq().await.unwrap_or(0);
        let now = Utc::now();
        let envelope = ItemEnvelope {
            id: devo_protocol::native::ids::ItemId::new(),
            session_id: native_session_id,
            turn_id: native_turn_id,
            seq,
            revision: 1,
            created_at: now,
            updated_at: now,
            state: ItemState::Completed,
            item: applied.native_item(),
            parent_id: None,
        };
        if let Err(error) = self
            .rollout_store
            .append_canonical_item_at(&rollout_path, envelope)
        {
            tracing::warn!(%error, "failed to persist Item::Refinement");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: child sessions cannot schedule refine.run.
    #[test]
    fn children_cannot_schedule() {
        let id = devo_protocol::native::ids::SessionId::from_string("ses_child".into());
        let params = SessionRefineRunParams {
            session_id: id,
            instructions: None,
            global: false,
            rollback_id: None,
        };
        let result = schedule_refine_run(&id, &params, false, None);
        assert!(!result.scheduled);
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: root schedule last-write-wins pending refine.
    #[test]
    fn root_schedule_is_pending() {
        let id =
            devo_protocol::native::ids::SessionId::from_string(format!("ses_{}", Uuid::new_v4()));
        let params = SessionRefineRunParams {
            session_id: id,
            instructions: Some("focus".into()),
            global: false,
            rollback_id: None,
        };
        let result = schedule_refine_run(&id, &params, true, None);
        assert!(result.scheduled);
        assert!(peek_pending_refine(&id));
        let taken = take_pending_refine(&id).expect("pending");
        assert_eq!(taken.instructions.as_deref(), Some("focus"));
        assert!(!taken.autonomous);
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: should_auto_refine integration schedules on interval for roots.
    #[test]
    fn auto_interval_schedules_on_root() {
        let id =
            devo_protocol::native::ids::SessionId::from_string(format!("ses_{}", Uuid::new_v4()));
        let settings = AutoRefineSettings {
            enabled: true,
            turn_interval: 2,
            ..AutoRefineSettings::default()
        };
        assert!(!maybe_schedule_auto_refine(
            &id, &settings, true, false, true, None
        ));
        assert!(!peek_pending_refine(&id));
        assert!(maybe_schedule_auto_refine(
            &id, &settings, true, false, true, None
        ));
        assert!(peek_pending_refine(&id));
        let taken = take_pending_refine(&id).expect("auto pending");
        assert!(taken.autonomous);
        assert_eq!(taken.instructions.as_deref(), Some(AUTO_REFINE_INSTRUCTION));
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: children never auto-refine even at interval.
    #[test]
    fn auto_interval_skips_children() {
        let id =
            devo_protocol::native::ids::SessionId::from_string(format!("ses_{}", Uuid::new_v4()));
        let settings = AutoRefineSettings {
            turn_interval: 1,
            ..AutoRefineSettings::default()
        };
        assert!(!maybe_schedule_auto_refine(
            &id, &settings, false, false, true, None
        ));
        assert!(!peek_pending_refine(&id));
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: boundary apply writes refinements.jsonl (never mid-ipython).
    #[test]
    fn apply_at_boundary_persists_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let id =
            devo_protocol::native::ids::SessionId::from_string(format!("ses_{}", Uuid::new_v4()));
        let params = SessionRefineRunParams {
            session_id: id,
            instructions: Some("manual".into()),
            global: false,
            rollback_id: None,
        };
        assert!(schedule_refine_run(&id, &params, true, Some(dir.path())).scheduled);
        let applied = apply_pending_refine_at_boundary(&id, dir.path(), false).expect("applied");
        assert!(!applied.proposal.id.is_empty());
        let log = std::fs::read_to_string(devo_harness::refinements_log_path(dir.path())).unwrap();
        assert!(log.contains(&applied.proposal.id));
        assert!(!peek_pending_refine(&id));
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: Plan Mode holds autonomous pending without applying.
    #[test]
    fn plan_mode_defers_autonomous_apply() {
        let dir = tempfile::tempdir().unwrap();
        let id =
            devo_protocol::native::ids::SessionId::from_string(format!("ses_{}", Uuid::new_v4()));
        let settings = AutoRefineSettings {
            turn_interval: 1,
            ..AutoRefineSettings::default()
        };
        assert!(maybe_schedule_auto_refine(
            &id,
            &settings,
            true,
            false,
            true,
            Some(dir.path())
        ));
        assert!(apply_pending_refine_at_boundary(&id, dir.path(), true).is_none());
        assert!(peek_pending_refine(&id));
        clear_pending_refine(&id);
    }

    #[test]
    fn default_interval_matches_spec() {
        assert_eq!(devo_harness::DEFAULT_TURN_INTERVAL, 25);
        assert_eq!(
            auto_refine_settings_from_session(None, None).turn_interval,
            25
        );
        assert!(auto_refine_settings_from_session(None, None).enabled);
        assert!(!auto_refine_settings_from_session(Some(false), Some(10)).enabled);
        assert_eq!(
            auto_refine_settings_from_session(Some(true), Some(10)).turn_interval,
            10
        );
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: plan_refine_proposal with instructions yields a memory Create edit.
    #[test]
    fn plan_refine_proposal_with_instructions_emits_memory_create() {
        let dir = tempfile::tempdir().unwrap();
        let path = HarnessState::file_path(dir.path());
        HarnessState::default().save_atomic(&path).unwrap();
        let pending = PendingRefine {
            proposal_id: "refine_test".into(),
            instructions: Some("  focus on harness digest wiring  ".into()),
            global: false,
            rollback_id: None,
            requested_at: Utc::now(),
            autonomous: false,
        };
        let proposal = plan_refine_proposal(&pending, &path);
        assert_eq!(proposal.id, "refine_test");
        assert!(!proposal.edits.is_empty());
        assert_eq!(proposal.edits.len(), 1);
        let edit = &proposal.edits[0];
        assert_eq!(edit.op, RefineEditOp::Create);
        assert_eq!(edit.kind, HarnessKind::Memory);
        assert!(edit.id.starts_with("mem_refine_"));
        let after = edit.after.as_ref().expect("after");
        assert!(after.content.contains("harness digest"));
        assert!(proposal.summary.contains("harness digest"));
        assert!(proposal.evidence.contains("harness digest"));
        assert!(proposal.expected_outcome.contains(&edit.id));
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: empty/missing instructions keep the no-op empty-edits proposal.
    #[test]
    fn plan_refine_proposal_without_instructions_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = HarnessState::file_path(dir.path());
        HarnessState::default().save_atomic(&path).unwrap();
        let pending = PendingRefine {
            proposal_id: "refine_empty".into(),
            instructions: Some("   ".into()),
            global: false,
            rollback_id: None,
            requested_at: Utc::now(),
            autonomous: false,
        };
        let proposal = plan_refine_proposal(&pending, &path);
        assert!(proposal.edits.is_empty());
        let pending_none = PendingRefine {
            instructions: None,
            ..pending
        };
        assert!(plan_refine_proposal(&pending_none, &path).edits.is_empty());
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: the auto-interval sentinel is a scheduling marker, not
    /// guidance — the heuristic must not echo it into a durable memory
    /// (bug #48), while the durable trigger keeps the sentinel label.
    #[test]
    fn plan_refine_proposal_auto_sentinel_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = HarnessState::file_path(dir.path());
        HarnessState::default().save_atomic(&path).unwrap();
        let pending = PendingRefine {
            proposal_id: "refine_auto".into(),
            instructions: Some(AUTO_REFINE_INSTRUCTION.into()),
            global: false,
            rollback_id: None,
            requested_at: Utc::now(),
            autonomous: true,
        };
        let proposal = plan_refine_proposal(&pending, &path);
        assert!(
            proposal.edits.is_empty(),
            "sentinel echoed as edits: {:?}",
            proposal.edits
        );
        assert_eq!(proposal.trigger, AUTO_REFINE_INSTRUCTION);
    }

    #[test]
    fn turns_since_last_refinement_counts_regular_turns_after_last_refine() {
        use crate::persisted_native_item::PersistedNativeItem;
        use devo_protocol::native::ids::{ItemId, TurnId};
        use devo_protocol::native::item::Item;
        use devo_protocol::native::turn::TurnKind;

        let item = |turn: TurnId, kind: TurnKind, id: &str| {
            PersistedNativeItem::new(
                turn,
                kind,
                ItemId::from_string(id.to_string()),
                Item::AssistantMessage {
                    text: format!("item {id}"),
                },
            )
        };
        let t1 = TurnId::new();
        let t2 = TurnId::new();
        let t3 = TurnId::new();
        let refine_turn = TurnId::new();
        let journal = vec![
            item(t1, TurnKind::Regular, "a1"),
            item(t1, TurnKind::Regular, "a2"),
            // Compaction turns are bookkeeping, not user-visible turns.
            item(TurnId::new(), TurnKind::Compaction, "c1"),
            item(t2, TurnKind::Regular, "b1"),
            // A refinement applied at t2's boundary.
            PersistedNativeItem::new(
                refine_turn,
                TurnKind::Regular,
                ItemId::from_string("r1".to_string()),
                Item::Refinement {
                    refinement_id: "refine_1".to_string(),
                    trigger: "auto-interval".to_string(),
                    summary: "applied".to_string(),
                    changes: Vec::new(),
                    evidence: None,
                    outcome: None,
                },
            ),
            item(t3, TurnKind::Regular, "d1"),
            item(t3, TurnKind::Regular, "d2"),
        ];

        // Only t3 (after the refinement marker) counts.
        assert_eq!(turns_since_last_refinement(&journal), 1);
        // Without any refinement marker, every regular turn counts
        // (restart must not silently reset the interval to zero).
        assert_eq!(turns_since_last_refinement(&journal[..4]), 2);
    }

    /// Trace: bug #45 — global refine must target the global store.
    /// Verifies: heuristic planner entries and records follow the requested
    /// scope, and the Devo home resolves to the global harness layout.
    #[test]
    fn global_refine_targets_global_store() {
        let global_pending = PendingRefine {
            proposal_id: "refine_global_test".into(),
            instructions: Some("remember the west wing passphrase".into()),
            global: true,
            rollback_id: None,
            requested_at: Utc::now(),
            autonomous: false,
        };
        assert_eq!(refine_target_scope(&global_pending), HarnessScope::Global);
        assert_eq!(refine_scope_label(&global_pending), "global");
        let proposal = plan_refine_proposal(&global_pending, Path::new("/unused"));
        let entry = proposal.edits[0].after.as_ref().expect("heuristic entry");
        assert_eq!(entry.scope, Some(HarnessScope::Global));

        let local_pending = PendingRefine {
            proposal_id: "refine_local_test".into(),
            instructions: Some("remember the session color".into()),
            global: false,
            rollback_id: None,
            requested_at: Utc::now(),
            autonomous: false,
        };
        assert_eq!(refine_target_scope(&local_pending), HarnessScope::Local);
        let session_dir = tempfile::tempdir().expect("session dir");
        assert_eq!(
            refine_target_harness_path(session_dir.path(), &local_pending).expect("local path"),
            session_dir
                .path()
                .join("harness")
                .join("harness_state.json")
        );
        assert_eq!(refine_scope_label(&local_pending), "local");
        let proposal = plan_refine_proposal(&local_pending, Path::new("/unused"));
        let entry = proposal.edits[0].after.as_ref().expect("heuristic entry");
        assert_eq!(entry.scope, Some(HarnessScope::Local));

        // Passing the Devo home as the "session dir" of the store helper is
        // exactly how refine_target_harness_path resolves the global store:
        // <home>/harness/harness_state.json.
        let home = tempfile::tempdir().expect("home dir");
        let path = ensure_session_harness(home.path()).expect("global store path");
        assert_eq!(path, home.path().join("harness").join("harness_state.json"));
        assert!(path.is_file());
    }
}
