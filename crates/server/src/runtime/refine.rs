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
    RefineEditOp, RefineProposal, append_refinement, apply_proposal_re_read, rollback_refinement,
    should_auto_refine,
};
use devo_protocol::native::item::Item;
use devo_protocol::native::rpc_session::{SessionRefineRunParams, SessionRefineRunResult};

/// Last-write-wins pending refine requests keyed by session id (in-memory).
static PENDING_REFINES: LazyLock<Mutex<HashMap<String, PendingRefine>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(test)]
#[derive(Clone)]
struct RefineRequeueTestGate {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[cfg(test)]
static REFINE_REQUEUE_TEST_GATES: LazyLock<Mutex<HashMap<String, RefineRequeueTestGate>>> =
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
    pub trajectory_item_ids: Vec<devo_protocol::native::ids::ItemId>,
    /// Sanitized evidence frozen by the user-facing handler at admission.
    pub trajectory_evidence: Option<serde_json::Value>,
    pub requested_at: chrono::DateTime<Utc>,
    /// True when scheduled by root auto-interval (Plan Mode skips apply).
    pub autonomous: bool,
    /// A finished plan deferred by a newer turn; never rerun the model on resume.
    pub prepared: Option<Result<(PathBuf, RefineProposal), String>>,
}

/// Ensure the session harness directory exists with a valid (or empty) state file.
pub(crate) fn ensure_session_harness(session_dir: &Path) -> Result<PathBuf, String> {
    let path = HarnessState::file_path(session_dir);
    devo_harness::initialize_harness(&path).map_err(|e| e.to_string())?;
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
/// Devo home (`<home>/harness/harness_state.json`, the same host-owned store
/// used by kernel `harness.*` requests). Global refinements used to be applied
/// to the session-local store
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
    // Automatic maintenance must not replace a user's pending request.
    if peek_pending_refine(session_id) {
        return false;
    }
    let params = SessionRefineRunParams {
        session_id: *session_id,
        instructions: Some(AUTO_REFINE_INSTRUCTION.into()),
        global: false,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
    };
    let result = schedule_refine_run_inner(
        session_id,
        &params,
        /*is_root*/ true,
        session_dir,
        /*autonomous*/ true,
        /*trajectory_evidence*/ None,
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
        /*trajectory_evidence*/ None,
    )
}

