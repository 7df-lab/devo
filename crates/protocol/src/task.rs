use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::SessionId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(transparent)]
pub struct TaskId(pub String);

impl From<SessionId> for TaskId {
    fn from(session_id: SessionId) -> Self {
        Self(session_id.to_string())
    }
}

impl AsRef<str> for TaskId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    WaitingApproval,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Agent,
    Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AgentTaskMetadata {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<SessionId>,
    pub agent_path: String,
    pub agent_nickname: String,
    pub agent_role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_task_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommandTaskMetadata {
    pub process_session_id: i32,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TaskInfo {
    pub task_id: TaskId,
    pub kind: TaskKind,
    pub state: TaskState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentTaskMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandTaskMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AwaitTaskParams {
    pub session_id: SessionId,
    pub task_id: TaskId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AwaitTaskResult {
    Terminal {
        task: TaskInfo,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    TimedOut {
        task: TaskInfo,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ListTasksParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ListTasksResult {
    pub tasks: Vec<TaskInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CancelTaskParams {
    pub session_id: SessionId,
    pub task_id: TaskId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CancelTaskResult {
    pub task: TaskInfo,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn task_result_roundtrips_with_agent_metadata() {
        let session_id = SessionId::new();
        let task = TaskInfo {
            task_id: TaskId::from(session_id),
            kind: TaskKind::Agent,
            state: TaskState::Completed,
            agent: Some(AgentTaskMetadata {
                session_id,
                parent_session_id: Some(SessionId::new()),
                agent_path: "root/reviewer".to_string(),
                agent_nickname: "reviewer".to_string(),
                agent_role: "default".to_string(),
                last_task_message: Some("review this".to_string()),
            }),
            command: None,
        };
        let result = AwaitTaskResult::Terminal {
            task,
            output: Some("done".to_string()),
        };

        let json = serde_json::to_value(&result).expect("serialize task result");
        let restored: AwaitTaskResult =
            serde_json::from_value(json).expect("deserialize task result");

        assert_eq!(restored, result);
    }
}
