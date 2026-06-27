use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use devo_protocol::{
    AgentContextMode, AgentListParams, AgentMessageParams, AgentToolPolicy, CloseAgentParams,
    SessionId, SpawnAgentParams, WaitAgentParams,
};

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgress, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::registry::ToolExposure;
use crate::registry::ToolRegistryBuilder;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolPreparationFeedback, ToolSpec};

#[derive(Clone, Copy)]
enum AgentToolKind {
    Spawn,
    SendMessage,
    Wait,
    List,
    Close,
}

pub struct AgentToolHandler {
    spec: ToolSpec,
    kind: AgentToolKind,
}

impl AgentToolHandler {
    fn new(spec: ToolSpec, kind: AgentToolKind) -> Self {
        Self { spec, kind }
    }
}

pub fn register_agent_tools(builder: &mut ToolRegistryBuilder) {
    let spawn = Arc::new(AgentToolHandler::new(spawn_spec(), AgentToolKind::Spawn));
    let send = Arc::new(AgentToolHandler::new(
        send_message_spec(),
        AgentToolKind::SendMessage,
    ));
    let wait = Arc::new(AgentToolHandler::new(
        wait_agent_spec(),
        AgentToolKind::Wait,
    ));
    let list = Arc::new(AgentToolHandler::new(
        list_agents_spec(),
        AgentToolKind::List,
    ));
    let close = Arc::new(AgentToolHandler::new(
        close_agent_spec(),
        AgentToolKind::Close,
    ));

    register(builder, spawn, &["spawn_subagent", "subagent", "delegate"]);
    register(builder, send, &[]);
    register(builder, wait, &["subagent_result"]);
    register(builder, list, &["subagent_status"]);
    register(builder, close, &[]);
}

fn register(builder: &mut ToolRegistryBuilder, handler: Arc<AgentToolHandler>, aliases: &[&str]) {
    builder.push_spec_with_exposure(handler.spec().clone(), ToolExposure::Direct);
    let handler: Arc<dyn ToolHandler> = handler;
    let name = handler.spec().name.clone();
    builder.register_handler(&name, Arc::clone(&handler));
    for alias in aliases {
        builder.register_handler(alias, Arc::clone(&handler));
    }
}

#[async_trait]
impl ToolHandler for AgentToolHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let Some(coordinator) = ctx.agent_coordinator.clone() else {
            return Err(ToolCallError::NeedsConfiguration(
                "child agent coordination is not configured".to_string(),
            ));
        };
        let session_id = current_session_id(&ctx)?;
        match self.kind {
            AgentToolKind::Spawn => {
                let input: SpawnAgentInput = parse_input(input)?;
                let fork_turns = match (ctx.agent_context_mode, input.fork_turns) {
                    (AgentContextMode::CodingAgent, fork_turns) => fork_turns,
                    (AgentContextMode::DeepResearch, _) => Some("none".to_string()),
                };
                let result = coordinator
                    .spawn_agent(SpawnAgentParams {
                        session_id,
                        message: input.message,
                        fork_turns,
                        max_turns: None,
                        tool_policy: match ctx.agent_context_mode {
                            AgentContextMode::CodingAgent => AgentToolPolicy::Inherit,
                            AgentContextMode::DeepResearch => AgentToolPolicy::DeepResearch,
                        },
                        context_mode: ctx.agent_context_mode,
                        ephemeral: false,
                    })
                    .await?;
                json_result(result, "agent spawned")
            }
            AgentToolKind::SendMessage => {
                let input: AgentMessageInput = parse_input(input)?;
                let result = coordinator
                    .send_message(AgentMessageParams {
                        session_id,
                        target: input.target,
                        message: input.message,
                    })
                    .await?;
                json_result(result, "message delivered")
            }
            AgentToolKind::Wait => {
                if let Some(progress) = progress {
                    let _ = progress.send(ToolProgress::StatusUpdate {
                        message: "Waiting for subagent messages...".to_string(),
                        percent: None,
                    });
                }
                let input: WaitAgentInput = parse_input(input)?;
                let params = WaitAgentParams {
                    session_id,
                    target: input.target,
                    after_sequence: input.after_sequence,
                    timeout_ms: input.timeout_ms,
                };
                let result = tokio::select! {
                    result = coordinator.wait_agent(params) => result?,
                    _ = ctx.cancel_token.cancelled() => return Err(ToolCallError::Cancelled),
                };
                json_result(result, "wait completed")
            }
            AgentToolKind::List => {
                let input: ListAgentsInput = parse_input(input)?;
                let agents = coordinator
                    .list_agents(AgentListParams {
                        session_id,
                        path_prefix: input.path_prefix,
                    })
                    .await?;
                json_result(serde_json::json!({ "agents": agents }), "agents listed")
            }
            AgentToolKind::Close => {
                let input: CloseAgentInput = parse_input(input)?;
                let result = coordinator
                    .close_agent(CloseAgentParams {
                        session_id,
                        target: input.target,
                    })
                    .await?;
                json_result(result, "agent closed")
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct SpawnAgentInput {
    message: String,
    #[serde(default)]
    fork_turns: Option<String>,
}

#[derive(serde::Deserialize)]
struct AgentMessageInput {
    target: String,
    message: String,
}

#[derive(serde::Deserialize)]
struct WaitAgentInput {
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    after_sequence: Option<u64>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(serde::Deserialize)]
struct ListAgentsInput {
    #[serde(default)]
    path_prefix: Option<String>,
}

#[derive(serde::Deserialize)]
struct CloseAgentInput {
    target: String,
}

fn current_session_id(ctx: &ToolContext) -> Result<SessionId, ToolCallError> {
    SessionId::try_from(ctx.session_id.clone()).map_err(|error| {
        ToolCallError::InvalidInput(format!("invalid current session id: {error}"))
    })
}

fn parse_input<T: serde::de::DeserializeOwned>(
    input: serde_json::Value,
) -> Result<T, ToolCallError> {
    serde_json::from_value(input).map_err(|error| ToolCallError::InvalidInput(error.to_string()))
}

fn json_result(
    value: impl serde::Serialize,
    summary: impl Into<String>,
) -> Result<ToolResult, ToolCallError> {
    let value = serde_json::to_value(value)
        .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
    Ok(ToolResult::success(ToolResultContent::Json(value), summary))
}

fn spec(name: &str, description: &str, schema: JsonSchema) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: schema,
        output_mode: ToolOutputMode::StructuredJson,
        execution_mode: ToolExecutionMode::Mutating,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    }
}

