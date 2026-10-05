//! First-party bus helpers for [`ServerNotification`] emit (L2-DES-APP-009).
//!
//! Native-covered lifecycle notifications ride the bus as
//! [`ServerNotification`] directly. The ACP adapter projects from that shape
//! at the connection boundary; first-party Native clients consume identity.

use super::event::ServerNotification;
use super::ids::SessionId;
use super::wire_projector::wire_from_server_notification;

/// Session id for subscription filters / activity stamps (opaque Native id).
pub fn notification_legacy_session_id(
    notification: &ServerNotification,
) -> Option<crate::SessionId> {
    notification_native_session_id(notification)
}

/// Native session id carried by a live notification, when present.
pub fn notification_native_session_id(notification: &ServerNotification) -> Option<SessionId> {
    match notification {
        ServerNotification::SessionCreated { session }
        | ServerNotification::SessionMetadataUpdated { session } => Some(session.id),
        ServerNotification::SessionCwdChanged { session_id, .. }
        | ServerNotification::SessionStatusChanged { session_id, .. }
        | ServerNotification::SessionArchived { session_id, .. }
        | ServerNotification::SessionClosed { session_id }
        | ServerNotification::SessionDeleted { session_id, .. }
        | ServerNotification::WorkspaceRestoreStarted { session_id, .. }
        | ServerNotification::WorkspaceRestoreCompleted { session_id, .. }
        | ServerNotification::QueueUpdated { session_id, .. }
        | ServerNotification::GoalStatusChanged { session_id, .. }
        | ServerNotification::GoalCleared { session_id, .. }
        | ServerNotification::ModelQueryFailed { session_id, .. }
        | ServerNotification::ModelQueryRetrying { session_id, .. }
        | ServerNotification::TurnUsageUpdated { session_id, .. }
        | ServerNotification::TurnSuperseded { session_id, .. }
        | ServerNotification::ContextUsageUpdated { session_id, .. }
        | ServerNotification::ContextCompactionStarted { session_id, .. }
        | ServerNotification::ContextCompactionCompleted { session_id, .. }
        | ServerNotification::ContextCompactionFailed { session_id, .. }
        | ServerNotification::SessionUsageUpdated { session_id, .. }
        | ServerNotification::PermissionDecision { session_id, .. }
        | ServerNotification::TurnRecoveryUpdated { session_id, .. }
        | ServerNotification::ToolCallStatusUpdated { session_id, .. }
        | ServerNotification::MessageEditRecorded { session_id, .. }
        | ServerNotification::ServerRequestResolved { session_id, .. } => Some(*session_id),
        ServerNotification::WorkspaceChangesUpdated(payload) => Some(payload.session_id),
        ServerNotification::TurnResumed { turn, .. }
        | ServerNotification::TurnStarted { turn }
        | ServerNotification::TurnCompleted { turn } => Some(turn.session_id),
        ServerNotification::ItemStarted { item }
        | ServerNotification::ItemUpdated { item }
        | ServerNotification::ItemCompleted { item } => Some(item.session_id),
        ServerNotification::ItemAssistantMessageDelta(delta)
        | ServerNotification::ItemReasoningDelta(delta)
        | ServerNotification::ItemCommandExecutionOutputDelta(delta)
        | ServerNotification::ItemToolCallInputDelta(delta)
        | ServerNotification::ItemPlanDelta(delta) => Some(delta.session_id),
        ServerNotification::GoalCreated { goal } | ServerNotification::GoalUpdated { goal } => {
            Some(goal.session_id)
        }
        ServerNotification::RequestUserInput { request, .. } => Some(request.session_id),
        ServerNotification::CommandExecOutputDelta { session_id, .. }
        | ServerNotification::CommandExecExited { session_id, .. } => *session_id,
        ServerNotification::SessionScheduleChanged { session_id, .. } => *session_id,
        ServerNotification::Initialized { .. }
        | ServerNotification::RuntimeWarning { .. }
        | ServerNotification::RuntimeShutdown { .. }
        | ServerNotification::TurnStatusChanged { .. }
        | ServerNotification::ProviderAuthStale { .. }
        | ServerNotification::TaskStarted { .. }
        | ServerNotification::TaskDelta { .. }
        | ServerNotification::TaskCompleted { .. }
        | ServerNotification::TaskLost { .. }
        | ServerNotification::AgentStarted { .. }
        | ServerNotification::AgentProgress { .. }
        | ServerNotification::AgentCompleted { .. }
        | ServerNotification::SecurityAlert { .. }
        | ServerNotification::CredentialChanged { .. }
        | ServerNotification::SearchUpdated(_)
        | ServerNotification::SearchCompleted(_)
        | ServerNotification::SearchFailed(_) => None,
    }
}

/// Wire method name for subscription filters (same strings as Native wire).
pub fn notification_method_name(notification: &ServerNotification) -> String {
    wire_from_server_notification(notification).0
}

/// Whether this notification should touch session last-activity.
pub fn notification_touches_session_activity(notification: &ServerNotification) -> bool {
    match notification {
        ServerNotification::ItemAssistantMessageDelta(_)
        | ServerNotification::ItemReasoningDelta(_)
        | ServerNotification::ToolCallStatusUpdated { .. } => true,
        ServerNotification::ItemStarted { item }
            if matches!(
                &item.item,
                super::item::Item::ToolCall { .. } | super::item::Item::CommandExecution { .. }
            ) =>
        {
            true
        }
        ServerNotification::ItemCompleted { item }
            if matches!(
                &item.item,
                super::item::Item::UserMessage { .. }
                    | super::item::Item::ToolResult { .. }
                    | super::item::Item::CommandExecution { .. }
                    | super::item::Item::FileChange { .. }
            ) =>
        {
            true
        }
        _ => false,
    }
}
