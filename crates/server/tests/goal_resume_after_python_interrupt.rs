use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_core::tools::create_default_tool_registry;
use devo_protocol::native::rpc_session::GoalIfExists;
use devo_protocol::{
    ModelRequest, ModelResponse, ResponseContent, ResponseMetadata, StopReason, StreamEvent, Usage,
};
use devo_provider::ModelProviderSDK;
use futures::{Stream, stream};
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

#[path = "support/goal_continuation.rs"]
mod support;

struct PythonGoalProvider {
    requests: AtomicUsize,
    started_path: std::path::PathBuf,
}

#[async_trait]
impl ModelProviderSDK for PythonGoalProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(final_response())
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let number = self.requests.fetch_add(1, Ordering::SeqCst) + 1;
        if number == 3 {
            return Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::MessageDone {
                response: final_response(),
            })])));
        }
        let code = match number {
            1 => format!(
                "import asyncio\nfrom pathlib import Path\nPath({}).write_text('started')\nawait asyncio.sleep(60)\nraise AssertionError('interrupted cell reached its end')",
                serde_json::to_string(&self.started_path.to_string_lossy())?
            ),
            2 => "import goal; await goal.complete()".to_string(),
            _ => anyhow::bail!("unexpected request {number}"),
        };
        let id = format!("python-goal-{number}");
        let input = serde_json::json!({ "code": code });
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::ToolCallStart {
                index: 0,
                id: id.clone(),
                name: "ipython".to_string(),
                input: input.clone(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: format!("response-{number}"),
                    content: vec![ResponseContent::ToolUse {
                        id,
                        name: "ipython".to_string(),
                        input,
                    }],
                    stop_reason: Some(StopReason::ToolUse),
                    usage: Usage {
                        input_tokens: 10,
                        output_tokens: 4,
                        ..Default::default()
                    },
                    metadata: ResponseMetadata::default(),
                },
            }),
        ])))
    }

    fn name(&self) -> &str {
        "python-goal-pause-resume"
    }
}

#[derive(Clone, Copy)]
enum ResumeTiming {
    AfterPauseReply,
    WhilePausing,
}

#[tokio::test]
async fn paused_python_goal_resumes_and_completes() -> Result<()> {
    check_pause_resume(ResumeTiming::AfterPauseReply).await
}

#[tokio::test]
async fn concurrent_resume_is_not_overwritten_by_late_pause_reply() -> Result<()> {
    check_pause_resume(ResumeTiming::WhilePausing).await
}

