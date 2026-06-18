use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::SessionId;
use crate::TurnId;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolPolicy {
    #[default]
    Inherit,
    DenyAll,
    DeepResearch,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentContextMode {
    #[default]
    CodingAgent,
    DeepResearch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub context_mode: AgentContextMode,
    #[serde(default)]
    pub ephemeral: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnAgentResult {
    pub child_session_id: SessionId,
    pub agent_path: String,
    pub agent_nickname: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessageParams {
    pub session_id: SessionId,
    pub target: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessageResult {
    pub delivered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitAgentParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitAgentResult {
    pub events: Vec<AgentOutputEvent>,
    pub next_sequence: u64,
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOutputEvent {
    pub sequence: u64,
    pub child_session_id: SessionId,
    pub agent_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentListParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentListResult {
    pub agents: Vec<AgentInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatusParams {
    pub session_id: SessionId,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseAgentParams {
    pub session_id: SessionId,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseAgentResult {
    pub closed: bool,
    pub status: String,
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
                context_mode: AgentContextMode::CodingAgent,
                ephemeral: false,
            },
            "result": SpawnAgentResult {
                child_session_id,
                agent_path: "root/review".to_string(),
                agent_nickname: "review".to_string(),
                status: "running".to_string(),
            },
            "wait": WaitAgentResult {
                events: vec![AgentOutputEvent {
                    sequence: 1,
                    child_session_id,
                    agent_path: "root/review".to_string(),
                    turn_id: Some(TurnId::new()),
                    kind: "assistant_delta".to_string(),
                    text: Some("done".to_string()),
                    status: None,
                    created_at: Utc::now(),
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
}
