use std::collections::VecDeque;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

use crate::{ItemId, PendingInputId, ReasoningEffort, SessionId, TurnId, TurnStatus, TurnUsage};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnMetadata {
    pub turn_id: TurnId,
    pub session_id: SessionId,
    pub sequence: u32,
    pub status: TurnStatus,
    pub kind: TurnKind,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding_id: Option<String>,
    pub thinking: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub request_model: String,
    pub request_thinking: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub usage: Option<TurnUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputItem {
    Text { text: String },
    Skill { name: String, path: PathBuf },
    LocalImage { path: PathBuf },
    Mention { path: String, name: Option<String> },
}

impl<'de> Deserialize<'de> for InputItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum WireInputItem {
            Text {
                text: String,
            },
            Skill {
                name: Option<String>,
                path: Option<PathBuf>,
                id: Option<String>,
            },
            LocalImage {
                path: PathBuf,
            },
            Mention {
                path: String,
                name: Option<String>,
            },
        }

        match WireInputItem::deserialize(deserializer)? {
            WireInputItem::Text { text } => Ok(Self::Text { text }),
            WireInputItem::Skill { name, path, id } => {
                let name = name
                    .or(id)
                    .ok_or_else(|| serde::de::Error::missing_field("name"))?;
                Ok(Self::Skill {
                    name,
                    path: path.unwrap_or_default(),
                })
            }
            WireInputItem::LocalImage { path } => Ok(Self::LocalImage { path }),
            WireInputItem::Mention { path, name } => Ok(Self::Mention { path, name }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationMode {
    #[default]
    Build,
    Plan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnStartParams {
    pub session_id: SessionId,
    pub input: Vec<InputItem>,
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding_id: Option<String>,
    pub thinking: Option<String>,
    pub sandbox: Option<String>,
    pub approval_policy: Option<String>,
    pub cwd: Option<PathBuf>,
    #[serde(default, alias = "interaction_mode")]
    pub collaboration_mode: CollaborationMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TurnInputDisposition {
    #[default]
    Started,
    Queued,
    Steered,
}

fn default_steered_disposition() -> TurnInputDisposition {
    TurnInputDisposition::Steered
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum TurnStartResult {
    Started {
        turn_id: TurnId,
        status: TurnStatus,
        accepted_at: DateTime<Utc>,
    },
    Queued {
        active_turn_id: TurnId,
        queued_input_id: PendingInputId,
        status: TurnStatus,
        accepted_at: DateTime<Utc>,
    },
}

impl TurnStartResult {
    pub fn turn_id(&self) -> Option<TurnId> {
        match self {
            Self::Started { turn_id, .. } => Some(*turn_id),
            Self::Queued { .. } => None,
        }
    }

    pub fn active_turn_id(&self) -> TurnId {
        match self {
            Self::Started { turn_id, .. } => *turn_id,
            Self::Queued { active_turn_id, .. } => *active_turn_id,
        }
    }

    pub fn disposition(&self) -> TurnInputDisposition {
        match self {
            Self::Started { .. } => TurnInputDisposition::Started,
            Self::Queued { .. } => TurnInputDisposition::Queued,
        }
    }

    pub fn status(&self) -> TurnStatus {
        match self {
            Self::Started { status, .. } | Self::Queued { status, .. } => status.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCommandParams {
    pub session_id: SessionId,
    pub command: String,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCommandResult {
    pub turn_id: TurnId,
    pub status: TurnStatus,
    pub accepted_at: DateTime<Utc>,
}

impl ShellCommandResult {
    pub fn status(&self) -> TurnStatus {
        self.status.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnInterruptParams {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnInterruptResult {
    pub turn_id: TurnId,
    pub status: TurnStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSteerParams {
    pub session_id: SessionId,
    pub expected_turn_id: TurnId,
    pub input: Vec<InputItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSteerResult {
    pub turn_id: TurnId,
    #[serde(default = "default_steered_disposition")]
    pub disposition: TurnInputDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TurnKind {
    #[default]
    Regular,
    Review,
    ManualCompaction,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerInputRecord {
    pub item_id: ItemId,
    pub received_at: DateTime<Utc>,
    pub input: Vec<InputItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveTurnSteeringState {
    pub turn_id: TurnId,
    pub turn_kind: TurnKind,
    pub pending_inputs: VecDeque<SteerInputRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingInputItem {
    #[serde(default)]
    pub id: PendingInputId,
    pub kind: PendingInputKind,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

impl PendingInputItem {
    pub fn new(
        kind: PendingInputKind,
        metadata: Option<serde_json::Value>,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: PendingInputId::new(),
            kind,
            metadata,
            created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PendingInputKind {
    UserText {
        text: String,
    },
    UserInput {
        input: Vec<InputItem>,
        display_text: String,
        prompt_text: String,
        #[serde(default)]
        prompt_messages: Vec<String>,
    },
    ToolCallBlockedByHook {
        tool_use_id: String,
        reason: String,
    },
    BudgetLimitSteering,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn turn_metadata_roundtrips_with_logical_and_request_fields() {
        let metadata = TurnMetadata {
            turn_id: TurnId::new(),
            session_id: SessionId::new(),
            sequence: 1,
            status: TurnStatus::Completed,
            kind: TurnKind::Regular,
            model: "logical-model".to_string(),
            model_binding_id: Some("provider-binding".to_string()),
            thinking: Some("high".to_string()),
            reasoning_effort: Some(ReasoningEffort::High),
            request_model: "provider-model".to_string(),
            request_thinking: Some("medium".to_string()),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            usage: Some(TurnUsage {
                input_tokens: 10,
                output_tokens: 20,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        };

        let json = serde_json::to_string(&metadata).expect("serialize");
        let restored: TurnMetadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, metadata);
    }

    #[test]
    fn turn_start_params_default_to_build_collaboration_mode() {
        let json = serde_json::json!({
            "session_id": SessionId::new(),
            "input": [{ "type": "text", "text": "hello" }],
            "model": null,
            "thinking": null,
            "sandbox": null,
            "approval_policy": null,
            "cwd": null
        });

        let restored: TurnStartParams = serde_json::from_value(json).expect("deserialize");

        assert_eq!(restored.collaboration_mode, CollaborationMode::Build);
    }

    #[test]
    fn turn_start_params_accept_legacy_interaction_mode_alias() {
        let json = serde_json::json!({
            "session_id": SessionId::new(),
            "input": [{ "type": "text", "text": "hello" }],
            "model": null,
            "thinking": null,
            "sandbox": null,
            "approval_policy": null,
            "cwd": null,
            "interaction_mode": "plan"
        });

        let restored: TurnStartParams = serde_json::from_value(json).expect("deserialize");

        assert_eq!(restored.collaboration_mode, CollaborationMode::Plan);
    }

    #[test]
    fn turn_steer_result_defaults_to_steered_disposition() {
        let json = serde_json::json!({
            "turn_id": TurnId::new()
        });

        let restored: TurnSteerResult = serde_json::from_value(json).expect("deserialize");

        assert_eq!(restored.disposition, TurnInputDisposition::Steered);
    }

    #[test]
    fn shell_command_result_keeps_plain_started_shape() {
        let result = ShellCommandResult {
            turn_id: TurnId::new(),
            status: TurnStatus::Running,
            accepted_at: Utc::now(),
        };

        let json = serde_json::to_value(&result).expect("serialize");
        let turn_id = result.turn_id.to_string();

        assert_eq!(json["turn_id"].as_str(), Some(turn_id.as_str()));
        assert_eq!(json["status"].as_str(), Some("Running"));
        assert!(json.get("disposition").is_none());
    }

    #[test]
    fn pending_input_item_user_text_roundtrips() {
        let item = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "hello".into(),
            },
            Some(serde_json::json!({"source": "tui"})),
            Utc::now(),
        );
        let json = serde_json::to_string(&item).expect("serialize");
        let restored: PendingInputItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(item.created_at, restored.created_at);
        assert_eq!(item.metadata, restored.metadata);
        assert_eq!(format!("{:?}", item.kind), format!("{:?}", restored.kind));
    }

    #[test]
    fn pending_input_item_tool_call_blocked_roundtrips() {
        let item = PendingInputItem::new(
            PendingInputKind::ToolCallBlockedByHook {
                tool_use_id: "tool-1".into(),
                reason: "blocked by safety".into(),
            },
            None,
            Utc::now(),
        );
        let json = serde_json::to_string(&item).expect("serialize");
        let restored: PendingInputItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(item.created_at, restored.created_at);
    }

    #[test]
    fn pending_input_item_budget_limit_steering_roundtrips() {
        let item = PendingInputItem::new(PendingInputKind::BudgetLimitSteering, None, Utc::now());
        let json = serde_json::to_string(&item).expect("serialize");
        let restored: PendingInputItem = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(
            restored.kind,
            PendingInputKind::BudgetLimitSteering
        ));
    }

    #[test]
    fn pending_input_kind_serializes_tagged_shape() {
        let json = serde_json::json!({"type": "user_text", "text": "hello"});
        let kind: PendingInputKind = serde_json::from_value(json).expect("deserialize");
        assert!(matches!(kind, PendingInputKind::UserText { .. }));
    }

    #[test]
    fn turn_kind_default_is_regular() {
        assert_eq!(TurnKind::default(), TurnKind::Regular);
    }
}
