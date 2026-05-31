use std::collections::HashMap;
use std::collections::HashSet;
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

use devo_core::ApprovalDecisionItem;
use devo_core::ApprovalRequestItem;
use devo_core::CommandExecutionItem;
use devo_core::ItemId;
use devo_core::Message;
use devo_core::QueryEvent;
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
use devo_core::tools::PermissionChecker;
use devo_core::tools::ToolPermissionRequest;
use devo_core::tools::ToolRuntime;
use devo_core::tools::ToolRuntimeContext;
use devo_safety::PermissionMode;

use crate::ApprovalDecisionValue;
use crate::ApprovalRequestPayload;
use crate::ApprovalRespondParams;
use crate::ApprovalScopeValue;
use crate::ClientMethod;
use crate::ClientTransportKind;
use crate::CommandExecutionPayload;
use crate::ConnectionState;
use crate::ErrorResponse;
use crate::EventContext;
use crate::EventsSubscribeParams;
use crate::EventsSubscribeResult;
use crate::InitializeParams;
use crate::InitializeResult;
use crate::ItemDeltaKind;
use crate::ItemDeltaPayload;
use crate::ItemEnvelope;
use crate::ItemEventPayload;
use crate::ItemKind;
use crate::NotificationEnvelope;
use crate::ProtocolError;
use crate::ProtocolErrorCode;
use crate::ServerEvent;
use crate::ServerRequestResolvedPayload;
use crate::SessionCompactParams;
use crate::SessionCompactResult;
use crate::SessionCompactionFailedPayload;
use crate::SessionEventPayload;
use crate::SessionForkParams;
use crate::SessionForkResult;
use crate::SessionListParams;
use crate::SessionListResult;
use crate::SessionMetadataUpdateParams;
use crate::SessionMetadataUpdateResult;
use crate::SessionPermissionsUpdateParams;
use crate::SessionPermissionsUpdateResult;
use crate::SessionResumeParams;
use crate::SessionResumeResult;
use crate::SessionRollbackParams;
use crate::SessionRollbackResult;
use crate::SessionRuntimeStatus;
use crate::SessionStartParams;
use crate::SessionStartResult;
use crate::SessionStatusChangedPayload;
use crate::SessionTitleUpdateParams;
use crate::SessionTitleUpdateResult;
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
use crate::execution::RuntimeSession;
use crate::execution::ServerRuntimeDependencies;
use crate::goal::CreateGoalParams;
use crate::goal::GoalAction;
use crate::goal::GoalId;
use crate::goal::GoalMutation;
use crate::persistence::RolloutStore;
use crate::persistence::build_item_record;
use crate::persistence::build_turn_record;
use crate::projection::history_item_from_turn_item;
use crate::runtime::handlers::goal::GoalProjection;
use crate::runtime::handlers::goal::GoalStore;
use crate::subagent::AgentRegistry;
use crate::subagent::SpawnAgentParams;
use crate::subagent::SubagentMetadata;
use crate::subagent::SubagentStatus;
use crate::titles::build_title_generation_request;
use crate::titles::derive_provisional_title;
use crate::titles::normalize_generated_title;

mod agents;
mod approval;
mod connection;
mod goal_handlers;
mod handlers;
mod items;
mod lifecycle;
mod model_api;
mod provider_vendor_api;
mod skills;
mod turn_exec;

pub(crate) use connection::ConnectionRuntime;
pub(crate) use connection::SubscriptionFilter;
pub(crate) use items::render_input_items;

pub struct ServerRuntime {
    metadata: InitializeResult,
    deps: ServerRuntimeDependencies,
    rollout_store: RolloutStore,
    /// Thread safe hashmap as sessions container, there are allowed multiple sessions.
    sessions: Mutex<HashMap<SessionId, Arc<Mutex<RuntimeSession>>>>,
    connections: Mutex<HashMap<u64, ConnectionRuntime>>,
    active_tasks: Mutex<HashMap<SessionId, tokio::task::AbortHandle>>,
    next_connection_id: AtomicU64,
    /// Per-session goal stores for goal lifecycle management.
    goal_stores: Mutex<HashMap<SessionId, GoalStore>>,
    /// Per-root-session agent registries for subagent coordination.
    agent_registries: Mutex<HashMap<SessionId, AgentRegistry>>,
}

impl ServerRuntime {
    pub fn new(server_home: PathBuf, deps: ServerRuntimeDependencies) -> Arc<Self> {
        let rollout_store = RolloutStore::new(server_home.clone());
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
            sessions: Mutex::new(HashMap::new()),
            connections: Mutex::new(HashMap::new()),
            active_tasks: Mutex::new(HashMap::new()),
            next_connection_id: AtomicU64::new(1),
            goal_stores: Mutex::new(HashMap::new()),
            agent_registries: Mutex::new(HashMap::new()),
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
) -> devo_safety::RuntimePermissionProfile {
    let preset = match preset {
        devo_protocol::PermissionPreset::ReadOnly => devo_safety::PermissionPreset::ReadOnly,
        devo_protocol::PermissionPreset::Default => devo_safety::PermissionPreset::Default,
        devo_protocol::PermissionPreset::AutoReview => devo_safety::PermissionPreset::AutoReview,
        devo_protocol::PermissionPreset::FullAccess => devo_safety::PermissionPreset::FullAccess,
    };
    devo_safety::RuntimePermissionProfile::from_preset(preset, cwd)
}

fn protocol_reviewer_from_safety(
    reviewer: devo_safety::ApprovalsReviewer,
) -> devo_protocol::ApprovalsReviewer {
    match reviewer {
        devo_safety::ApprovalsReviewer::User => devo_protocol::ApprovalsReviewer::User,
        devo_safety::ApprovalsReviewer::AutoReview => devo_protocol::ApprovalsReviewer::AutoReview,
    }
}
