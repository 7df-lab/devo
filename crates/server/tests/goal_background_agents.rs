use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_core::tools::AgentToolCoordinator;
use devo_protocol::{ModelRequest, ModelResponse, StreamEvent};
use devo_provider::ModelProviderSDK;
use futures::{Stream, StreamExt};
use pretty_assertions::assert_eq;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::Notify;

#[path = "support/goal_continuation.rs"]
mod support;

#[derive(Default)]
struct ControlledProvider {
    requests: Mutex<Vec<ModelRequest>>,
    releases: Mutex<Vec<Arc<Notify>>>,
    started: Notify,
}

impl ControlledProvider {
    fn release(&self, index: usize) {
        self.releases.lock().unwrap()[index].notify_one();
    }

    async fn wait_for_request_after(&self, start: usize, text: &str) -> Result<usize> {
        tokio::time::timeout(Duration::from_secs(/*secs*/ 10), async {
            loop {
                let started = self.started.notified();
                if let Some(index) = self
                    .requests
                    .lock()
                    .expect("requests")
                    .iter()
                    .enumerate()
                    .skip(start)
                    .find_map(|(index, request)| {
                        support::request_last_message_contains_text(request, text).then_some(index)
                    })
                {
                    return index;
                }
                started.await;
            }
        })
        .await
        .with_context(|| format!("provider did not receive {text}"))
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
        let result = if support::request_contains_text(&request, "first child task") {
            if support::request_contains_text(&request, "busy child follow-up") {
                "followup-review-result-marker"
            } else {
                "initial-review-result-marker"
            }
        } else {
            "parent-result-marker"
        };
        let release = Arc::new(Notify::new());
        // Keep release handles and request indices paired across concurrent
        // parent and child calls instead of assuming a global call order.
        {
            let mut requests = self.requests.lock().expect("requests");
            self.releases
                .lock()
                .expect("releases")
                .push(release.clone());
            requests.push(request);
        }
        self.started.notify_one();
        let mut response = support::title_response();
        response.content = vec![devo_protocol::ResponseContent::Text(result.into())];
        Ok(Box::pin(
            futures::stream::once(async move {
                release.notified().await;
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: result.into(),
                })
            })
            .chain(futures::stream::iter([Ok(StreamEvent::MessageDone {
                response,
            })])),
        ))
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
    let followup = provider
        .wait_for_request_after(/*start*/ 2, "busy child follow-up")
        .await?;
    let completion = provider
        .wait_for_request_after(/*start*/ 2, "initial-review-result-marker")
        .await?;
    assert_ne!(followup, completion);
    support::wait_for_captured_request_count(&provider.requests, /*expected*/ 4).await?;

    // The parent may process a completion between child turns. That regular
    // inbox turn must not resume its autonomous goal while the follow-up runs.
    provider.release(completion);
    tokio::time::timeout(Duration::from_secs(/*secs*/ 10), async {
        loop {
            let completed =
                support::wait_for_notification(&mut notifications, "turn/completed").await?;
            if completed["params"]["turn"]["sessionId"] == parent.to_string() {
                return Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await??;
    let unexpected = tokio::time::timeout(Duration::from_millis(/*millis*/ 150), async {
        loop {
            let started =
                support::wait_for_notification(&mut notifications, "turn/started").await?;
            if started["params"]["turn"]["sessionId"] == parent.to_string()
                && started["params"]["turn"]["kind"] == "goalContinuation"
            {
                return Ok::<_, anyhow::Error>(started);
            }
        }
    })
    .await;
    assert!(
        unexpected.is_err(),
        "goal resumed before child finished: {unexpected:?}"
    );
    assert_eq!(provider.requests.lock().expect("requests").len(), 4);

    provider.release(followup);
    let final_completion = provider
        .wait_for_request_after(/*start*/ 4, "followup-review-result-marker")
        .await?;
    assert_eq!(final_completion, 4);
    {
        let requests = provider.requests.lock().unwrap();
        assert!(support::request_contains_text(
            &requests[final_completion],
            "initial-review-result-marker"
        ));
        assert!(support::request_last_message_contains_text(
            &requests[final_completion],
            "followup-review-result-marker"
        ));
    }
    provider.release(final_completion);
    let continuation = provider
        .wait_for_request_after(/*start*/ 5, "Wait for delegated review")
        .await?;
    assert_eq!(continuation, 5);
    {
        let requests = provider.requests.lock().unwrap();
        assert!(support::request_contains_text(
            &requests[continuation],
            "Wait for delegated review"
        ));
        assert!(support::request_contains_text(
            &requests[continuation],
            "followup-review-result-marker"
        ));
        assert_eq!(requests.len(), 6);
    }
    support::pause_goal_and_interrupt_session(&runtime, connection, parent).await
}
