use std::sync::Arc;

use devo_core::SessionTitleState;
use devo_core::TurnId;
use devo_protocol::ApprovalScopeValue;
use devo_protocol::CollaborationMode;
use devo_protocol::PendingInputItem;
use devo_protocol::ThreadGoal;
use tokio::sync::oneshot;

use super::snapshots::{
    HookContextSnapshot, PendingQueueSnapshot, PersistItemPrep, QueuedTurnInputData,
    ShellExecContextSnapshot, ShutdownDeferredSnapshot, TitleGenerationContext,
    TurnPersistenceSnapshot, TurnReservationSnapshot,
};
use super::state::{ApprovalCacheSnapshot, DeferredItems, SessionActorState, SpawnSnapshot};
use crate::execution::PendingApproval;
use crate::execution::PersistedTurnItem;
use crate::runtime::subagent_usage::ParentUsageSnapshot;
use crate::runtime_session_summary::RuntimeSessionSummary;
use crate::session::SessionHistoryEntry;
use crate::turn::RuntimeTurn;
use devo_core::TurnConfig;

use super::turn_working::TurnWorkingSet;

#[derive(Clone)]
pub(crate) struct ApprovalCheckpointSnapshot {
    pub(crate) messages: Vec<devo_protocol::Message>,
    pub(crate) turn_config: TurnConfig,
    pub(crate) collaboration_mode: devo_protocol::CollaborationMode,
}