fn spawn_spec() -> ToolSpec {
    spec(
        "spawn_agent",
        "Launch a new Devo agent to handle complex, multi-step tasks autonomously.\n\nUse this from a parent session to parallelize independent research or implementation work, then use wait_agent to collect child output. Launch multiple agents concurrently whenever possible when the work is independent.\n\nWriting the prompt:\n- Brief the agent like a smart colleague who just walked into the room: it has not seen this conversation, does not know what you have tried, and does not understand why the task matters unless you tell it.\n- Explain what you are trying to accomplish and why.\n- Describe what you have already learned or ruled out.\n- Give enough context about the surrounding problem that the agent can make judgment calls rather than just following narrow instructions.\n- If you need a short response, say so, for example \"report in under 200 words\".\n- Lookups: hand over the exact command. Investigations: hand over the question; prescribed steps become dead weight when the premise is wrong.\n- Terse command-style prompts produce shallow, generic work.\n- Never delegate understanding. Do not write \"based on your findings, fix the bug\" or \"based on the research, implement it.\" Those phrases push synthesis onto the agent instead of doing it yourself. Write prompts that prove you understood: include file paths, line numbers, and what specifically to change.\n\nWhen not to use an agent:\n- If you want to read a specific file path, use the read tool or file search instead.\n- If you are searching for a specific class, symbol, or string, use grep/code search instead when available.\n- If you are searching within a specific file or a small set of files, read those files directly.\n- Do not use an agent for tasks unrelated to available agent descriptions or the current request.\n\nThe agent's result is not visible to the user. To show the user the result, send a concise summary after wait_agent returns.",
        JsonSchema::object(
            BTreeMap::from([
                (
                    "message".to_string(),
                    JsonSchema::string(Some(
                        "Initial task message for the child agent. Include the goal, scope, files or subsystems to inspect, context needed for judgment calls, and the expected result.",
                    )),
                ),
                (
                    "fork_turns".to_string(),
                    JsonSchema::string(Some(
                        "Optional history fork mode. In coding-agent sessions, use \"all\" (default) when the child needs stable completed parent history; it excludes the active parent turn. Use \"none\" for a clean child context containing only the task message. In DeepResearch sessions, this is always forced to \"none\". Do not assume the child sees your active turn unless you include needed context in message.",
                    )),
                ),
            ]),
            Some(vec!["message".to_string()]),
            Some(false),
        ),
    )
}

