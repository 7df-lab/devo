use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use devo_core::tools::AgentToolCoordinator;
use devo_protocol::{ModelRequest, ModelResponse, StreamEvent};
use devo_provider::ModelProviderSDK;
use futures::Stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::Notify;

#[path = "support/goal_continuation.rs"]
mod support;

#[derive(Default)]
struct ControlledProvider {
    requests: Mutex<Vec<ModelRequest>>,
    releases: Mutex<Vec<Arc<Notify>>>,
}

impl ControlledProvider {
    fn release(&self, index: usize) {
        self.releases.lock().unwrap()[index].notify_one();
    }
}

#[async_trait]
impl ModelProviderSDK for ControlledProvider {
    fn name(&self) -> &str {
        "controlled-background-agent"
    }
    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        Ok(support::title_response())
    }
    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let release = Arc::new(Notify::new());
        self.releases.lock().unwrap().push(Arc::clone(&release));
        self.requests.lock().unwrap().push(request);
        Ok(Box::pin(futures::stream::once(async move {
            release.notified().await;
            Ok(StreamEvent::MessageDone {
                response: support::title_response(),
            })
        })))
    }
}

#[tokio::test]
async fn busy_child_mailbox_keeps_goal_waiting_through_followup() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ControlledProvider::default());
    let runtime = support::build_runtime(data_root.path(), provider.clone())?;
    let (connection, mut notifications) = support::initialize_connection(&runtime).await?;
    let parent = support::start_session(&runtime, connection, data_root.path()).await?;
    support::create_goal(
        &runtime,
        connection,
        parent,
        "Wait for delegated review",
        None,
        devo_protocol::native::rpc_session::GoalIfExists::Reject,
        "delegated-goal",
    )
    .await?;
    support::wait_for_captured_request_count(&provider.requests, /*expected*/ 1).await?;
    let child = Arc::clone(&runtime)
        .spawn_agent(devo_protocol::SpawnAgentParams {
            model: None,
            thinking: None,
            session_id: parent,
            message: "first child task".to_string(),
            fork_turns: Some("none".to_string()),
            max_turns: None,
            tool_policy: devo_protocol::AgentToolPolicy::Inherit,
            ephemeral: false,
            nickname: Some("reviewer".to_string()),
        })
        .await?;
    support::wait_for_captured_request_count(&provider.requests, /*expected*/ 2).await?;
    Arc::clone(&runtime)
        .send_message(devo_protocol::AgentMessageParams {
            session_id: parent,
            target: child.child_session_id.to_string(),
            message: "busy child follow-up".to_string(),
        })
        .await?;
    provider.release(0);
    support::collect_until_turn_completed(&mut notifications).await?;
    provider.release(1);
    support::wait_for_captured_request_count(&provider.requests, /*expected*/ 3).await?;
    {
        let requests = provider.requests.lock().unwrap();
        assert!(support::request_contains_text(
            &requests[2],
            "busy child follow-up"
        ));
    }
    // If the parent woke between child turns, the third request above would
    // have been a hidden goal continuation instead of the queued child task.
    provider.release(2);
    support::wait_for_captured_request_count(&provider.requests, /*expected*/ 4).await?;
    {
        let requests = provider.requests.lock().unwrap();
        assert!(support::request_contains_text(
            &requests[3],
            "Wait for delegated review"
        ));
        assert_eq!(requests.len(), 4);
    }
    support::pause_goal_and_interrupt_session(&runtime, connection, parent).await
}