async fn check_pause_resume(timing: ResumeTiming) -> Result<()> {
    let data_root = TempDir::new()?;
    let started_path = data_root.path().join("python-started.txt");
    let provider = Arc::new(PythonGoalProvider {
        requests: AtomicUsize::new(0),
        started_path: started_path.clone(),
    });
    let runtime = support::build_runtime_with_registry(
        data_root.path(),
        provider.clone(),
        Arc::new(create_default_tool_registry()),
    )?;
    let (connection_id, mut notifications_rx) =
        Box::pin(support::initialize_connection(&runtime)).await?;
    let session_id = Box::pin(support::start_session(
        &runtime,
        connection_id,
        data_root.path(),
    ))
    .await?;
    let goal = Box::pin(support::create_goal(
        &runtime,
        connection_id,
        session_id,
        "pause Python, then resume and complete",
        /*token_budget*/ None,
        GoalIfExists::Reject,
        "python-pause-resume",
    ))
    .await?;

    // Wait for a real effect from inside the cell, not a tool-start preview.
    timeout(Duration::from_secs(/*secs*/ 30), async {
        let mut ticks = tokio::time::interval(Duration::from_millis(/*millis*/ 20));
        while !started_path.exists() {
            ticks.tick().await;
        }
    })
    .await
    .context("Python cell did not start")?;

    // A slow subscriber keeps terminal delivery/merge in progress after the
    // bounded pause RPC returns. Resume must survive that finishing turn.
    let (_slow_connection, slow_rx) =
        tokio::spawn(stalled_subscriber(runtime.clone(), session_id)).await??;

    let pause_runtime = Arc::clone(&runtime);
    let pause_goal_id = goal.id;
    let mut pause_task = Some(tokio::spawn(async move {
        support::transition_goal(
            &pause_runtime,
            connection_id,
            session_id,
            "session/goal/pause",
            &pause_goal_id,
        )
        .await
    }));
    if matches!(timing, ResumeTiming::AfterPauseReply) {
        let paused = timeout(
            Duration::from_secs(/*secs*/ 10),
            pause_task.take().expect("pause task"),
        )
        .await
        .context("pause RPC timed out")???;
        assert_eq!(
            paused.status,
            devo_protocol::native::goal::GoalStatus::Paused
        );
    } else {
        // Observe the committed pause while its interrupt wait is still gated.
        timeout(Duration::from_secs(/*secs*/ 5), async {
            let mut ticks = tokio::time::interval(Duration::from_millis(/*millis*/ 20));
            loop {
                if support::read_goal(&runtime, connection_id, session_id)
                    .await?
                    .is_some_and(|g| g.status == devo_protocol::native::goal::GoalStatus::Paused)
                {
                    return Ok::<(), anyhow::Error>(());
                }
                ticks.tick().await;
            }
        })
        .await
        .context("goal did not enter paused state")??;
    }
    let resumed = timeout(
        Duration::from_secs(/*secs*/ 10),
        Box::pin(support::transition_goal(
            &runtime,
            connection_id,
            session_id,
            "session/goal/resume",
            &goal.id,
        )),
    )
    .await
    .context("resume RPC timed out")??;
    assert_eq!(
        resumed.status,
        devo_protocol::native::goal::GoalStatus::Active
    );
    if let Some(pause_task) = pause_task {
        let late_pause_reply = timeout(Duration::from_secs(/*secs*/ 10), pause_task)
            .await
            .context("late pause reply timed out")???;
        assert_eq!(
            late_pause_reply.status,
            devo_protocol::native::goal::GoalStatus::Active
        );
    }
    drop(slow_rx);
    let first = terminal_turn(&mut notifications_rx)
        .await
        .context("first terminal turn")?;
    let second = terminal_turn(&mut notifications_rx)
        .await
        .context("resumed continuation did not publish a terminal turn")?;
    assert_eq!(
        [
            first["params"]["turn"]["status"].clone(),
            second["params"]["turn"]["status"].clone()
        ],
        [
            serde_json::json!("interrupted"),
            serde_json::json!("completed")
        ]
    );
    assert_eq!(
        Box::pin(support::read_goal(&runtime, connection_id, session_id))
            .await?
            .map(|g| g.status),
        Some(devo_protocol::native::goal::GoalStatus::Completed)
    );
    assert_eq!(provider.requests.load(Ordering::SeqCst), 3);
    Ok(())
}

fn final_response() -> ModelResponse {
    ModelResponse {
        id: "goal-finished".to_string(),
        content: vec![ResponseContent::Text("Goal complete.".to_string())],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage::default(),
        metadata: ResponseMetadata::default(),
    }
}

async fn stalled_subscriber(
    runtime: Arc<devo_server::ServerRuntime>,
    session_id: devo_protocol::SessionId,
) -> Result<(u64, tokio::sync::mpsc::Receiver<devo_server::OutboundFrame>)> {
    let (slow_tx, slow_rx) = tokio::sync::mpsc::channel(1);
    let slow_connection = runtime
        .register_connection(devo_server::ClientTransportKind::Stdio, slow_tx.clone())
        .await;
    let initialized = Box::pin(runtime.handle_incoming(
        slow_connection,
        serde_json::json!({
            "id": "slow-init", "method": "initialize", "params": {
                "protocolVersion": 1, "clientCapabilities": {},
                "clientInfo": {"name":"slow-goal-test","title":"slow-goal-test","version":"1"},
                "_meta": {"devo":{"protocol":"native"}}
            }
        }),
    ))
    .await
    .context("slow initialize response")?;
    assert!(initialized.get("error").is_none(), "{initialized}");
    let subscribed = Box::pin(runtime.handle_incoming(slow_connection, serde_json::json!({
        "id": "slow-subscribe", "method": "subscription/create", "params": {"selectors":[{"kind":"session","sessionId":session_id}],"includeSnapshot":false}
    }))).await.context("slow subscription response")?;
    assert!(subscribed.get("error").is_none(), "{subscribed}");
    let _ = slow_tx.try_send(devo_server::OutboundFrame::json_rpc_response(
        slow_connection,
        serde_json::Value::Null,
    ));

    Ok((slow_connection, slow_rx))
}

async fn terminal_turn(
    notifications: &mut tokio::sync::mpsc::Receiver<serde_json::Value>,
) -> Result<serde_json::Value> {
    timeout(Duration::from_secs(/*secs*/ 30), async {
        while let Some(value) = notifications.recv().await {
            if value["method"] == "turn/completed" {
                return Ok(value);
            }
        }
        anyhow::bail!("notification channel closed before terminal turn")
    })
    .await
    .context("terminal turn timed out")?
}