fn send_message_spec() -> ToolSpec {
    spec(
        "send_message",
        "Parent-only tool that sends additional user input to an existing child agent. If the child is idle, the message starts a child turn; if active, it queues for the next turn. Use this to continue a previously spawned agent with the agent's full context preserved. Each newly spawned agent starts from its configured fork context, so provide a complete task description when spawning rather than relying on hidden assumptions.",
        message_schema(),
    )
}

fn wait_agent_spec() -> ToolSpec {
    spec(
        "wait_agent",
        "Parent-only tool that polls child assistant output and terminal status events. Use after spawn_agent or send_message to collect incremental child results.\n\nDo not peek at generated transcript files or tail child output unless the user explicitly asks for a progress check. Reading a transcript mid-flight pulls the child agent's tool noise into your context and defeats the point of delegation.\n\nDo not race. After launching a child agent, you know nothing about what it found. Never fabricate or predict child results in any format: not as prose, summary, or structured output. If the user asks a follow-up before output lands, tell them the child agent is still running and give status, not a guess.",
        JsonSchema::object(
            BTreeMap::from([
                (
                    "target".to_string(),
                    JsonSchema::string(Some(
                        "Optional child agent path, generated nickname, or session id. Omit to poll all direct children.",
                    )),
                ),
                (
                    "after_sequence".to_string(),
                    JsonSchema::integer(Some(
                        "Only return output events with sequence greater than this value. To poll incrementally, pass the largest sequence value from the previous events list.",
                    )),
                ),
                (
                    "timeout_ms".to_string(),
                    JsonSchema::integer(Some(
                        "Optional wait timeout in milliseconds. If no newer output exists, wait up to this timeout before returning timed_out.",
                    )),
                ),
            ]),
            None,
            Some(false),
        ),
    )
}

fn list_agents_spec() -> ToolSpec {
    spec(
        "list_agents",
        "Parent-only tool that lists child agents for the current session, including generated path, nickname, status, and last task message.",
        JsonSchema::object(
            BTreeMap::from([(
                "path_prefix".to_string(),
                JsonSchema::string(Some("Optional generated child path or path prefix filter.")),
            )]),
            None,
            Some(false),
        ),
    )
}

fn close_agent_spec() -> ToolSpec {
    spec(
        "close_agent",
        "Parent-only tool that closes an existing child agent, interrupts active work if needed, and records a terminal output event.",
        JsonSchema::object(
            BTreeMap::from([(
                "target".to_string(),
                JsonSchema::string(Some(
                    "Target child agent path, generated nickname, or session id.",
                )),
            )]),
            Some(vec!["target".to_string()]),
            Some(false),
        ),
    )
}

