use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::tools::create_default_tool_registry;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_protocol::native::rpc_session::GoalIfExists;
use devo_provider::ModelProviderSDK;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::Notify;
use tokio::time::timeout;

#[path = "support/goal_continuation.rs"]
mod support;

use support::build_runtime_with_registry;
use support::create_goal;
use support::initialize_connection;
use support::read_goal;
use support::start_session;
use support::wait_for_captured_request_count;

#[derive(Clone, Copy)]
enum ControlOutcome {
    ReplacementPaused,
    Paused,
    Canceled,
}

struct ControllingGoalProvider {
    requests: AtomicUsize,
    captured_requests: Mutex<Vec<ModelRequest>>,
    unexpected_request: Notify,
    outcome: ControlOutcome,
}

#[async_trait]
impl ModelProviderSDK for ControllingGoalProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(text_response("goal-title", "Completed goal"))
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_number = self.requests.fetch_add(1, Ordering::SeqCst) + 1;
        self.captured_requests
            .lock()
            .expect("lock captured requests")
            .push(request);

        match request_number {
            1 => {
                let controls = match self.outcome {
                    ControlOutcome::ReplacementPaused => {
                        "for action, status in ((goal.pause, 'paused'), (goal.resume, 'active'), (goal.cancel, 'canceled')):\n    result = await action()\n    assert result['goal']['status'] == status, result\nresult = await goal.create('Replacement goal')\nassert result['goal']['status'] == 'active', result\nresult = await goal.pause()\nassert result['goal']['status'] == 'paused', result"
                    }
                    ControlOutcome::Paused => "assert (await goal.pause())['goal']['status'] == 'paused'",
                    ControlOutcome::Canceled => "assert (await goal.cancel())['goal']['status'] == 'canceled'",
                };
                let code = format!(
                    "import goal, asyncio\n{controls}\nawait asyncio.sleep(1.1)\nprint('goal-controls-finished')"
                );
                let input = serde_json::json!({ "code": code });
                Ok(Box::pin(stream::iter(vec![
                    Ok(StreamEvent::ToolCallStart {
                        index: 0,
                        id: "control-goal".to_string(),
                        name: "ipython".to_string(),
                        input: input.clone(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "update-goal-response".to_string(),
                            content: vec![ResponseContent::ToolUse {
                                id: "control-goal".to_string(),
                                name: "ipython".to_string(),
                                input,
                            }],
                            stop_reason: Some(StopReason::ToolUse),
                            usage: Usage {
                                reported: true,
                                input_tokens: 10,
                                output_tokens: 4,
                                ..Default::default()
                            },
                            metadata: ResponseMetadata::default(),
                        },
                    }),
                ])))
            }
            2 => Ok(Box::pin(stream::iter(vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "The replacement goal is paused.".to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: text_response("final-response", "The replacement goal is paused."),
                }),
            ]))),
            _ => {
                self.unexpected_request.notify_one();
                anyhow::bail!("unexpected provider request {request_number} after goal pause")
            }
        }
    }

    fn name(&self) -> &str {
        "controlling-goal-provider"
    }
}

#[tokio::test]
async fn python_goal_controls_broadcast_without_aborting_their_own_turn() -> Result<()> {
    check_python_goal_controls(ControlOutcome::ReplacementPaused).await
}

#[tokio::test]
async fn python_goal_pause_accounts_final_turn_usage() -> Result<()> {
    check_python_goal_controls(ControlOutcome::Paused).await
}

#[tokio::test]
async fn python_goal_cancel_accounts_final_turn_usage() -> Result<()> {
    check_python_goal_controls(ControlOutcome::Canceled).await
}

