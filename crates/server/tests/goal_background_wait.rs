use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::tools::AgentToolCoordinator;
use devo_protocol::SessionId;
use devo_protocol::native::goal::Goal;
use devo_protocol::native::rpc_session::GoalIfExists;
use devo_protocol::{ModelRequest, ModelResponse, StreamEvent};
use devo_provider::ModelProviderSDK;
use devo_server::ServerRuntime;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio::time::timeout;

#[path = "support/goal_continuation.rs"]
mod support;

#[derive(Default)]
struct ControlledProvider {
    requests: Mutex<Vec<ModelRequest>>,
    releases: Mutex<Vec<Arc<Notify>>>,
}

impl ControlledProvider {
    fn release(&self, index: usize) {
        self.releases.lock().expect("releases")[index].notify_one();
    }
}

#[async_trait]
impl ModelProviderSDK for ControlledProvider {
    fn name(&self) -> &str {
        "controlled-background-inbox"
    }

    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        Ok(support::title_response())
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        let release = Arc::new(Notify::new());
        self.releases
            .lock()
            .expect("releases")
            .push(release.clone());
        self.requests.lock().expect("requests").push(request);
        Ok(Box::pin(futures::stream::once(async move {
            release.notified().await;
            Ok(StreamEvent::MessageDone {
                response: support::title_response(),
            })
        })))
    }
}

struct WaitingGoal {
    _data_root: TempDir,
    runtime: Arc<ServerRuntime>,
    provider: Arc<ControlledProvider>,
    notifications: mpsc::Receiver<serde_json::Value>,
    connection_id: u64,
    session_id: SessionId,
    goal: Goal,
    initial_turn_id: String,
}

impl WaitingGoal {
    async fn new() -> Result<Self> {
        let data_root = TempDir::new()?;
        let provider = Arc::new(ControlledProvider::default());
        let runtime = support::build_runtime(data_root.path(), provider.clone())?;
        let (connection_id, mut notifications) = support::initialize_connection(&runtime).await?;
        let session_id = support::start_session(&runtime, connection_id, data_root.path()).await?;
        let goal = support::create_goal(
            &runtime,
            connection_id,
            session_id,
            "Finish the objective after all managed background work settles",
            /*token_budget*/ None,
            GoalIfExists::Reject,
            "background-wait-goal",
        )
        .await?;
        let started = support::wait_for_notification(&mut notifications, "turn/started").await?;
        let initial_turn_id = started["params"]["turn"]["id"]
            .as_str()
            .context("initial turn id")?
            .to_owned();
        support::wait_for_captured_request_count(&provider.requests, /*expected*/ 1).await?;
        Ok(Self {
            _data_root: data_root,
            runtime,
            provider,
            notifications,
            connection_id,
            session_id,
            goal,
            initial_turn_id,
        })
    }

    fn start_work(&self, id: &str) {
        self.runtime
            .background_work_started(self.session_id, id.to_string());
    }

    fn finish_work(&self, id: &str, result: &str) {
        self.runtime
            .background_work_completed(self.session_id, id.to_string(), result.to_string());
    }

    async fn finish_first_turn(&mut self) -> Result<()> {
        self.finish_turn(/*index*/ 0).await
    }

    async fn finish_turn(&mut self, index: usize) -> Result<()> {
        self.provider.release(index);
        support::collect_until_turn_completed(&mut self.notifications).await?;
        Ok(())
    }

    async fn assert_no_new_turn(&mut self, expected_requests: usize) -> Result<()> {
        let next_turn = timeout(Duration::from_millis(/*millis*/ 150), async {
            while let Some(notification) = self.notifications.recv().await {
                if notification["method"] == "turn/started" {
                    return notification;
                }
            }
            panic!("notification channel unexpectedly closed");
        })
        .await;
        assert!(next_turn.is_err(), "unexpected turn: {next_turn:?}");
        assert_eq!(
            self.provider.requests.lock().expect("lock requests").len(),
            expected_requests
        );
        Ok(())
    }

    async fn wait_for_second_turn(&mut self) -> Result<()> {
        self.wait_for_turn(/*expected_requests*/ 2, "regular").await
    }

    async fn wait_for_turn(&mut self, expected_requests: usize, kind: &str) -> Result<()> {
        let started =
            support::wait_for_notification(&mut self.notifications, "turn/started").await?;
        assert_eq!(started["params"]["turn"]["kind"], kind);
        support::wait_for_captured_request_count(&self.provider.requests, expected_requests).await
    }

