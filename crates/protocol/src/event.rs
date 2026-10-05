use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use ts_rs::TS;

use crate::parse_command::ParsedCommand;
use crate::protocol::ExecCommandSource;
use crate::{ItemId, SessionId, TurnId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallPayload {
    pub tool_call_id: String,
    pub tool_name: String,
    pub parameters: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command_actions: Vec<ParsedCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultPayload {
    pub tool_call_id: String,
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    pub content: serde_json::Value,
    /// Optional UI-facing rendering of `content`.
    ///
    /// `content` remains the canonical protocol payload; this field lets clients
    /// show a compact version without losing the original result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_content: Option<String>,
    pub is_error: bool,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandExecutionPayload {
    pub tool_call_id: String,
    pub tool_name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    #[serde(default)]
    pub source: ExecCommandSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command_actions: Vec<ParsedCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnEventPayload {
    pub session_id: SessionId,
    pub turn: crate::native::turn::Turn,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TurnFailedPayload {
    pub session_id: SessionId,
    /// Native turn with `error` stamped at emit (no sidecar failure bag).
    pub turn: crate::native::turn::Turn,
}

/// Emit-site discriminator for Native item text/output deltas.
///
/// Maps 1:1 onto [`crate::native::event::ServerNotification`] item-delta
/// variants via [`item_delta_notification`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemDeltaKind {
    AgentMessageDelta,
    ReasoningSummaryTextDelta,
    ReasoningTextDelta,
    CommandExecutionOutputDelta,
    PlanDelta,
    ToolCallInputDelta,
}

/// Build a Native bus item-delta notification from Native opaque ids.
pub fn item_delta_notification(
    kind: ItemDeltaKind,
    session_id: crate::native::ids::SessionId,
    item_id: crate::native::ids::ItemId,
    chunk_index: u64,
    delta: impl Into<String>,
) -> crate::native::event::ServerNotification {
    let delta = crate::native::event::ItemDelta {
        item_id,
        session_id,
        // Text deltas apply to the item birth snapshot; `item/updated`
        // revisions are not emitted for them yet.
        base_revision: 1,
        chunk_index,
        delta: delta.into(),
    };
    match kind {
        ItemDeltaKind::AgentMessageDelta => {
            crate::native::event::ServerNotification::ItemAssistantMessageDelta(delta)
        }
        ItemDeltaKind::ReasoningSummaryTextDelta | ItemDeltaKind::ReasoningTextDelta => {
            crate::native::event::ServerNotification::ItemReasoningDelta(delta)
        }
        ItemDeltaKind::CommandExecutionOutputDelta => {
            crate::native::event::ServerNotification::ItemCommandExecutionOutputDelta(delta)
        }
        ItemDeltaKind::ToolCallInputDelta => {
            crate::native::event::ServerNotification::ItemToolCallInputDelta(delta)
        }
        ItemDeltaKind::PlanDelta => crate::native::event::ServerNotification::ItemPlanDelta(delta),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerRequestKind {
    ItemCommandExecutionRequestApproval,
    ItemFileChangeRequestApproval,
    ItemPermissionsRequestApproval,
    ItemToolRequestUserInput,
    McpServerElicitationRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingServerRequestContext {
    pub request_id: SmolStr,
    pub request_kind: ServerRequestKind,
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub item_id: Option<ItemId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequestPayload {
    pub request: PendingServerRequestContext,
    pub approval_id: SmolStr,
    pub action_summary: String,
    pub justification: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_pattern: Option<Vec<String>>,
    /// Suggested command prefix for "always allow commands that start with …".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_prefix: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalDecisionPayload {
    pub approval_id: SmolStr,
    pub decision: String,
    pub scope: String,
    /// Authority that produced the decision. Missing on legacy events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_source: Option<crate::native::item::ApprovalDecisionSource>,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::workspace_changes::{
        WorkspaceChangeCoverage, WorkspaceChangeScope, WorkspaceChangeSetStatus,
        WorkspaceChangeViewStatus,
    };

    #[test]
    fn turn_failed_payload_carries_error_on_native_turn() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let mut turn = crate::native::turn::Turn {
            id: turn_id,
            session_id,
            sequence: 1,
            kind: crate::native::turn::TurnKind::Regular,
            status: crate::native::turn::TurnStatus::Failed,
            model: crate::native::model::ModelBinding {
                provider: "unknown".into(),
                model: "provider-model".into(),
                variant: None,
                reasoning_effort: None,
            },
            collaboration_mode: None,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            error: None,
            usage: None,
        };
        turn.error = Some(crate::native::error::AgentError::new(
            "PROVIDER_SERVER_ERROR".to_string(),
            "Internal server error".to_string(),
        ));
        let payload = TurnFailedPayload {
            session_id,
            turn: turn.clone(),
        };

        let value = serde_json::to_value(&payload).expect("serialize turn failure");
        assert!(value.get("error").is_none());
        assert_eq!(
            value["turn"]["error"]["errorCode"],
            serde_json::json!("PROVIDER_SERVER_ERROR")
        );
        let restored: TurnFailedPayload =
            serde_json::from_value(value).expect("deserialize turn failure");
        assert_eq!(restored, payload);
    }

    #[test]
    fn tool_result_payload_display_content_is_optional() {
        let payload: ToolResultPayload = serde_json::from_str(
            r#"{
                "tool_call_id": "call-1",
                "tool_name": "read",
                "content": "canonical",
                "is_error": false
            }"#,
        )
        .expect("deserialize legacy payload");
        assert_eq!(payload.display_content, None);
        assert_eq!(payload.input, None);
        assert_eq!(payload.summary, "");

        let payload = ToolResultPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: Some("read".to_string()),
            input: Some(serde_json::json!({"filePath": "foo.txt"})),
            content: serde_json::Value::String("canonical".to_string()),
            display_content: Some("display".to_string()),
            is_error: false,
            summary: "read output".to_string(),
        };
        let json = serde_json::to_value(&payload).expect("serialize payload");
        assert_eq!(
            json.get("display_content"),
            Some(&serde_json::Value::String("display".to_string()))
        );
        assert_eq!(
            json.get("input"),
            Some(&serde_json::json!({"filePath": "foo.txt"}))
        );
    }

    #[test]
    fn message_edit_events_roundtrip_and_report_methods() {
        let session_id = SessionId::new();
        let target_message_id = ItemId::new();
        let replacement_message_id = ItemId::new();
        let timestamp = Utc::now();
        let native_session_id = session_id;
        let edit = crate::native::event::ServerNotification::MessageEditRecorded {
            session_id: native_session_id,
            edit_id: "edit-1".to_string(),
            target_message_id,
            replacement_message_id,
            edit_state: "accepted".to_string(),
            content_preview: "edited".to_string(),
            mentions: vec![],
            timestamp,
        };
        let restore_started = crate::native::event::ServerNotification::WorkspaceRestoreStarted {
            session_id: native_session_id,
            restore_plan_id: crate::native::ids::RestorePlanId::from_string("edit-1".to_string()),
        };
        let restore_completed =
            crate::native::event::ServerNotification::WorkspaceRestoreCompleted {
                session_id: native_session_id,
                restore_plan_id: crate::native::ids::RestorePlanId::from_string(
                    "edit-1".to_string(),
                ),
                succeeded: true,
                error: None,
            };

        let (edit_method, _) = crate::native::wire_projector::wire_from_server_notification(&edit);
        assert_eq!(edit_method, "message/edit/recorded");
        assert_eq!(
            crate::native::notification_bus::notification_legacy_session_id(&edit),
            Some(session_id)
        );

        let (restore_started_method, _) =
            crate::native::wire_projector::wire_from_server_notification(&restore_started);
        assert_eq!(restore_started_method, "workspace/restoreStarted");
        assert_eq!(
            crate::native::notification_bus::notification_legacy_session_id(&restore_started),
            Some(session_id)
        );

        let (restore_completed_method, _) =
            crate::native::wire_projector::wire_from_server_notification(&restore_completed);
        assert_eq!(restore_completed_method, "workspace/restoreCompleted");
        assert_eq!(
            crate::native::notification_bus::notification_legacy_session_id(&restore_completed),
            Some(session_id)
        );
    }

    #[test]
    fn workspace_changes_updated_notification_method_name() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let notification = crate::native::event::ServerNotification::WorkspaceChangesUpdated(
            crate::native::event::WorkspaceChangesUpdatedNotification {
                session_id,
                turn_id,
                scope: WorkspaceChangeScope::Turn,
                status: WorkspaceChangeViewStatus::Ready,
                coverage: WorkspaceChangeCoverage::GitVisible,
                change_set_status: WorkspaceChangeSetStatus::Finalized,
                stats: crate::native::event::WorkspaceChangeStatsNotification {
                    files_changed: 1,
                    additions: 2,
                    deletions: 0,
                },
                version: 1,
                generated_at: Utc::now(),
            },
        );
        let (method, _) =
            crate::native::wire_projector::wire_from_server_notification(&notification);
        assert_eq!(method, "workspace/changes/updated");
        assert_eq!(
            crate::native::notification_bus::notification_legacy_session_id(&notification),
            Some(session_id)
        );
    }
}