pub(crate) enum SessionCommand {
    /// Short: clone turn-owned state and install `TurnInlineState` on the shared stream.
    CheckoutTurnWorkingSet {
        turn: RuntimeTurn,
        reply: oneshot::Sender<TurnWorkingSet>,
    },
    /// Short: install turn-owned fields after the spawned turn task finishes.
    MergeTurn {
        working: Box<TurnWorkingSet>,
        reply: oneshot::Sender<()>,
    },
    GetSummary {
        reply: oneshot::Sender<RuntimeSessionSummary>,
    },
    GetNativeSession {
        reply: oneshot::Sender<devo_protocol::native::session::Session>,
    },
    GetSpawnSnapshot {
        reply: oneshot::Sender<SpawnSnapshot>,
    },
    /// Full persisted transcript INCLUDING the active turn's items so far.
    /// Unlike [`SessionCommand::GetSpawnSnapshot`] (a fork snapshot, which
    /// excludes the in-flight turn), this is the read path for live
    /// observation (`agent_observe.recent`).
    GetTranscriptItems {
        reply: oneshot::Sender<Vec<PersistedTurnItem>>,
    },
    GetApprovalCacheSnapshot {
        reply: oneshot::Sender<ApprovalCacheSnapshot>,
    },
    GetCollaborationMode {
        reply: oneshot::Sender<CollaborationMode>,
    },
    GetParentSessionId {
        reply: oneshot::Sender<Option<devo_protocol::SessionId>>,
    },
    GetTurnReservationSnapshot {
        reply: oneshot::Sender<TurnReservationSnapshot>,
    },
    GetHookContextSnapshot {
        reply: oneshot::Sender<HookContextSnapshot>,
    },
    GetTurnPersistenceSnapshot {
        reply: oneshot::Sender<TurnPersistenceSnapshot>,
    },
    GetShellExecContext {
        cwd: std::path::PathBuf,
        reply: oneshot::Sender<ShellExecContextSnapshot>,
    },
    GetTitleGenerationContext {
        reply: oneshot::Sender<TitleGenerationContext>,
    },
    GetPendingQueueSnapshot {
        reply: oneshot::Sender<PendingQueueSnapshot>,
    },
    PopQueuedTurnInput {
        require_idle_session: bool,
        reply: oneshot::Sender<Option<QueuedTurnInputData>>,
    },
    EnqueuePendingTurnInput {
        item: PendingInputItem,
    },
    GetActiveTurnId {
        reply: oneshot::Sender<Option<TurnId>>,
    },
    GetApprovalCheckpointSnapshot {
        reply: oneshot::Sender<Option<ApprovalCheckpointSnapshot>>,
    },
    MarkActiveTurnWaitingApproval {
        turn_id: TurnId,
        reply: oneshot::Sender<Option<RuntimeTurn>>,
    },
    GetRolloutPath {
        reply: oneshot::Sender<Option<std::path::PathBuf>>,
    },
    PreparePersistItem {
        turn_id: TurnId,
        reply: oneshot::Sender<PersistItemPrep>,
    },
    TakeShutdownDeferredSnapshot {
        reply: oneshot::Sender<ShutdownDeferredSnapshot>,
    },
    AllocateItemSeq {
        reply: oneshot::Sender<u64>,
    },
    AppendPersistedItem {
        item: PersistedTurnItem,
    },
    AppendHistoryItem {
        item: SessionHistoryEntry,
    },
    TakeDeferredItems {
        reply: oneshot::Sender<DeferredItems>,
    },
    TouchLastActivity,
    ApplyApprovalScope {
        scope: ApprovalScopeValue,
        pending: PendingApproval,
    },
    UpdateSummary {
        summary: RuntimeSessionSummary,
    },
    SetFirstUserInputIfUnset {
        text: String,
        reply: oneshot::Sender<Option<String>>,
    },
    UpdateTitle {
        title: String,
        title_state: SessionTitleState,
        reply: oneshot::Sender<Option<RuntimeSessionSummary>>,
    },
    BeginActiveTurn {
        turn: RuntimeTurn,
        turn_config: TurnConfig,
    },
    ClearActiveTurnIfMatches {
        turn_id: TurnId,
        reply: oneshot::Sender<bool>,
    },
    SetSessionIdle {
        latest_turn: Option<RuntimeTurn>,
    },
    ActivateQueuedTurn {
        turn: RuntimeTurn,
        turn_config: TurnConfig,
    },
    UpdateCorePermissionMode {
        permission_mode: devo_safety::PermissionMode,
    },
    SetActiveGoal {
        goal: Option<ThreadGoal>,
    },
    #[cfg_attr(not(test), allow(dead_code))]
    UpdateRolloutPath {
        rollout_path: std::path::PathBuf,
    },
    /// Move the in-session transcript tip after `session/tree/navigate`.
    /// `scoped_item_ids` re-scopes the actor's linear in-memory journal to
    /// the navigated branch (None keeps the current items, e.g. when the
    /// rollout journal lags the actor); without it the next turn's model
    /// context would still include abandoned branches. `compaction_snapshot`
    /// is the rollout's latest snapshot: when its summary survives the
    /// re-scope, the rebuilt prompt keeps the compacted shape instead of
    /// reverting to the full scoped history.
    SetTranscriptLeaf {
        leaf_id: Option<devo_protocol::native::ids::ItemId>,
        epoch: u64,
        scoped_item_ids: Option<Vec<devo_protocol::native::ids::ItemId>>,
        compaction_snapshot: Option<devo_core::CompactionSnapshotLine>,
    },
    ApplyParentUsageSnapshot {
        snapshot: ParentUsageSnapshot,
    },
    InterruptActiveTurn {
        reply: oneshot::Sender<Option<RuntimeTurn>>,
    },
    ExportRuntimeSession {
        reply: oneshot::Sender<crate::execution::RuntimeSession>,
    },
    UpdateSessionWorkspace {
        cwd: std::path::PathBuf,
        runtime_context: Arc<crate::session_context::SessionRuntimeContext>,
    },
    SetArchived {
        archived: bool,
        reply: oneshot::Sender<devo_protocol::native::session::Session>,
    },
    UpdateSessionModelSettings {
        model: Option<String>,
        model_binding_id: Option<String>,
        reasoning_effort_selection: Option<String>,
        collaboration_mode: Option<CollaborationMode>,
        reply: oneshot::Sender<RuntimeSessionSummary>,
    },
    /// Native-session settings that live outside model selection (auto-refine
    /// enable/interval, python cell first-wait). Persisted first by the
    /// metadata-update handler; this mirrors them into the live actor so the
    /// post-turn pipeline and next-turn admission observe them without a
    /// rehydration. `None` fields are left unchanged.
    UpdateNativeSessionSettings {
        auto_refine_enabled: Option<bool>,
        auto_refine_turn_interval: Option<u32>,
        python_cell_first_wait_ms: Option<u64>,
        reply: oneshot::Sender<()>,
    },
    ApplyPermissionProfile {
        profile: devo_safety::RuntimePermissionProfile,
        reply: oneshot::Sender<()>,
    },
    ApplyEffectiveContextWindow {
        limit: usize,
        reply: oneshot::Sender<Result<(), String>>,
    },
    ApplySandboxProfile {
        profile: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
    SetSessionTitleUserRename {
        title: String,
        reply: oneshot::Sender<RuntimeSessionSummary>,
    },
    SetToolRegistry {
        tool_registry: Option<Arc<devo_core::tools::ToolRegistry>>,
        reply: oneshot::Sender<()>,
    },
    GetRuntimeContext {
        reply: oneshot::Sender<Arc<crate::session_context::SessionRuntimeContext>>,
    },
    GetResumeSnapshot {
        reply: oneshot::Sender<super::snapshots::SessionResumeSnapshot>,
    },
    TryBeginActiveTurn {
        turn: RuntimeTurn,
        turn_config: TurnConfig,
        reply: oneshot::Sender<bool>,
    },
    ReplaceState {
        state: Box<SessionActorState>,
        reply: oneshot::Sender<()>,
    },
    PersistTurnLine {
        runtime: Arc<crate::runtime::ServerRuntime>,
        turn: RuntimeTurn,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    Shutdown {
        reply: oneshot::Sender<Option<Arc<devo_kernel::KernelSession>>>,
    },
}
