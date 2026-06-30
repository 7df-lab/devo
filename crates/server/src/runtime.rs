use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use chrono::Utc;
use futures::FutureExt;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use devo_core::ApprovalDecisionItem;
use devo_core::CommandExecutionItem;
use devo_core::ItemId;
use devo_core::Message;
use devo_core::QueryEvent;
use devo_core::ResearchArtifactItem;
use devo_core::ResearchArtifactType;
use devo_core::ResponseItem;
use devo_core::SessionId;
use devo_core::SessionTitleFinalSource;
use devo_core::SessionTitleState;
use devo_core::TextItem;
use devo_core::TokenInfo;
use devo_core::ToolCallItem;
use devo_core::ToolResultItem;
use devo_core::TurnConfig;
use devo_core::TurnId;
use devo_core::TurnItem;
use devo_core::TurnStatus;
use devo_core::TurnUsage;
use devo_core::Worklog;
use devo_core::history::compaction::CompactAction;
use devo_core::history::compaction::CompactionConfig;
use devo_core::history::compaction::CompactionKind;
use devo_core::history::compaction::compact_history;
use devo_core::history::summarizer::DefaultHistorySummarizer;
use devo_core::message_to_response_items;
use devo_core::query;
use devo_core::tools::AgentToolCoordinator;
use devo_core::tools::ClientFilesystem;
use devo_core::tools::ClientTerminal;
use devo_core::tools::PermissionChecker;
use devo_core::tools::ToolAgentScope;
use devo_core::tools::ToolCall;
use devo_core::tools::ToolCallError;
use devo_core::tools::ToolCallResult;
use devo_core::tools::ToolContent;
use devo_core::tools::ToolExecutionOptions;
use devo_core::tools::ToolPermissionRequest;
use devo_core::tools::ToolRegistry;
use devo_core::tools::ToolRuntime;
use devo_core::tools::ToolRuntimeContext;
use devo_protocol::{
    SessionDeletedPayload, WorkspaceChangeAttribution, WorkspaceChangeScope, WorkspaceChangeView,
    WorkspaceChangesReadParams, WorkspaceChangesReadResult, WorkspaceChangesUpdatedPayload,
    WorkspaceDiffDetail,
};
use devo_safety::PermissionMode;
use devo_util_shell_command::parse_command::parse_command;

use crate::ApprovalDecisionValue;
use crate::ApprovalScopeValue;
use crate::ClientMethod;
use crate::ClientTransportKind;
use crate::CommandExecutionPayload;
use crate::ConnectionState;
use crate::ErrorResponse;
use crate::EventContext;
use crate::EventsSubscribeParams;
use crate::EventsSubscribeResult;
use crate::InitializeResult;
use crate::ItemDeltaKind;
use crate::ItemDeltaPayload;
use crate::ItemEnvelope;
use crate::ItemEventPayload;
use crate::ItemKind;
use crate::ProtocolError;
use crate::ProtocolErrorCode;
use crate::RequestUserInputArgs;
use crate::RequestUserInputPayload;
use crate::RequestUserInputRespondParams;
use crate::RequestUserInputResponse;
use crate::ServerEvent;
use crate::ServerRequestResolvedPayload;
use crate::SessionCompactParams;
use crate::SessionCompactResult;
use crate::SessionCompactionFailedPayload;
use crate::SessionEventPayload;
use crate::SessionForkParams;
use crate::SessionForkResult;
use crate::SessionMetadata;
use crate::SessionMetadataUpdateParams;
use crate::SessionMetadataUpdateResult;
use crate::SessionPermissionsUpdateParams;
use crate::SessionPermissionsUpdateResult;
use crate::SessionResumeParams;
use crate::SessionResumeResult;
use crate::SessionRollbackMode;
use crate::SessionRollbackParams;
use crate::SessionRollbackResult;
use crate::SessionRuntimeStatus;
use crate::SessionStartParams;
use crate::SessionStartResult;
use crate::SessionStatusChangedPayload;
use crate::SessionTitleUpdateParams;
use crate::SessionTitleUpdateResult;
use crate::ShellCommandParams;
use crate::ShellCommandResult;
use crate::SuccessResponse;
use crate::ToolCallPayload;
use crate::ToolResultPayload;
use crate::TurnEventPayload;
use crate::TurnInterruptParams;
use crate::TurnInterruptResult;
use crate::TurnMetadata;
use crate::TurnStartParams;
use crate::TurnStartResult;
use crate::TurnSteerParams;
use crate::TurnSteerResult;
use crate::TurnUsageUpdatedPayload;
use crate::approval_reviewer::ReviewerDecision;
use crate::approval_reviewer::build_approval_review_request;
use crate::approval_reviewer::parse_reviewer_decision;
use crate::db::QueueType;
use crate::db::SessionStats;
use crate::execution::PendingApproval;
use crate::execution::PendingUserInput;
use crate::execution::RuntimeSession;
use crate::execution::ServerRuntimeDependencies;
use crate::goal::Goal;
use crate::goal::GoalAction;
use crate::goal::GoalId;
use crate::goal::GoalMutation;
use crate::goal_durable::GoalDurableStore;
use crate::persistence::RolloutStore;
use crate::persistence::build_item_record;
use crate::persistence::build_turn_record;
use crate::projection::history_item_from_turn_item;
pub(crate) use crate::runtime::handlers::goal::GoalStore;
use crate::subagent::AgentPath;
use crate::subagent::AgentRegistry;
use crate::subagent::SubagentMailbox;
use crate::subagent::SubagentMetadata;
use crate::subagent::SubagentOutputBuffer;
use crate::subagent::SubagentStatus;
use crate::workspace_changes::ActiveWorkspaceBaseline;

