use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use crate::SessionId;
use crate::TaskId;
use crate::TaskState;
use crate::TurnId;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolPolicy {
    #[default]
    Inherit,
    DenyAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutputEventKind {
    #[serde(alias = "assistant_delta")]
    AssistantDelta,
    AssistantMessage,
    Status,
}

impl AgentOutputEventKind {
    pub fn is_assistant_text(self) -> bool {
        matches!(self, Self::AssistantDelta | Self::AssistantMessage)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SpawnAgentParams {
    pub session_id: SessionId,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_turns: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub tool_policy: AgentToolPolicy,
    #[serde(default)]
    pub ephemeral: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SpawnAgentResult {
    pub task_id: TaskId,
    pub child_session_id: SessionId,
    pub agent_path: String,
    pub agent_nickname: String,
    pub status: String,
}

/// Model-facing spawn result: address children by path or nickname, not session ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ParentSpawnAgentResult {
    pub task_id: TaskId,
    pub agent_path: String,
    pub agent_nickname: String,
    pub state: TaskState,
}

impl From<SpawnAgentResult> for ParentSpawnAgentResult {
    fn from(result: SpawnAgentResult) -> Self {
        Self {
            task_id: result.task_id,
            agent_path: result.agent_path,
            agent_nickname: result.agent_nickname,
            state: match result.status.as_str() {
                "completed" | "waiting_for_input" => TaskState::Completed,
                "failed" => TaskState::Failed,
                "interrupted" | "canceled" | "closed" => TaskState::Canceled,
                "spawning" | "running" => TaskState::Running,
                _ => TaskState::Failed,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AgentMessageParams {
    pub session_id: SessionId,
    pub target: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AgentMessageResult {
    pub delivered: bool,
    pub task_id: TaskId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WaitAgentParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

pub const DEFAULT_WAIT_AGENT_TIMEOUT_SECS: u64 = 5;
pub const MAX_WAIT_AGENT_TIMEOUT_SECS: u64 = 120;

pub fn resolve_wait_agent_timeout(timeout_secs: Option<u64>) -> u64 {
    timeout_secs
        .unwrap_or(DEFAULT_WAIT_AGENT_TIMEOUT_SECS)
        .min(MAX_WAIT_AGENT_TIMEOUT_SECS)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AgentMailboxMessage {
    pub message_id: String,
    pub from_session_id: SessionId,
    pub to_session_id: SessionId,
    pub from_agent_path: String,
    pub to_agent_path: String,
    pub content: String,
    pub sequence: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WaitAgentResult {
    pub events: Vec<ParentAgentOutputEvent>,
    pub next_sequence: u64,
    pub timed_out: bool,
}

/// Buffered child output stored server-side (includes routing metadata).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOutputEvent {
    pub sequence: u64,
    pub child_session_id: SessionId,
    pub agent_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub kind: AgentOutputEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Model-facing wait output: address children by path or nickname; omit internal session and turn ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ParentAgentOutputEvent {
    pub sequence: u64,
    pub agent_path: String,
    pub agent_nickname: String,
    pub kind: AgentOutputEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl From<AgentOutputEvent> for ParentAgentOutputEvent {
    fn from(event: AgentOutputEvent) -> Self {
        let agent_nickname = event
            .agent_path
            .rsplit('/')
            .next()
            .unwrap_or(&event.agent_path)
            .to_string();
        Self {
            sequence: event.sequence,
            agent_path: event.agent_path,
            agent_nickname,
            kind: event.kind,
            text: event.text,
            status: event.status,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AgentInfo {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<SessionId>,
    pub agent_path: String,
    pub agent_nickname: String,
    pub agent_role: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_task_message: Option<String>,
}

/// Model-facing list entry: address children by path or nickname.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ParentAgentInfo {
    pub agent_path: String,
    pub agent_nickname: String,
    pub agent_role: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_task_message: Option<String>,
}

impl From<AgentInfo> for ParentAgentInfo {
    fn from(info: AgentInfo) -> Self {
        Self {
            agent_path: info.agent_path,
            agent_nickname: info.agent_nickname,
            agent_role: info.agent_role,
            status: info.status,
            last_task_message: info.last_task_message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AgentListParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AgentListResult {
    pub agents: Vec<AgentInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ParentAgentListResult {
    pub agents: Vec<ParentAgentInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AgentStatusParams {
    pub session_id: SessionId,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CloseAgentParams {
    pub session_id: SessionId,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CloseAgentResult {
    pub closed: bool,
    pub status: String,
}

pub fn wait_agent_cursor_key(target: Option<&str>) -> String {
    target
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn agent_dtos_roundtrip_through_json() {
        let session_id = SessionId::new();
        let child_session_id = SessionId::new();
        let payloads = serde_json::json!({
            "spawn": SpawnAgentParams {
                session_id,
                message: "review this".to_string(),
                fork_turns: Some("all".to_string()),
                max_turns: None,
                tool_policy: AgentToolPolicy::Inherit,
                ephemeral: false,
            },
            "result": SpawnAgentResult {
                task_id: TaskId::from(child_session_id),
                child_session_id,
                agent_path: "root/review".to_string(),
                agent_nickname: "review".to_string(),
                status: "running".to_string(),
            },
            "wait": WaitAgentResult {
                events: vec![ParentAgentOutputEvent {
                    sequence: 1,
                    agent_path: "root/review".to_string(),
                    agent_nickname: "review".to_string(),
                    kind: AgentOutputEventKind::AssistantMessage,
                    text: Some("done".to_string()),
                    status: None,
                }],
                next_sequence: 2,
                timed_out: false,
            },
        });
        let json = serde_json::to_string(&payloads).expect("serialize agent payloads");
        let restored: serde_json::Value =
            serde_json::from_str(&json).expect("deserialize agent payloads");

        assert_eq!(restored, payloads);
    }

    #[test]
    fn resolve_wait_agent_timeout_uses_default_and_clamps() {
        assert_eq!(resolve_wait_agent_timeout(None), 5);
        assert_eq!(resolve_wait_agent_timeout(Some(30)), 30);
        assert_eq!(resolve_wait_agent_timeout(Some(999)), 120);
    }

    #[test]
    fn parent_visible_output_omits_internal_ids_and_includes_nickname() {
        let child_session_id = SessionId::new();
        let event = AgentOutputEvent {
            sequence: 1,
            child_session_id,
            agent_path: "root/review".to_string(),
            turn_id: Some(TurnId::new()),
            kind: AgentOutputEventKind::AssistantMessage,
            text: Some("done".to_string()),
            status: None,
            created_at: Utc::now(),
        };
        let parent_visible = ParentAgentOutputEvent::from(event);
        assert_eq!(parent_visible.agent_nickname, "review");
        let json = serde_json::to_value(parent_visible).expect("serialize parent output");
        assert!(json.get("child_session_id").is_none());
        assert!(json.get("turn_id").is_none());
        assert!(json.get("created_at").is_none());
    }
}
