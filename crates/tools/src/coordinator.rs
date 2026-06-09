use std::sync::Arc;

use async_trait::async_trait;
use devo_protocol::{
    AgentInfo, AgentListParams, AgentMessageParams, AgentMessageResult, CloseAgentParams,
    CloseAgentResult, RequestUserInputArgs, RequestUserInputResponse, SpawnAgentParams,
    SpawnAgentResult, WaitAgentParams, WaitAgentResult,
};

use crate::contracts::ToolCallError;

/// Runtime bridge used by built-in agent tools to coordinate child agents.
///
/// Implementations own session-tree state, mailboxes, persistence, and turn
/// execution. Tool handlers should validate model-facing input, fill in the
/// current session from `ToolContext`, and delegate to this trait.
#[async_trait]
pub trait AgentToolCoordinator: Send + Sync {
    async fn spawn_agent(
        self: Arc<Self>,
        params: SpawnAgentParams,
    ) -> Result<SpawnAgentResult, ToolCallError>;

    async fn send_message(
        self: Arc<Self>,
        params: AgentMessageParams,
    ) -> Result<AgentMessageResult, ToolCallError>;

    async fn wait_agent(
        self: Arc<Self>,
        params: WaitAgentParams,
    ) -> Result<WaitAgentResult, ToolCallError>;

    async fn list_agents(
        self: Arc<Self>,
        params: AgentListParams,
    ) -> Result<Vec<AgentInfo>, ToolCallError>;

    async fn close_agent(
        self: Arc<Self>,
        params: CloseAgentParams,
    ) -> Result<CloseAgentResult, ToolCallError>;

    async fn request_user_input(
        self: Arc<Self>,
        _session_id: String,
        _turn_id: String,
        _tool_call_id: String,
        _args: RequestUserInputArgs,
    ) -> Result<RequestUserInputResponse, ToolCallError> {
        Err(ToolCallError::ExecutionFailed(
            "request_user_input is unavailable in this runtime".to_string(),
        ))
    }
}