fn schedule_refine_run_inner(
    session_id: &devo_protocol::native::ids::SessionId,
    params: &SessionRefineRunParams,
    is_root: bool,
    _session_dir: Option<&Path>,
    autonomous: bool,
    trajectory_evidence: Option<serde_json::Value>,
) -> SessionRefineRunResult {
    if !is_root {
        return SessionRefineRunResult {
            scheduled: false,
            note: None,
            reason: Some("refine.run is not available on child sessions".into()),
            refinement_id: None,
        };
    }
    if params.trajectory_item_ids.len() > 16
        || (!params.trajectory_item_ids.is_empty()
            && (params.global
                || autonomous
                || params.rollback_id.is_some()
                || trajectory_evidence.is_none()))
    {
        return SessionRefineRunResult {
            scheduled: false,
            note: None,
            reason: Some("trajectory requires a manual local refine, at most 16 selected items, and no rollback".into()),
            refinement_id: None,
        };
    }
    // Admission only queues work. Filesystem validation and initialization run
    // on the blocking pool at apply time, never in the actor/host-request path.
    let proposal_id = format!("refine_{}", Uuid::new_v4());
    let pending = PendingRefine {
        proposal_id: proposal_id.clone(),
        instructions: params.instructions.clone(),
        global: params.global,
        rollback_id: params.rollback_id.clone(),
        trajectory_item_ids: params.trajectory_item_ids.clone(),
        trajectory_evidence,
        requested_at: Utc::now(),
        autonomous,
        prepared: None,
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
    if !pending.trajectory_item_ids.is_empty() || pending.rollback_id.is_some() {
        return placeholder_proposal(pending);
    }
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
    if plan_mode {
        // Re-queue refinement until Plan Mode ends.
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

    let proposal = if pending.rollback_id.is_some() {
        placeholder_proposal(&pending)
    } else {
        plan(&pending, &harness_path)
    };
    commit_refinement(session_dir, &harness_path, &pending, proposal)
}

fn commit_refinement(
    session_dir: &Path,
    harness_path: &Path,
    pending: &PendingRefine,
    proposal: RefineProposal,
) -> Option<AppliedRefine> {
    let result = if let Some(target) = &pending.rollback_id {
        rollback_refinement(harness_path, target, &pending.proposal_id)
    } else {
        apply_proposal_re_read(harness_path, &proposal).map(|state| (state, proposal))
    };
    match result {
        Ok((state, proposal)) => {
            // The canonical undo record and entries already committed together.
            // JSONL is only a compatibility mirror, never the rollback authority.
            if let Some(record) = state.refinement_history.last()
                && let Err(error) = append_refinement(session_dir, record)
            {
                tracing::warn!(%error, "failed to mirror refinements.jsonl; undo history is durable");
            }
            Some(AppliedRefine {
                proposal,
                autonomous: pending.autonomous,
                failure: None,
            })
        }
        Err(error) => {
            tracing::warn!(%error, "refine apply failed");
            Some(AppliedRefine::failed(pending, error.to_string()))
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
        loop {
            let mut pending = take_pending_refine(native_session_id)?;
            if plan_mode {
                PENDING_REFINES
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(native_session_id.as_str().to_string())
                    .or_insert(pending);
                return None;
            }
            let prepared = if let Some(prepared) = pending.prepared.take() {
                prepared
            } else {
                let target_dir = session_dir.to_path_buf();
                let target_pending = pending.clone();
                let harness_path = tokio::task::spawn_blocking(move || {
                    refine_target_harness_path(&target_dir, &target_pending)
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result);
                match harness_path {
                    Err(error) => Err(error),
                    Ok(path) if pending.rollback_id.is_some() => {
                        Ok((path, placeholder_proposal(&pending)))
                    }
                    Ok(path) => self
                        .plan_refine_proposal_llm(session_id, &pending, &path)
                        .await
                        .map(|proposal| (path, proposal)),
                }
            };
            let handle = self.session(session_id).await?;
            // Never hold admission while waiting for the model. Recheck after
            // planning, then serialize only the local commit + canonical item write
            // against new turn admission, just like compaction's apply phase.
            let _state_change_guard = handle.lock_state_change().await;
            if self.runtime_active_turn_id(session_id).await.is_some()
                || self.active_stream_state(session_id).await.is_some()
                || handle.collaboration_mode().await == Some(devo_protocol::CollaborationMode::Plan)
            {
                #[cfg(test)]
                {
                    let gate = REFINE_REQUEUE_TEST_GATES
                        .lock()
                        .unwrap()
                        .remove(native_session_id.as_str());
                    if let Some(gate) = gate {
                        gate.entered.notify_one();
                        gate.release.notified().await;
                    }
                }
                pending.prepared = Some(prepared);
                PENDING_REFINES
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(native_session_id.as_str().to_string())
                    .or_insert(pending);
                // Post-turn can finish while this plan is in flight and observe
                // no pending request. Recheck *after* publishing the prepared work:
                // either its boundary takes it, or we drain it here when idle.
                if self.runtime_active_turn_id(session_id).await.is_none()
                    && self.active_stream_state(session_id).await.is_none()
                    && handle.collaboration_mode().await
                        != Some(devo_protocol::CollaborationMode::Plan)
                {
                    continue;
                }
                return None;
            }
            let applied = match prepared {
                Err(error) => Some(AppliedRefine::failed(&pending, error)),
                Ok((harness_path, proposal)) => {
                    let session_dir = session_dir.to_path_buf();
                    let failed_pending = pending.clone();
                    match tokio::task::spawn_blocking(move || {
                        commit_refinement(&session_dir, &harness_path, &pending, proposal)
                    })
                    .await
                    {
                        Ok(applied) => applied,
                        Err(error) => {
                            Some(AppliedRefine::failed(&failed_pending, error.to_string()))
                        }
                    }
                }
            };
            if let Some(applied) = &applied {
                // Still under the idle admission gate: actor sequence allocation and
                // the transcript leaf cannot race a new inline turn or MergeTurn.
                self.persist_applied_refinement(session_id, applied).await;
            }
            return applied;
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AppliedRefine {
    pub proposal: RefineProposal,
    pub failure: Option<String>,
    #[allow(dead_code)]
    pub autonomous: bool,
}

impl AppliedRefine {
    fn failed(pending: &PendingRefine, error: String) -> Self {
        Self {
            proposal: placeholder_proposal(pending),
            autonomous: pending.autonomous,
            failure: Some(error),
        }
    }

    pub fn native_item(&self) -> Item {
        if let Some(error) = &self.failure {
            return Item::Warning {
                code: "refinement_not_applied".into(),
                message: format!("Refinement {} was not applied: {error}", self.proposal.id),
                retryable: false,
            };
        }
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
        let Some(handle) = self.session(legacy_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                format!("session {} not found", params.session_id.as_str()),
            );
        };
        let summary = handle.summary().await;
        let is_root = summary.as_ref().is_none_or(|s| s.agent_path.is_none());
        let session_dir = summary.as_ref().and_then(|s| s.session_dir.clone());
        let plan_mode =
            handle.collaboration_mode().await == Some(devo_protocol::CollaborationMode::Plan);
        let trajectory_evidence = if params.trajectory_item_ids.is_empty() {
            None
        } else {
            if !is_root || self.runtime_active_turn_id(legacy_id).await.is_some() {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "trajectory selection requires an idle root session",
                );
            }
            let Some(items) = handle.transcript_items().await else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "trajectory is unavailable",
                );
            };
            match super::refine_planner::trajectory::selected_evidence(
                &items,
                &params.trajectory_item_ids,
            ) {
                Ok(evidence) => Some(evidence),
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        error,
                    );
                }
            }
        };
        if session_dir.is_none() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "Refinement requires a persistent session directory",
            );
        }
        let mut result = schedule_refine_run_inner(
            &params.session_id,
            &params,
            is_root,
            session_dir.as_deref(),
            /*autonomous*/ false,
            trajectory_evidence,
        );
        let idle = self.runtime_active_turn_id(legacy_id).await.is_none();
        if result.scheduled {
            result.note = Some(
                if plan_mode {
                    "Refinement queued until Plan Mode ends."
                } else if idle {
                    "Refinement started; the result will appear when it finishes."
                } else {
                    "Refinement queued until the current turn ends."
                }
                .into(),
            );
        }
        if result.scheduled
            && idle
            && let Some(session_dir) = session_dir
        {
            // Start idle work without blocking the RPC. A newer turn defers the
            // prepared proposal until a stable post-turn boundary.
            let runtime = Arc::clone(self);
            let native_session_id = params.session_id;
            let legacy_session_id = legacy_id;
            tokio::spawn(async move {
                runtime
                    .apply_pending_refine_at_boundary(
                        legacy_session_id,
                        &native_session_id,
                        &session_dir,
                        plan_mode,
                    )
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
        // Use the normal item transaction so the result is on the active
        // transcript path, in the actor snapshot, durable, and broadcast.
        if let Err(error) = self
            .complete_native_item_checked(
                native_session_id,
                native_turn_id,
                devo_protocol::native::ids::ItemId::new(),
                seq,
                applied.native_item(),
            )
            .await
        {
            tracing::warn!(%error, "failed to persist Item::Refinement");
        }
    }
}

#[cfg(test)]
#[path = "refine_tests.rs"]
mod tests;
