use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use chrono::Utc;
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use devo_core::Message;
use devo_core::ResponseItem;
use devo_core::SessionTitleFinalSource;
use devo_core::SessionTitleState;
use devo_core::TokenInfo;
use devo_core::TurnStatus;
use devo_core::history::compaction::CompactAction;
use devo_core::history::compaction::CompactionConfig;
use devo_core::history::compaction::CompactionKind;
use devo_core::history::compaction::compact_history;
use devo_core::history::summarizer::DefaultHistorySummarizer;
use devo_core::message_to_response_items;
use devo_core::tools::AgentToolCoordinator;
use devo_core::tools::PermissionChecker;
use devo_core::tools::PermissionGrant;
use devo_core::tools::ToolCallError;
use devo_core::tools::ToolPermissionRequest;
use devo_protocol::WorkspaceChangeAttribution;
use devo_protocol::WorkspaceChangeScope;
use devo_protocol::WorkspaceChangeView;
use devo_protocol::WorkspaceChangesReadParams;
use devo_protocol::WorkspaceDiffDetail;
use devo_protocol::native::ids::SessionId;
use devo_protocol::native::ids::TurnId;
use devo_safety::PermissionMode;

use crate::ApprovalDecisionValue;
use crate::ApprovalScopeValue;
use crate::ClientTransportKind;
use crate::ConnectionState;
use crate::ErrorResponse;
use crate::InitializeResult;
use crate::ProtocolError;
use crate::ProtocolErrorCode;
use crate::ProtocolExposurePolicy;
use crate::ProtocolSet;
use crate::RequestUserInputArgs;
use crate::RequestUserInputResponse;
use crate::ServerProtocol;
use crate::SessionForkResult;
use crate::SessionResumeParams;
use crate::SessionResumeResult;
use crate::SessionStartParams;
use crate::SessionStartResult;
use crate::SuccessResponse;
use crate::TurnInterruptParams;
use crate::TurnInterruptResult;
use crate::TurnStartParams;
use crate::TurnStartResult;
use crate::approval_reviewer::build_approval_review_request;
use crate::approval_reviewer::extend_approval_review_request;
use crate::approval_reviewer::parse_reviewer_decision;
use crate::db::QueueType;
use crate::execution::PendingUserInput;
use crate::execution::RuntimeSession;
use crate::execution::ServerRuntimeDependencies;
use crate::goal::Goal;
use crate::goal::GoalAction;
use crate::goal::GoalId;
use crate::goal::GoalMutation;
use crate::goal_durable::GoalDurableStore;
use crate::persistence::RolloutStore;
pub(crate) use crate::runtime::handlers::goal::GoalStore;
use crate::subagent::AgentPath;
use crate::subagent::AgentRegistry;
use crate::subagent::SubagentMailbox;
use crate::subagent::SubagentMetadata;
use crate::subagent::SubagentOutputBuffer;
use crate::subagent::SubagentStatus;
use crate::usage_ledger::UsageLedger;
use crate::workspace_changes::ActiveWorkspaceBaseline;
use devo_protocol::native::session::SessionStatus;

