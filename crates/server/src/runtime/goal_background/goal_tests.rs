use super::*;
use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_core::tools::AgentToolCoordinator;
use devo_protocol::{ModelRequest, ModelResponse, SpawnAgentParams, StreamEvent};
use devo_provider::ModelProviderSDK;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

struct GoalLane {
    requests: Mutex<Vec<ModelRequest>>,
    started: Semaphore,
    release: Semaphore,
}

impl Default for GoalLane {
    fn default() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            started: Semaphore::new(/*permits*/ 0),
            release: Semaphore::new(/*permits*/ 0),
        }
    }
}

#[derive(Default)]
struct ChildGoalProvider {
    parent: GoalLane,
    child: GoalLane,
}

#[async_trait]
impl ModelProviderSDK for ChildGoalProvider {
    fn name(&self) -> &str {
        "child-goal-inbox"
    }

    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("no auxiliary completion")
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        let is_child =
            serde_json::to_string(&request.messages)?.contains("child-goal-task-input-marker");
        let lane = if is_child { &self.child } else { &self.parent };
        let request_number = {
            let mut requests = lane.requests.lock().expect("requests");
            requests.push(request);
            requests.len()
        };
        let result = match (is_child, request_number) {
            (true, 1) => "initial-child-result-marker",
            (true, _) => "child-goal-continuation-result-marker",
            (false, _) => "parent-result-marker",
        };
        lane.started.add_permits(/*n*/ 1);
        lane.release.acquire().await?.forget();
        Ok(Box::pin(futures::stream::iter([
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: result.into(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: result.into(),
                    content: vec![devo_protocol::ResponseContent::Text(result.into())],
                    stop_reason: Some(devo_protocol::StopReason::EndTurn),
                    usage: Default::default(),
                    metadata: Default::default(),
                },
            }),
        ])))
    }
}

#[tokio::test]
async fn spawned_child_goal_completion_automatically_reaches_parent_provider() -> Result<()> {
    let home = tempfile::TempDir::new()?;
    let provider = Arc::new(ChildGoalProvider::default());
    let runtime = crate::test_support::TestRuntime::new(provider.clone())
        .with_test_model()
        .runtime(home.path());
    let (tx, _notifications) = crate::test_outbound_channel(/*capacity*/ 2048);
    let connection = runtime
        .register_connection(crate::ClientTransportKind::Stdio, tx)
        .await;
    runtime
        .handle_incoming(
            connection,
            json!({"id":1,"method":"initialize","params":{
                "protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}},
                "clientInfo":{"name":"child-goal-test","title":"test","version":"1"}
            }}),
        )
        .await
        .context("initialize")?;
    let created = runtime
        .handle_incoming(
            connection,
            json!({"id":2,"method":"session/new",
                "params":{"cwd":home.path(),"idempotencyKey":"parent"}
            }),
        )
        .await
        .context("create parent")?;
    let parent = SessionId::from(
        created["result"]["session"]["id"]
            .as_str()
            .context("parent id")?,
    );
    let child = runtime
        .clone()
        .spawn_agent(SpawnAgentParams {
            session_id: parent,
            message: "child-goal-task-input-marker".into(),
            model: None,
            thinking: None,
            fork_turns: Some("none".into()),
            max_turns: None,
            tool_policy: Default::default(),
            ephemeral: false,
            nickname: Some("goal-worker".into()),
        })
        .await?
        .child_session_id;
    tokio::time::timeout(
        Duration::from_secs(/*secs*/ 20),
        provider.child.started.acquire(),
    )
    .await??
    .forget();
    provider.child.release.add_permits(/*n*/ 1);
    tokio::time::timeout(
        Duration::from_secs(/*secs*/ 20),
        provider.parent.started.acquire(),
    )
    .await??
    .forget();

    // The initial spawned turn has reported independently. Start a real hidden
    // continuation on that child while its parent processes the first result.
    let goal = runtime
        .handle_incoming(
            connection,
            json!({"id":3,"method":"session/goal/set","params":{
                "sessionId":child,"objective":"child-goal-objective-marker",
                "ifExists":"reject","idempotencyKey":"child-goal"
            }}),
        )
        .await
        .context("set child goal")?;
    assert!(goal.get("error").is_none(), "{goal}");
    tokio::time::timeout(
        Duration::from_secs(/*secs*/ 20),
        provider.child.started.acquire(),
    )
    .await??
    .forget();
    let child_turn = runtime
        .session(child)
        .await
        .context("child session")?
        .turn_reservation_snapshot()
        .await
        .context("child reservation")?
        .active_turn
        .context("active child goal turn")?;
    assert_eq!(
        child_turn.native.kind,
        devo_protocol::native::turn::TurnKind::GoalContinuation
    );
    {
        let requests = provider.child.requests.lock().expect("child requests");
        assert_eq!(requests.len(), 2);
        assert!(serde_json::to_string(&requests[1])?.contains("child-goal-objective-marker"));
    }

    // No completion callback, collect, wait_agent, or user message is injected.
    // Goal startup must register the same parent-delivery ticket as other turns.
    provider.child.release.add_permits(/*n*/ 1);
    provider.parent.release.add_permits(/*n*/ 1);
    tokio::time::timeout(
        Duration::from_secs(/*secs*/ 20),
        provider.parent.started.acquire(),
    )
    .await
    .context("child goal result did not reach its direct parent's provider")??
    .forget();
    {
        let requests = provider.parent.requests.lock().expect("parent requests");
        assert_eq!(requests.len(), 2);
        let input = serde_json::to_string(&requests[1])?;
        assert!(
            input.contains("child-goal-continuation-result-marker"),
            "{input}"
        );
        assert!(input.contains(child.as_str()), "{input}");
        assert!(input.contains(child_turn.native.id.as_str()), "{input}");
    }

    let paused = runtime
        .handle_incoming(
            connection,
            json!({"id":4,"method":"session/goal/pause","params":{
                "sessionId":child,"expectedGoalId":goal["result"]["goal"]["id"]
            }}),
        )
        .await
        .context("pause child goal")?;
    assert!(paused.get("error").is_none(), "{paused}");
    provider.parent.release.add_permits(/*n*/ 8);
    Ok(())
}