fn message_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([
            (
                "target".to_string(),
                JsonSchema::string(Some(
                    "Target child agent path, generated nickname, or session id.",
                )),
            ),
            (
                "message".to_string(),
                JsonSchema::string(Some(
                    "Additional task input to deliver to the child as user text.",
                )),
            ),
        ]),
        Some(vec!["target".to_string(), "message".to_string()]),
        Some(false),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pretty_assertions::assert_eq;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::contracts::ToolBudgets;

    #[derive(Debug, Default)]
    struct FakeAgentCoordinator {
        spawned: Mutex<Vec<SpawnAgentParams>>,
    }

    #[derive(Debug, Default)]
    struct BlockingWaitCoordinator;

    #[async_trait]
    impl devo_tools::AgentToolCoordinator for FakeAgentCoordinator {
        async fn spawn_agent(
            self: Arc<Self>,
            params: SpawnAgentParams,
        ) -> Result<devo_protocol::SpawnAgentResult, ToolCallError> {
            self.spawned.lock().await.push(params);
            Ok(devo_protocol::SpawnAgentResult {
                child_session_id: SessionId::new(),
                agent_path: "root/reviewer".to_string(),
                agent_nickname: "reviewer".to_string(),
                status: "running".to_string(),
            })
        }

        async fn send_message(
            self: Arc<Self>,
            _params: AgentMessageParams,
        ) -> Result<devo_protocol::AgentMessageResult, ToolCallError> {
            Ok(devo_protocol::AgentMessageResult { delivered: true })
        }

        async fn wait_agent(
            self: Arc<Self>,
            _params: devo_protocol::WaitAgentParams,
        ) -> Result<devo_protocol::WaitAgentResult, ToolCallError> {
            Ok(devo_protocol::WaitAgentResult {
                events: Vec::new(),
                next_sequence: 1,
                timed_out: false,
            })
        }

        async fn list_agents(
            self: Arc<Self>,
            _params: AgentListParams,
        ) -> Result<Vec<devo_protocol::AgentInfo>, ToolCallError> {
            Ok(Vec::new())
        }

        async fn close_agent(
            self: Arc<Self>,
            _params: CloseAgentParams,
        ) -> Result<devo_protocol::CloseAgentResult, ToolCallError> {
            Ok(devo_protocol::CloseAgentResult {
                closed: true,
                status: "closed".to_string(),
            })
        }
    }

    #[async_trait]
    impl devo_tools::AgentToolCoordinator for BlockingWaitCoordinator {
        async fn spawn_agent(
            self: Arc<Self>,
            _params: SpawnAgentParams,
        ) -> Result<devo_protocol::SpawnAgentResult, ToolCallError> {
            Err(ToolCallError::InternalError("not used".to_string()))
        }

        async fn send_message(
            self: Arc<Self>,
            _params: AgentMessageParams,
        ) -> Result<devo_protocol::AgentMessageResult, ToolCallError> {
            Err(ToolCallError::InternalError("not used".to_string()))
        }

        async fn wait_agent(
            self: Arc<Self>,
            _params: devo_protocol::WaitAgentParams,
        ) -> Result<devo_protocol::WaitAgentResult, ToolCallError> {
            std::future::pending().await
        }

        async fn list_agents(
            self: Arc<Self>,
            _params: AgentListParams,
        ) -> Result<Vec<devo_protocol::AgentInfo>, ToolCallError> {
            Err(ToolCallError::InternalError("not used".to_string()))
        }

        async fn close_agent(
            self: Arc<Self>,
            _params: CloseAgentParams,
        ) -> Result<devo_protocol::CloseAgentResult, ToolCallError> {
            Err(ToolCallError::InternalError("not used".to_string()))
        }
    }

    #[tokio::test]
    async fn spawn_handler_delegates_to_coordinator() {
        let session_id = SessionId::new();
        let coordinator = Arc::new(FakeAgentCoordinator::default());
        let handler = AgentToolHandler::new(spawn_spec(), AgentToolKind::Spawn);
        let result = handler
            .handle(
                tool_context(
                    session_id,
                    Some(coordinator.clone() as Arc<dyn devo_tools::AgentToolCoordinator>),
                ),
                serde_json::json!({
                    "message": "review this",
                    "fork_turns": "all"
                }),
                None,
            )
            .await
            .expect("spawn succeeds");

        assert_eq!(result.result_summary, "agent spawned");
        assert_eq!(
            coordinator.spawned.lock().await.as_slice(),
            &[SpawnAgentParams {
                session_id,
                message: "review this".to_string(),
                fork_turns: Some("all".to_string()),
                max_turns: None,
                tool_policy: AgentToolPolicy::Inherit,
                context_mode: AgentContextMode::CodingAgent,
                ephemeral: false,
            }]
        );
    }

    #[tokio::test]
    async fn spawn_handler_uses_deep_research_context_defaults() {
        let session_id = SessionId::new();
        let coordinator = Arc::new(FakeAgentCoordinator::default());
        let handler = AgentToolHandler::new(spawn_spec(), AgentToolKind::Spawn);
        let result = handler
            .handle(
                tool_context_with_mode(
                    session_id,
                    Some(coordinator.clone() as Arc<dyn devo_tools::AgentToolCoordinator>),
                    AgentContextMode::DeepResearch,
                ),
                serde_json::json!({
                    "message": "research this source cluster",
                    "fork_turns": "all"
                }),
                None,
            )
            .await
            .expect("spawn succeeds");

        assert_eq!(result.result_summary, "agent spawned");
        assert_eq!(
            coordinator.spawned.lock().await.as_slice(),
            &[SpawnAgentParams {
                session_id,
                message: "research this source cluster".to_string(),
                fork_turns: Some("none".to_string()),
                max_turns: None,
                tool_policy: AgentToolPolicy::DeepResearch,
                context_mode: AgentContextMode::DeepResearch,
                ephemeral: false,
            }]
        );
    }

    #[test]
    fn agent_tool_schemas_explain_subagent_workflow() {
        let spawn = spawn_spec();
        let schema = spawn.input_schema.to_json_value();
        let fork_description = schema["properties"]["fork_turns"]["description"]
            .as_str()
            .expect("fork_turns description");

        assert!(spawn.description.contains("wait_agent"));
        assert!(fork_description.contains("\"all\" (default)"));
        assert!(fork_description.contains("stable completed parent history"));
        assert!(fork_description.contains("excludes the active parent turn"));
        assert!(fork_description.contains("DeepResearch sessions"));
        assert!(fork_description.contains("always forced to \"none\""));
        assert!(fork_description.contains("\"none\""));
        assert!(send_message_spec().description.contains("Parent-only"));
        assert!(
            send_message_spec()
                .description
                .contains("queues for the next turn")
        );
        assert!(
            wait_agent_spec()
                .description
                .contains("polls child assistant output")
        );
        let wait_schema = wait_agent_spec().input_schema.to_json_value();
        let after_sequence_description = wait_schema["properties"]["after_sequence"]["description"]
            .as_str()
            .expect("after_sequence description");
        assert!(after_sequence_description.contains("largest sequence value"));
        assert!(!after_sequence_description.contains("previous next_sequence"));
        assert!(list_agents_spec().description.contains("generated path"));
        assert!(
            close_agent_spec()
                .description
                .contains("terminal output event")
        );
    }

    #[tokio::test]
    async fn agent_handler_requires_configured_coordinator() {
        let handler = AgentToolHandler::new(spawn_spec(), AgentToolKind::Spawn);
        let error = handler
            .handle(
                tool_context(SessionId::new(), None),
                serde_json::json!({
                    "message": "review this"
                }),
                None,
            )
            .await
            .expect_err("missing coordinator should fail");

        assert!(matches!(
            error,
            ToolCallError::NeedsConfiguration(message)
                if message == "child agent coordination is not configured"
        ));
    }

    #[tokio::test]
    async fn wait_handler_stops_when_context_is_cancelled() {
        let session_id = SessionId::new();
        let coordinator = Arc::new(BlockingWaitCoordinator);
        let handler = AgentToolHandler::new(wait_agent_spec(), AgentToolKind::Wait);
        let cancel_token = CancellationToken::new();
        let ctx = tool_context_with_cancel_token(
            session_id,
            Some(coordinator as Arc<dyn devo_tools::AgentToolCoordinator>),
            cancel_token.clone(),
        );
        cancel_token.cancel();

        let error = handler
            .handle(ctx, serde_json::json!({}), None)
            .await
            .expect_err("cancelled wait should fail");

        assert!(matches!(error, ToolCallError::Cancelled));
    }

    fn tool_context(
        session_id: SessionId,
        agent_coordinator: Option<Arc<dyn devo_tools::AgentToolCoordinator>>,
    ) -> ToolContext {
        tool_context_with_mode(session_id, agent_coordinator, AgentContextMode::CodingAgent)
    }

    fn tool_context_with_mode(
        session_id: SessionId,
        agent_coordinator: Option<Arc<dyn devo_tools::AgentToolCoordinator>>,
        agent_context_mode: AgentContextMode,
    ) -> ToolContext {
        tool_context_with_cancel_token_and_mode(
            session_id,
            agent_coordinator,
            CancellationToken::new(),
            agent_context_mode,
        )
    }

    fn tool_context_with_cancel_token(
        session_id: SessionId,
        agent_coordinator: Option<Arc<dyn devo_tools::AgentToolCoordinator>>,
        cancel_token: CancellationToken,
    ) -> ToolContext {
        tool_context_with_cancel_token_and_mode(
            session_id,
            agent_coordinator,
            cancel_token,
            AgentContextMode::CodingAgent,
        )
    }

    fn tool_context_with_cancel_token_and_mode(
        session_id: SessionId,
        agent_coordinator: Option<Arc<dyn devo_tools::AgentToolCoordinator>>,
        cancel_token: CancellationToken,
        agent_context_mode: AgentContextMode,
    ) -> ToolContext {
        ToolContext {
            tool_call_id: crate::invocation::ToolCallId("tool-call".to_string()),
            session_id: session_id.to_string(),
            turn_id: None,
            workspace_root: ".".into(),
            budgets: ToolBudgets {
                output_limit_bytes: 1024,
                wall_time_limit_ms: None,
            },
            cancel_token,
            agent_scope: crate::contracts::ToolAgentScope::Parent,
            agent_context_mode,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            agent_coordinator,
            client_filesystem: None,
            client_terminal: None,
            network_proxy: None,
            network_no_proxy: None,
        }
    }
}