mod acp_fs;
mod active_turn;
mod agents;
mod approval;
mod approval_checkpoint;
mod auth_notifications;
mod command_exec;
mod compaction_persist;
pub(crate) use compaction_persist::CompactionSummaryPersist;
pub(crate) use compaction_persist::append_compaction_summary_and_snapshot;
pub(crate) use compaction_persist::build_compaction_snapshot_line;
pub(crate) use compaction_persist::compaction_persisted_turn_item;
pub(crate) use compaction_persist::preserved_item_ids_from_compacted;
pub(crate) use compaction_persist::summary_item_from_compacted;
mod catalog_refresh;
pub(crate) mod compact_host;
mod connection;
pub(crate) mod context_occupancy;
mod context_usage;
mod control_requests;
mod goal_accounting;
mod goal_continuation;
mod goal_handlers;
mod handlers;
mod history_cache;
mod hooks;
mod interaction_items;
mod items;
mod kernel_host;
mod kernel_host_agent_message;
mod kernel_host_bridge;
mod kernel_host_heartbeat;
mod kernel_idle_bash;
mod lifecycle;
mod mcp;
mod model_api;
mod outbound;
mod permission_decision;
mod proposed_plan;
mod provider_api;
mod provider_discovery;
#[cfg(test)]
mod provider_persistence_tests;
pub(crate) mod python_cell_watch;
mod reference_search;
pub(crate) mod refine;
pub(crate) mod refine_planner;
mod session_actor;
mod session_cache;
mod session_deletion;
mod session_file_lock;
mod session_interactive;
mod session_title;
mod skills;
mod subagent_usage;
mod turn_exec;
mod turn_lifecycle;
mod turn_reservation;
mod user_input;
mod workspace_baseline;
mod worktrees;

pub(crate) use connection::ConnectionRuntime;
pub(crate) use connection::INBOUND_CONCURRENCY_LIMIT;
pub use connection::IncomingResponse;
pub use connection::PostResponseActions;
pub(crate) use items::render_input_items;
pub(crate) use outbound::OUTBOUND_CHANNEL_CAPACITY;
pub use outbound::OutboundFrame;
pub(crate) use outbound::enqueue_outbound;
pub(crate) use outbound::log_outbound_frame;
pub(crate) use outbound::outbound_frame_to_value;
pub use outbound::test_outbound_channel;
use session_actor::SessionHandle;
use session_interactive::SessionInteractiveLanes;
use turn_exec::ExecuteTurnRequest;

pub(crate) use session_actor::SessionActorState;