async fn check_python_goal_controls(outcome: ControlOutcome) -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ControllingGoalProvider {
        requests: AtomicUsize::new(0),
        captured_requests: Mutex::new(Vec::new()),
        unexpected_request: Notify::new(),
        outcome,
    });
    let runtime = build_runtime_with_registry(
        data_root.path(),
        provider.clone(),
        Arc::new(create_default_tool_registry()),
    )?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    create_goal(
        &runtime,
        connection_id,
        session_id,
        "exercise the goal controls from Python",
        /*token_budget*/ None,
        GoalIfExists::Reject,
        "goal-python-controls",
    )
    .await?;

    // This turn starts a real Python process and performs a host RPC. The
    // tool-free goal fixture's five-second budget can expire on a busy runner.
    let mut observed = Vec::new();
    timeout(Duration::from_secs(/*secs*/ 30), async {
        while let Some(value) = notifications_rx.recv().await {
            let completed = value.get("method") == Some(&serde_json::json!("turn/completed"));
            observed.push(value);
            if completed {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before turn/completed")
    })
    .await
    .with_context(|| {
        format!(
            "timed out waiting for Python goal completion; provider requests: {}; notifications: {observed:?}",
            provider.requests.load(Ordering::SeqCst),
        )
    })??;
    wait_for_captured_request_count(&provider.captured_requests, /*expected*/ 2).await?;
    let mut lifecycle = observed
        .iter()
        .filter_map(|event| {
            let params = &event["params"];
            match event["method"].as_str()? {
                "session/goal/created" | "session/goal/updated" => {
                    params["goal"]["status"].as_str()
                }
                "session/goal/statusChanged" => params["status"].as_str(),
                _ => None,
            }
        })
        .collect::<Vec<_>>();
    lifecycle.dedup();
    let expected_lifecycle = match outcome {
        ControlOutcome::ReplacementPaused => {
            vec!["active", "paused", "active", "canceled", "active", "paused"]
        }
        ControlOutcome::Paused => vec!["active", "paused"],
        ControlOutcome::Canceled => vec!["active", "canceled"],
    };
    assert_eq!(lifecycle, expected_lifecycle);

    let goal = read_goal(&runtime, connection_id, session_id)
        .await?
        .context("goal after Python control")?;
    let expected_status = match outcome {
        ControlOutcome::ReplacementPaused | ControlOutcome::Paused => {
            devo_protocol::native::goal::GoalStatus::Paused
        }
        ControlOutcome::Canceled => devo_protocol::native::goal::GoalStatus::Canceled,
    };
    assert_eq!(goal.status, expected_status);
    match outcome {
        ControlOutcome::ReplacementPaused => {
            assert_eq!((goal.tokens_used, goal.time_used_seconds), (0, 0));
        }
        ControlOutcome::Paused | ControlOutcome::Canceled => {
            assert_eq!(goal.tokens_used, 14);
            assert!(
                goal.time_used_seconds >= 1,
                "final turn elapsed time was lost"
            );
        }
    }

    {
        let requests = provider
            .captured_requests
            .lock()
            .expect("lock captured requests");
        let active_goal_contexts = requests
            .iter()
            .map(|request| {
                request.messages.iter().any(|message| {
                    message.content.iter().any(|content| {
                        matches!(content, devo_protocol::RequestContent::Text { text, .. }
                            if text.contains("Continue working toward the active thread goal."))
                    })
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(active_goal_contexts, vec![true, false]);
        assert!(
            requests[1].messages.iter().any(|message| {
                message.content.iter().any(|content| {
                    matches!(
                        content,
                        devo_protocol::RequestContent::ToolResult { content, .. }
                            if content.contains("goal-controls-finished")
                    )
                })
            }),
            "the follow-up request should contain the final Python marker"
        );
    }

    assert!(
        timeout(
            Duration::from_secs(/*secs*/ 1),
            provider.unexpected_request.notified()
        )
        .await
        .is_err(),
        "a paused goal must not start another provider request"
    );
    assert_eq!(provider.requests.load(Ordering::SeqCst), 2);
    Ok(())
}

fn text_response(id: &str, text: &str) -> ModelResponse {
    ModelResponse {
        id: id.to_string(),
        content: vec![ResponseContent::Text(text.to_string())],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage::default(),
        metadata: ResponseMetadata::default(),
    }
}