    async fn stop(&self) -> Result<()> {
        support::pause_goal_and_interrupt_session(
            &self.runtime,
            self.connection_id,
            self.session_id,
        )
        .await
    }
}

#[tokio::test]
async fn goal_waits_for_every_background_task_and_receives_terminal_results() -> Result<()> {
    let mut test = WaitingGoal::new().await?;
    test.start_work("slow-command");
    test.start_work("child-agent");
    test.finish_first_turn().await?;
    test.assert_no_new_turn(/*expected_requests*/ 1).await?;

    test.finish_work("slow-command", "command terminal output: build succeeded");
    test.wait_for_second_turn().await?;
    {
        let requests = test.provider.requests.lock().expect("lock requests");
        assert!(support::request_last_message_contains_text(
            &requests[1],
            "command terminal output: build succeeded"
        ));
        assert!(!support::request_contains_text(
            &requests[1],
            "child terminal output: review complete"
        ));
    }
    test.finish_turn(/*index*/ 1).await?;
    // Completion output is delivered immediately, but the autonomous goal
    // must still wait for the remaining producer rather than start early.
    test.assert_no_new_turn(/*expected_requests*/ 2).await?;
    test.finish_work("child-agent", "child terminal output: review complete");
    test.wait_for_turn(/*expected_requests*/ 3, "regular")
        .await?;
    {
        let requests = test.provider.requests.lock().expect("lock requests");
        assert!(support::request_contains_text(
            &requests[2],
            "command terminal output: build succeeded"
        ));
        assert!(support::request_last_message_contains_text(
            &requests[2],
            "child terminal output: review complete"
        ));
    }

    // Duplicate delivery must not admit an additional continuation.
    test.finish_work("child-agent", "child terminal output: review complete");
    test.finish_work("slow-command", "command terminal output: build succeeded");
    test.assert_no_new_turn(/*expected_requests*/ 3).await?;
    test.finish_turn(/*index*/ 2).await?;
    test.wait_for_turn(/*expected_requests*/ 4, "goalContinuation")
        .await?;
    {
        let requests = test.provider.requests.lock().expect("lock requests");
        assert!(support::request_contains_text(
            &requests[3],
            "command terminal output: build succeeded"
        ));
        assert!(support::request_contains_text(
            &requests[3],
            "child terminal output: review complete"
        ));
    }
    test.stop().await
}