pub struct ServerRuntime {
    metadata: InitializeResult,
    protocol_exposure: ProtocolExposurePolicy,
    deps: ServerRuntimeDependencies,
    rollout_store: RolloutStore,
    history_cache: history_cache::HistoryCache,
    goal_durable_store: GoalDurableStore,
    pub(crate) usage_ledger: UsageLedger,
    /// Per-session actor handles; map lock must not be held across await.
    sessions: Mutex<HashMap<SessionId, SessionHandle>>,
    /// Interactive approval and user-input waits outside session actors.
    session_interactive: SessionInteractiveLanes,
    /// New-style (`subscription/*`) subscriptions keyed by subscription id
    /// (08 §4). Lock order: `connections` → `event_subscriptions`.
    event_subscriptions: Mutex<HashMap<String, handlers::subscription::EventSubscription>>,
    /// Count of active `SessionsByCwd` selectors; gates per-event cwd
    /// resolution during broadcast (0 = skip the lookup entirely).
    sessions_by_cwd_subscriptions: std::sync::atomic::AtomicUsize,
    /// In-flight turn execution handles keyed by session id.
    active_turns: active_turn::ActiveTurnRegistry,
    connections: Arc<Mutex<HashMap<u64, ConnectionRuntime>>>,
    terminal_turn_statuses: Mutex<VecDeque<(TurnId, TerminalTurnSnapshot)>>,
    acp_prompt_waiters: Mutex<HashMap<TurnId, Vec<oneshot::Sender<TerminalTurnSnapshot>>>>,
    active_goal_continuation_turns: Mutex<HashMap<SessionId, TurnId>>,
    goal_continuation_turn_goals: Mutex<HashMap<TurnId, GoalId>>,
    next_connection_id: AtomicU64,
    /// Per-session goal stores for goal lifecycle management.
    goal_stores: Mutex<HashMap<SessionId, GoalStore>>,
    /// Per-root-session agent registries for subagent coordination.
    agent_registries: Mutex<HashMap<SessionId, AgentRegistry>>,
    /// Per-session inboxes used by agent tools to exchange ordered messages.
    agent_mailboxes: Mutex<HashMap<SessionId, SubagentMailbox>>,
    /// Per-parent child-output buffers used by wait_agent polling.
    agent_output_buffers: Mutex<HashMap<SessionId, SubagentOutputBuffer>>,
    /// Per-parent `wait_agent` sequence cursors keyed by optional target string.
    agent_wait_cursors: Mutex<HashMap<SessionId, HashMap<String, u64>>>,
    /// Latest subagent turn usage grouped under the parent turn that requested the work.
    subagent_usage: Mutex<subagent_usage::SubagentUsageState>,
    /// Live client-owned reference search sessions.
    reference_searches: Mutex<
        HashMap<
            devo_protocol::native::rpc_search::SearchId,
            reference_search::ReferenceSearchState,
        >,
    >,
    /// Live client-owned shell/process sessions.
    command_exec_manager: command_exec::CommandExecManager,
    /// Turn-scoped workspace baselines captured at actual execution start.
    active_workspace_baselines: Mutex<HashMap<TurnId, ActiveWorkspaceBaseline>>,
    /// In-process idempotency for canonical `turn/start`
    /// (`(session, idempotencyKey) -> turn`). Retry-safe within the process;
    /// cross-restart dedup is a documented follow-up (L2-DES-APP-008 Phase B).
    recovery_idempotency:
        Mutex<HashMap<(SessionId, String), devo_protocol::native::rpc_turn::TurnResumeResult>>,
    recovery_gates: Mutex<HashMap<SessionId, Arc<Mutex<()>>>>,
    turn_start_idempotency: Mutex<HashMap<(SessionId, String), devo_protocol::native::turn::Turn>>,
    /// In-process idempotency for canonical `session/new`
    /// (`idempotencyKey -> session`). Same process-scope caveat as
    /// `turn_start_idempotency`.
    session_new_idempotency: Mutex<HashMap<String, SessionId>>,
    /// In-process idempotency for canonical `session/goal/set`
    /// (`(session, idempotencyKey) -> goal`).
    goal_set_idempotency: Mutex<HashMap<(SessionId, String), devo_protocol::native::goal::Goal>>,
    /// In-process idempotency for canonical `session/goal/update` (ratified
    /// #3 in-place edit; same keying as goal/set).
    goal_update_idempotency: Mutex<HashMap<(SessionId, String), devo_protocol::native::goal::Goal>>,
    /// In-process idempotency for canonical `task/start` (`(session,
    /// idempotencyKey) -> item id as process id`).
    task_start_idempotency: Mutex<HashMap<(SessionId, String), String>>,
    /// Short-lived, connection-bound P4d rollback plans.
    restore_plans: Mutex<handlers::rollback_plan::RestorePlanStore>,
    /// Sessions with an in-flight model title-generation task.
    title_generation_in_flight: Mutex<HashSet<SessionId>>,
    /// Sessions waiting for optional LLM title polish after a heuristic title.
    title_polish_pending: Mutex<HashMap<SessionId, TitlePolishPending>>,
    /// Cap concurrent auxiliary model calls (title polish, refine, …) to 1.
    auxiliary_model_slots: Arc<Semaphore>,
    /// Weak back-reference used when session actors need the owning runtime `Arc`.
    self_weak: std::sync::Weak<ServerRuntime>,
    /// LRU order for loaded root session actors.
    session_lru: Mutex<session_cache::ParentSessionLru>,
    /// Per-session gate that serializes lazy parent session hydration.
    parent_session_load_gate: Arc<session_cache::SessionLoadGate>,
    /// Per-session gate that serializes durable metadata reads and writes
    /// without waiting for a session actor or an active turn.
    session_metadata_write_gate: Arc<session_cache::SessionLoadGate>,
    /// User exec-policy rules loaded from `$DEVO_HOME/rules/*.rules`.
    user_exec_policy: std::sync::Mutex<Option<devo_execpolicy::Policy>>,
    /// Localhost HTTP CONNECT proxy for restricted sandbox profiles.
    sandbox_network_proxy: std::sync::Arc<
        std::sync::Mutex<Option<devo_sandbox_network_proxy::SharedSandboxNetworkProxyHandle>>,
    >,
    /// Durable cron/heartbeat jobs (`$DEVO_HOME/schedules.json`).
    schedule_store: crate::schedule_store::ScheduleStore,
    schedule_wake_cancel: CancellationToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnInputMode {
    VisibleUserMessage,
    HiddenGoalContinuation {
        goal: devo_protocol::ThreadGoal,
    },
    /// Resume a turn after interactive approval without emitting a user message.
    ApprovalResume,
    /// Continue an unexpectedly stopped turn without synthetic input.
    Recovery,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TitlePolishPending {
    pub(crate) attempts: usize,
}

const TERMINAL_TURN_STATUS_LIMIT: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalTurnSnapshot {
    pub(crate) status: TurnStatus,
    pub(crate) stop_reason: Option<devo_core::StopReason>,
    pub(crate) failure_reason: Option<devo_protocol::TurnFailureReason>,
}

impl TerminalTurnSnapshot {
    pub(crate) fn from_runtime_turn(turn: &crate::turn::RuntimeTurn) -> Self {
        Self {
            status: match turn.native.status {
                devo_protocol::native::turn::TurnStatus::InProgress => TurnStatus::Running,
                devo_protocol::native::turn::TurnStatus::WaitingApproval => {
                    TurnStatus::WaitingApproval
                }
                devo_protocol::native::turn::TurnStatus::Completed => TurnStatus::Completed,
                devo_protocol::native::turn::TurnStatus::Interrupted => TurnStatus::Interrupted,
                devo_protocol::native::turn::TurnStatus::Failed => TurnStatus::Failed,
            },
            stop_reason: turn.extras.stop_reason.clone(),
            failure_reason: turn.extras.failure_reason,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnStartQueuePolicy {
    RejectActive,
    /// Accept a busy session by materializing the input into its Native
    /// session queue. This is used only by `session/queue/push` after the
    /// request has been decoded on the canonical protocol surface.
    Queue,
}

impl TurnInputMode {
    fn emits_user_message(&self) -> bool {
        matches!(self, Self::VisibleUserMessage)
    }
}

trait SessionModelSelection {
    fn model_selection(&self) -> Option<&str>;
}

impl SessionModelSelection for crate::runtime_session_summary::RuntimeSessionSummary {
    fn model_selection(&self) -> Option<&str> {
        crate::runtime_session_summary::RuntimeSessionSummary::model_selection(self)
    }
}

fn session_model_selection(session: &impl SessionModelSelection) -> Option<&str> {
    session.model_selection()
}

fn requested_model_selection<'a>(
    model_binding_id: Option<&'a str>,
    model: Option<&'a str>,
    session: &'a impl SessionModelSelection,
) -> Option<&'a str> {
    model_binding_id
        .or(model)
        .or_else(|| session_model_selection(session))
}

const SUBAGENT_USAGE_PARENT_SESSION_ID_METADATA: &str = "devo_subagent_usage_parent_session_id";
const SUBAGENT_USAGE_PARENT_TURN_ID_METADATA: &str = "devo_subagent_usage_parent_turn_id";

pub(super) fn subagent_usage_owner_pending_metadata(
    parent_session_id: SessionId,
    parent_turn_id: Option<TurnId>,
) -> serde_json::Value {
    serde_json::json!({
        SUBAGENT_USAGE_PARENT_SESSION_ID_METADATA: parent_session_id.to_string(),
        SUBAGENT_USAGE_PARENT_TURN_ID_METADATA: parent_turn_id.map(|turn_id| turn_id.to_string()),
    })
}

impl ServerRuntime {
    pub fn new(server_home: PathBuf, deps: ServerRuntimeDependencies) -> Arc<Self> {
        // Embedded callers historically exposed both surfaces. The server
        // process always calls `with_protocols` with its explicit startup
        // policy, whose CLI default is Native-only.
        Self::with_protocols(server_home, deps, ProtocolSet::all())
    }

    pub fn with_protocols(
        server_home: PathBuf,
        deps: ServerRuntimeDependencies,
        protocols: ProtocolSet,
    ) -> Arc<Self> {
        let rollout_store = RolloutStore::new(server_home.clone(), Some(Arc::clone(&deps.db)));
        let goal_durable_store = GoalDurableStore::with_primary(
            server_home.clone(),
            rollout_store.clone(),
            Arc::clone(&deps.db),
        );
        let usage_ledger = UsageLedger::new(rollout_store.clone(), Arc::clone(&deps.db));
        let sandbox_network_proxy = std::sync::Arc::new(std::sync::Mutex::new(None));
        // Proxy startup is async; ports are published via the thread-safe
        // `set_sandbox_proxy_ports` store (not process-wide `env::set_var`).
        if let Ok(runtime_handle) = tokio::runtime::Handle::try_current() {
            let proxy_slot = sandbox_network_proxy.clone();
            runtime_handle.spawn(async move {
                match devo_sandbox_network_proxy::start_sandbox_network_proxy().await {
                    Ok(handle) => {
                        tracing::info!(
                            http_port = handle.http_port(),
                            "started sandbox network proxy"
                        );
                        *proxy_slot.lock().expect("proxy slot mutex") =
                            Some(std::sync::Arc::new(handle));
                    }
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "failed to start sandbox network proxy; restricted profiles keep network isolation without managed proxy egress"
                        );
                    }
                }
            });
        } else {
            tracing::warn!(
                "no tokio runtime available at ServerRuntime::new; sandbox network proxy not started"
            );
        }
        let runtime = Arc::new_cyclic(|self_weak| Self {
            metadata: InitializeResult {
                server_name: "devo-server".into(),
                server_version: env!("CARGO_PKG_VERSION").into(),
                platform_family: std::env::consts::FAMILY.into(),
                platform_os: std::env::consts::OS.into(),
                server_home: server_home.clone(),
            },
            protocol_exposure: ProtocolExposurePolicy::new(protocols),
            deps,
            rollout_store,
            history_cache: history_cache::HistoryCache::default(),
            goal_durable_store,
            usage_ledger,
            sessions: Mutex::new(HashMap::new()),
            session_interactive: SessionInteractiveLanes::default(),
            event_subscriptions: Mutex::new(HashMap::new()),
            sessions_by_cwd_subscriptions: std::sync::atomic::AtomicUsize::new(0),
            active_turns: active_turn::ActiveTurnRegistry::default(),
            connections: Arc::new(Mutex::new(HashMap::new())),
            terminal_turn_statuses: Mutex::new(VecDeque::new()),
            acp_prompt_waiters: Mutex::new(HashMap::new()),
            active_goal_continuation_turns: Mutex::new(HashMap::new()),
            goal_continuation_turn_goals: Mutex::new(HashMap::new()),
            next_connection_id: AtomicU64::new(1),
            goal_stores: Mutex::new(HashMap::new()),
            agent_registries: Mutex::new(HashMap::new()),
            agent_mailboxes: Mutex::new(HashMap::new()),
            agent_output_buffers: Mutex::new(HashMap::new()),
            agent_wait_cursors: Mutex::new(HashMap::new()),
            subagent_usage: Mutex::new(subagent_usage::SubagentUsageState::default()),
            reference_searches: Mutex::new(HashMap::new()),
            command_exec_manager: command_exec::CommandExecManager::new(),
            active_workspace_baselines: Mutex::new(HashMap::new()),
            recovery_idempotency: Mutex::new(HashMap::new()),
            recovery_gates: Mutex::new(HashMap::new()),
            turn_start_idempotency: Mutex::new(HashMap::new()),
            session_new_idempotency: Mutex::new(HashMap::new()),
            goal_set_idempotency: Mutex::new(HashMap::new()),
            goal_update_idempotency: Mutex::new(HashMap::new()),
            task_start_idempotency: Mutex::new(HashMap::new()),
            restore_plans: Mutex::new(HashMap::new()),
            title_generation_in_flight: Mutex::new(HashSet::new()),
            title_polish_pending: Mutex::new(HashMap::new()),
            auxiliary_model_slots: Arc::new(Semaphore::new(1)),
            self_weak: self_weak.clone(),
            session_lru: Mutex::new(session_cache::ParentSessionLru::new(
                session_cache::PARENT_SESSION_LRU_CAPACITY,
            )),
            parent_session_load_gate: Arc::new(session_cache::SessionLoadGate::default()),
            session_metadata_write_gate: Arc::new(session_cache::SessionLoadGate::default()),
            user_exec_policy: std::sync::Mutex::new(
                crate::exec_policy_store::load_user_exec_policy(),
            ),
            sandbox_network_proxy,
            schedule_store: crate::schedule_store::ScheduleStore::new(server_home),
            schedule_wake_cancel: CancellationToken::new(),
        });
        runtime.start_schedule_wake_loop();
        runtime
    }

    pub async fn enabled_protocols(&self) -> ProtocolSet {
        self.protocol_exposure.enabled().await
    }

    pub async fn protocol_enabled(&self, protocol: ServerProtocol) -> bool {
        self.protocol_exposure.allows(protocol).await
    }

    pub async fn enable_protocols(&self, protocols: &ProtocolSet) -> ProtocolSet {
        self.protocol_exposure.enable(protocols).await
    }

    pub fn sandbox_network_proxy(
        &self,
    ) -> Option<devo_sandbox_network_proxy::SharedSandboxNetworkProxyHandle> {
        self.sandbox_network_proxy
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    /// The rollout store, for the startup event-log reconciler (08 §7).
    pub(crate) fn rollout_store(&self) -> RolloutStore {
        self.rollout_store.clone()
    }

    /// The shared SQLite handle (session index, queues, event log).
    pub(crate) fn deps_db(&self) -> Arc<crate::db::Database> {
        Arc::clone(&self.deps.db)
    }
}

fn permission_mode_from_approval_policy(policy: &str) -> Option<PermissionMode> {
    match policy {
        "on-request" | "interactive" | "ask" => Some(PermissionMode::Interactive),
        "never" | "auto" | "auto-approve" | "yolo" => Some(PermissionMode::Yolo),
        "deny" => Some(PermissionMode::Deny),
        _ => None,
    }
}

fn safety_profile_from_protocol(
    preset: devo_protocol::PermissionPreset,
    cwd: std::path::PathBuf,
    additional_directories: Vec<std::path::PathBuf>,
) -> devo_safety::RuntimePermissionProfile {
    let preset = match preset {
        devo_protocol::PermissionPreset::Default => devo_safety::PermissionPreset::Default,
        devo_protocol::PermissionPreset::AutoReview => devo_safety::PermissionPreset::AutoReview,
        devo_protocol::PermissionPreset::FullAccess => devo_safety::PermissionPreset::FullAccess,
    };
    devo_safety::RuntimePermissionProfile::from_preset(preset, cwd)
        .with_additional_roots(additional_directories)
}

pub(crate) fn protocol_preset_from_safety(
    preset: devo_safety::PermissionPreset,
) -> devo_protocol::PermissionPreset {
    match preset {
        devo_safety::PermissionPreset::Default => devo_protocol::PermissionPreset::Default,
        devo_safety::PermissionPreset::AutoReview => devo_protocol::PermissionPreset::AutoReview,
        devo_safety::PermissionPreset::FullAccess => devo_protocol::PermissionPreset::FullAccess,
    }
}