mod acp_fs;
mod acp_terminal;
mod agents;
mod approval;
mod command_exec;
mod connection;
mod goal_accounting;
mod goal_continuation;
mod goal_handlers;
mod handlers;
mod hooks;
mod items;
mod lifecycle;
mod model_api;
mod proposed_plan;
mod provider_vendor_api;
mod reference_search;
mod research;
mod research_capture;
mod research_child_agents;
mod research_context;
mod research_events;
mod research_final_report;
mod research_formatting;
mod research_parsing;
mod research_session;
mod research_stages;
mod research_streaming;
mod research_tool_runtime;
mod research_tools;
mod skills;
mod subagent_usage;
mod turn_exec;
mod turn_reservation;
mod user_input;
mod workspace_baseline;

pub(crate) use connection::CONNECTION_NOTIFICATION_CHANNEL_CAPACITY;
pub(crate) use connection::ConnectionRuntime;
pub use connection::IncomingResponse;
pub use connection::PostResponseActions;
pub(crate) use connection::SubscriptionFilter;
pub(crate) use items::render_input_items;
pub(crate) use research_tools::extract_written_file_path;
pub(crate) use research_tools::is_write_tool_name;
use turn_exec::ExecuteTurnRequest;

pub struct ServerRuntime {
    metadata: InitializeResult,
    deps: ServerRuntimeDependencies,
    rollout_store: RolloutStore,
    goal_durable_store: GoalDurableStore,
    /// Thread safe hashmap as sessions container, there are allowed multiple sessions.
    sessions: Mutex<HashMap<SessionId, Arc<Mutex<RuntimeSession>>>>,
    connections: Arc<Mutex<HashMap<u64, ConnectionRuntime>>>,
    active_tasks: Mutex<HashMap<SessionId, tokio::task::AbortHandle>>,
    active_turn_cancellations: Mutex<HashMap<SessionId, CancellationToken>>,
    active_turn_connections: Mutex<HashMap<SessionId, u64>>,
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
    /// Child agents owned by an active `/research` pipeline.
    research_child_agents: Mutex<HashMap<SessionId, HashSet<SessionId>>>,
    /// Latest subagent turn usage grouped under the parent turn that requested the work.
    subagent_usage: Mutex<subagent_usage::SubagentUsageState>,
    /// Live client-owned reference search sessions.
    reference_searches:
        Mutex<HashMap<devo_protocol::ReferenceSearchId, reference_search::ReferenceSearchState>>,
    /// Live client-owned shell/process sessions.
    command_exec_manager: command_exec::CommandExecManager,
    /// Turn-scoped workspace baselines captured at actual execution start.
    active_workspace_baselines: Mutex<HashMap<TurnId, ActiveWorkspaceBaseline>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TurnInputMode {
    VisibleUserMessage,
    HiddenGoalContinuation { goal: devo_protocol::ThreadGoal },
}

const TERMINAL_TURN_STATUS_LIMIT: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalTurnSnapshot {
    status: TurnStatus,
    stop_reason: Option<devo_core::StopReason>,
    failure_reason: Option<devo_protocol::TurnFailureReason>,
}

impl TerminalTurnSnapshot {
    fn from_turn(turn: &TurnMetadata) -> Self {
        Self {
            status: turn.status.clone(),
            stop_reason: turn.stop_reason.clone(),
            failure_reason: turn.failure_reason,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnStartQueuePolicy {
    Queue,
    RejectActive,
}

impl TurnInputMode {
    fn emits_user_message(&self) -> bool {
        matches!(self, Self::VisibleUserMessage)
    }
}

fn session_model_selection(session: &SessionMetadata) -> Option<&str> {
    session
        .model_binding_id
        .as_deref()
        .or(session.model.as_deref())
}

fn requested_model_selection<'a>(
    model_binding_id: Option<&'a str>,
    model: Option<&'a str>,
    session: &'a SessionMetadata,
) -> Option<&'a str> {
    model_binding_id
        .or(model)
        .or_else(|| session_model_selection(session))
}

fn apply_turn_config_to_session_summary(summary: &mut SessionMetadata, turn_config: &TurnConfig) {
    summary.model = Some(turn_config.model.slug.clone());
    summary.model_binding_id = turn_config.model_binding_id.clone();
    summary.reasoning_effort_selection = turn_config.reasoning_effort_selection.clone();
}

fn string_field_from_pending_metadata(
    metadata: Option<&serde_json::Value>,
    key: &str,
) -> Option<String> {
    metadata?
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn model_selection_from_pending_metadata(metadata: Option<&serde_json::Value>) -> Option<String> {
    string_field_from_pending_metadata(metadata, "model_binding_id")
        .or_else(|| string_field_from_pending_metadata(metadata, "model"))
}

const SUBAGENT_USAGE_PARENT_SESSION_ID_METADATA: &str = "devo_subagent_usage_parent_session_id";
const SUBAGENT_USAGE_PARENT_TURN_ID_METADATA: &str = "devo_subagent_usage_parent_turn_id";

fn subagent_usage_owner_pending_metadata(
    parent_session_id: SessionId,
    parent_turn_id: Option<TurnId>,
) -> serde_json::Value {
    serde_json::json!({
        SUBAGENT_USAGE_PARENT_SESSION_ID_METADATA: parent_session_id.to_string(),
        SUBAGENT_USAGE_PARENT_TURN_ID_METADATA: parent_turn_id.map(|turn_id| turn_id.to_string()),
    })
}

fn subagent_usage_owner_from_pending_metadata(
    metadata: Option<&serde_json::Value>,
) -> Option<(SessionId, Option<TurnId>)> {
    let parent_session_id =
        string_field_from_pending_metadata(metadata, SUBAGENT_USAGE_PARENT_SESSION_ID_METADATA)
            .and_then(|value| SessionId::try_from(value).ok())?;
    let parent_turn_id =
        string_field_from_pending_metadata(metadata, SUBAGENT_USAGE_PARENT_TURN_ID_METADATA)
            .and_then(|value| TurnId::try_from(value).ok());
    Some((parent_session_id, parent_turn_id))
}

impl ServerRuntime {
    pub fn new(server_home: PathBuf, deps: ServerRuntimeDependencies) -> Arc<Self> {
        let rollout_store = RolloutStore::new(server_home.clone());
        let goal_durable_store = GoalDurableStore::new(server_home.clone());
        Arc::new(Self {
            metadata: InitializeResult {
                server_name: "devo-server".into(),
                server_version: env!("CARGO_PKG_VERSION").into(),
                platform_family: std::env::consts::FAMILY.into(),
                platform_os: std::env::consts::OS.into(),
                server_home,
            },
            deps,
            rollout_store,
            goal_durable_store,
            sessions: Mutex::new(HashMap::new()),
            connections: Arc::new(Mutex::new(HashMap::new())),
            active_tasks: Mutex::new(HashMap::new()),
            active_turn_cancellations: Mutex::new(HashMap::new()),
            active_turn_connections: Mutex::new(HashMap::new()),
            terminal_turn_statuses: Mutex::new(VecDeque::new()),
            acp_prompt_waiters: Mutex::new(HashMap::new()),
            active_goal_continuation_turns: Mutex::new(HashMap::new()),
            goal_continuation_turn_goals: Mutex::new(HashMap::new()),
            next_connection_id: AtomicU64::new(1),
            goal_stores: Mutex::new(HashMap::new()),
            agent_registries: Mutex::new(HashMap::new()),
            agent_mailboxes: Mutex::new(HashMap::new()),
            agent_output_buffers: Mutex::new(HashMap::new()),
            research_child_agents: Mutex::new(HashMap::new()),
            subagent_usage: Mutex::new(subagent_usage::SubagentUsageState::default()),
            reference_searches: Mutex::new(HashMap::new()),
            command_exec_manager: command_exec::CommandExecManager::new(),
            active_workspace_baselines: Mutex::new(HashMap::new()),
        })
    }
}

fn permission_mode_from_approval_policy(policy: &str) -> Option<PermissionMode> {
    match policy {
        "on-request" | "interactive" | "ask" => Some(PermissionMode::Interactive),
        "never" | "auto" | "auto-approve" => Some(PermissionMode::AutoApprove),
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
        devo_protocol::PermissionPreset::ReadOnly => devo_safety::PermissionPreset::ReadOnly,
        devo_protocol::PermissionPreset::Default => devo_safety::PermissionPreset::Default,
        devo_protocol::PermissionPreset::AutoReview => devo_safety::PermissionPreset::AutoReview,
        devo_protocol::PermissionPreset::FullAccess => devo_safety::PermissionPreset::FullAccess,
    };
    devo_safety::RuntimePermissionProfile::from_preset(preset, cwd)
        .with_additional_roots(additional_directories)
}

fn protocol_reviewer_from_safety(
    reviewer: devo_safety::ApprovalsReviewer,
) -> devo_protocol::ApprovalsReviewer {
    match reviewer {
        devo_safety::ApprovalsReviewer::User => devo_protocol::ApprovalsReviewer::User,
        devo_safety::ApprovalsReviewer::AutoReview => devo_protocol::ApprovalsReviewer::AutoReview,
    }
}