#[tokio::test]
async fn completion_before_wait_is_armed_is_not_lost() -> Result<()> {
    let mut test = WaitingGoal::new().await?;
    // Queue snapshots use the new subscription API, as in the TUI.
    let subscribed = test
        .runtime
        .handle_incoming(
            test.connection_id,
            serde_json::json!({
                "id": "subscribe-background-admission", "method": "subscription/create",
                "params": {
                    "selectors": [{"kind": "session", "sessionId": test.session_id}],
                    "includeSnapshot": false
                }
            }),
        )
        .await
        .context("subscribe to queue updates")?;
    assert!(subscribed.get("error").is_none(), "{subscribed}");
    test.start_work("fast-command");
    test.finish_work("fast-command", "fast command finished before parent turn");
    // The coordinator publishes asynchronously. Wait for durable admission
    // before releasing the request, so this really exercises the busy boundary.
    timeout(Duration::from_secs(/*secs*/ 5), async {
        loop {
            let update = support::wait_for_notification(&mut test.notifications, "queue/updated")
                .await
                .context("await background admission notification")?;
            if update["params"]["queue"]
                .to_string()
                .contains("fast command finished before parent turn")
            {
                return Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await
    .context("background result was not admitted")??;
    test.provider.release(/*index*/ 0);
    support::wait_for_captured_request_count(&test.provider.requests, /*expected*/ 2).await?;
    {
        let requests = test.provider.requests.lock().expect("lock requests");
        assert!(support::request_contains_text(
            &requests[1],
            "fast command finished before parent turn"
        ));
    }
    // The completion steers the current turn, not a separate regular turn.
    test.assert_no_new_turn(/*expected_requests*/ 2).await?;
    test.provider.release(/*index*/ 1);
    let completed = support::collect_until_turn_completed(&mut test.notifications).await?;
    assert_eq!(
        completed.last().context("completion event")?["params"]["turn"]["id"],
        test.initial_turn_id
    );
    test.wait_for_turn(/*expected_requests*/ 3, "goalContinuation")
        .await
        .context("await goal continuation after same-turn result")?;
    {
        let requests = test.provider.requests.lock().expect("lock requests");
        assert!(support::request_contains_text(
            &requests[2],
            "fast command finished before parent turn"
        ));
    }
    test.stop().await
}

#[tokio::test]
async fn failure_and_cancellation_are_terminal_background_results() -> Result<()> {
    for terminal_result in [
        "background command failed: exit code 17",
        "background command cancelled by the user",
    ] {
        let mut test = WaitingGoal::new().await?;
        test.start_work("terminal-command");
        test.finish_first_turn().await?;
        test.assert_no_new_turn(/*expected_requests*/ 1).await?;
        test.finish_work("terminal-command", terminal_result);
        test.wait_for_second_turn().await?;
        {
            let requests = test.provider.requests.lock().expect("lock requests");
            assert!(support::request_contains_text(
                &requests[1],
                terminal_result
            ));
        }
        test.stop().await?;
    }
    Ok(())
}

#[tokio::test]
async fn paused_or_cleared_goal_does_not_restart_on_background_completion() -> Result<()> {
    for method in ["session/goal/pause", "session/goal/clear"] {
        let mut test = WaitingGoal::new().await?;
        test.start_work("late-command");
        test.finish_first_turn().await?;
        test.assert_no_new_turn(/*expected_requests*/ 1).await?;
        let response = test
            .runtime
            .handle_incoming(
                test.connection_id,
                serde_json::json!({
                    "id": method,
                    "method": method,
                    "params": {
                        "sessionId": test.session_id,
                        "expectedGoalId": test.goal.id
                    }
                }),
            )
            .await
            .context("goal mutation response")?;
        assert!(response.get("error").is_none(), "{response}");
        test.finish_work("late-command", "late output must not revive old goal");
        test.wait_for_second_turn().await?;
        {
            let requests = test.provider.requests.lock().expect("lock requests");
            assert!(support::request_last_message_contains_text(
                &requests[1],
                "late output must not revive old goal"
            ));
        }
        test.finish_turn(/*index*/ 1).await?;
        test.assert_no_new_turn(/*expected_requests*/ 2).await?;
    }
    Ok(())
}

#[tokio::test]
async fn queued_user_input_runs_while_goal_waits_for_background_work() -> Result<()> {
    let mut test = WaitingGoal::new().await?;
    test.start_work("still-running-command");
    let response = test
        .runtime
        .handle_incoming(
            test.connection_id,
            serde_json::json!({
                "id": "queue-user-input",
                "method": "session/queue/push",
                "params": {
                    "sessionId": test.session_id,
                    "input": [{"type": "text", "text": "queued user input has priority"}],
                    "idempotencyKey": "background-wait-user-input"
                }
            }),
        )
        .await
        .context("queue response")?;
    let queued: devo_server::SuccessResponse<
        devo_protocol::native::rpc_turn::SessionQueuePushResult,
    > = serde_json::from_value(response)?;
    assert!(matches!(
        queued.result,
        devo_protocol::native::rpc_turn::SessionQueuePushResult::Queued { .. }
    ));
    test.finish_first_turn().await?;
    test.wait_for_second_turn().await?;
    {
        let requests = test.provider.requests.lock().expect("lock requests");
        assert!(support::request_contains_text(
            &requests[1],
            "queued user input has priority"
        ));
    }
    test.finish_work(
        "still-running-command",
        "background output during user turn",
    );
    test.assert_no_new_turn(/*expected_requests*/ 2).await?;
    test.stop().await
}

#[tokio::test]
async fn interrupted_parent_does_not_resume_goal_when_background_work_finishes() -> Result<()> {
    let mut test = WaitingGoal::new().await?;
    test.start_work("interrupted-command");
    let response = test
        .runtime
        .handle_incoming(
            test.connection_id,
            serde_json::json!({
                "id": "interrupt-parent",
                "method": "session/interrupt",
                "params": {
                    "scope": { "scope": "session", "sessionId": test.session_id }
                }
            }),
        )
        .await
        .context("interrupt response")?;
    assert!(response.get("error").is_none(), "{response}");
    support::collect_until_turn_completed(&mut test.notifications).await?;
    test.finish_work("interrupted-command", "completion after parent interrupt");
    test.assert_no_new_turn(/*expected_requests*/ 1).await
}

#[tokio::test]
async fn replaced_goal_does_not_restore_old_continuation_after_background_completion() -> Result<()>
{
    let mut test = WaitingGoal::new().await?;
    test.start_work("old-goal-command");
    test.finish_first_turn().await?;
    test.assert_no_new_turn(/*expected_requests*/ 1).await?;
    let replacement = support::create_goal(
        &test.runtime,
        test.connection_id,
        test.session_id,
        "Only pursue the replacement objective",
        /*token_budget*/ None,
        GoalIfExists::Replace,
        "replacement-background-goal",
    )
    .await?;
    test.finish_work("old-goal-command", "old task finished after replacement");
    test.wait_for_second_turn().await?;
    {
        let requests = test.provider.requests.lock().expect("lock requests");
        assert!(support::request_last_message_contains_text(
            &requests[1],
            "old task finished after replacement"
        ));
    }
    test.finish_turn(/*index*/ 1).await?;
    test.wait_for_turn(/*expected_requests*/ 3, "goalContinuation")
        .await?;
    {
        let requests = test.provider.requests.lock().expect("lock requests");
        assert!(support::request_last_message_contains_text(
            &requests[2],
            "Only pursue the replacement objective"
        ));
        assert!(!support::request_last_message_contains_text(
            &requests[2],
            "Finish the objective after all managed background work settles"
        ));
    }
    assert_eq!(
        support::read_goal(&test.runtime, test.connection_id, test.session_id)
            .await?
            .context("replacement goal")?
            .id,
        replacement.id
    );
    test.assert_no_new_turn(/*expected_requests*/ 3).await?;
    test.stop().await
}

#[tokio::test]
async fn idle_interrupt_cancels_wait_without_reserving_a_turn() -> Result<()> {
    let mut test = WaitingGoal::new().await?;
    test.start_work("idle-interrupted-command");
    test.finish_first_turn().await?;
    test.assert_no_new_turn(/*expected_requests*/ 1).await?;
    // Waiting leaves the session actor available for reads and controls.
    let goal = support::read_goal(&test.runtime, test.connection_id, test.session_id).await?;
    assert_eq!(goal.context("waiting goal")?.id, test.goal.id);
    let response = test
        .runtime
        .handle_incoming(
            test.connection_id,
            serde_json::json!({
                "id": "interrupt-idle-wait",
                "method": "session/interrupt",
                "params": {"scope": {"scope": "session", "sessionId": test.session_id}}
            }),
        )
        .await
        .context("idle interrupt response")?;
    assert_eq!(response["result"]["interrupted"], true);
    test.finish_work(
        "idle-interrupted-command",
        "late output after idle interrupt",
    );
    test.assert_no_new_turn(/*expected_requests*/ 1).await
}

#[tokio::test]
async fn simultaneous_completions_start_only_one_inbox_turn_at_a_time() -> Result<()> {
    let mut test = WaitingGoal::new().await?;
    for index in 0..12 {
        test.start_work(&format!("parallel-{index}"));
    }
    test.finish_first_turn().await?;
    test.assert_no_new_turn(/*expected_requests*/ 1).await?;
    let mut tasks = Vec::new();
    for index in 0..12 {
        let runtime = Arc::clone(&test.runtime);
        let session_id = test.session_id;
        tasks.push(tokio::spawn(async move {
            runtime.background_work_completed(
                session_id,
                format!("parallel-{index}"),
                format!("result-{index}"),
            );
        }));
    }
    for task in tasks {
        task.await?;
    }
    test.wait_for_second_turn().await?;
    test.assert_no_new_turn(/*expected_requests*/ 2).await?;
    test.stop().await
}

#[tokio::test]
async fn interrupt_and_last_completion_cannot_leave_a_late_goal_turn() -> Result<()> {
    for _ in 0..4 {
        let mut test = WaitingGoal::new().await?;
        test.start_work("racing-command");
        test.finish_first_turn().await?;
        test.assert_no_new_turn(/*expected_requests*/ 1).await?;
        let runtime = Arc::clone(&test.runtime);
        let session = test.session_id;
        let complete = tokio::spawn(async move {
            runtime.background_work_completed(
                session,
                "racing-command".to_string(),
                "racing result".to_string(),
            );
        });
        let response = test
            .runtime
            .handle_incoming(
                test.connection_id,
                serde_json::json!({
                    "id": "racing-interrupt",
                    "method": "session/interrupt",
                    "params": {"scope": {"scope": "session", "sessionId": session}}
                }),
            )
            .await
            .context("racing interrupt response")?;
        complete.await?;
        assert!(response.get("error").is_none(), "{response}");
        // A turn admitted before the interrupt is allowed, but must have been
        // interrupted before the control response. No later admission is valid.
        while test.notifications.try_recv().is_ok() {}
        let count = test.provider.requests.lock().unwrap().len();
        test.assert_no_new_turn(count).await?;
    }
    Ok(())
}
