use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::{
    ModelRequest, ModelResponse, RequestContent, ResponseContent, ResponseMetadata, StopReason,
    StreamEvent, Usage,
};
use devo_provider::ModelProviderSDK;
use futures::Stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[path = "support/subagent_lifecycle.rs"]
#[allow(dead_code)]
mod support;

use support::{
    build_runtime, initialize_connection, start_parent_session, start_turn,
    wait_for_parent_turn_completed,
};

#[derive(Default)]
struct InteractiveExecProvider {
    calls: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
}

#[derive(Clone, Copy)]
enum BackgroundWorkflow {
    Complete,
    Cancel,
}

struct BackgroundExecProvider {
    workflow: BackgroundWorkflow,
    calls: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
}

impl BackgroundExecProvider {
    fn new(workflow: BackgroundWorkflow) -> Self {
        Self {
            workflow,
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("requests lock").clone()
    }
}

impl InteractiveExecProvider {
    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("requests lock").clone()
    }
}

#[async_trait]
impl ModelProviderSDK for InteractiveExecProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("interactive exec test uses streaming completion")
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests
            .lock()
            .expect("requests lock")
            .push(request.clone());
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let events = match call {
            0 => tool_call_events(
                "exec-1",
                "exec_command",
                serde_json::json!({
                    "cmd": interactive_command(),
                    "login": false,
                    "tty": true,
                    "yield_time_ms": 50,
                    "max_output_tokens": 1000
                }),
            ),
            1 => {
                let process_id = extract_process_id(&request)?;
                tool_call_events(
                    "stdin-1",
                    "write_stdin",
                    serde_json::json!({
                        "process_id": process_id,
                        "chars": "hello\n",
                        "yield_time_ms": 5000,
                        "max_output_tokens": 1000
                    }),
                )
            }
            2 => text_events("interactive command completed"),
            _ => anyhow::bail!("unexpected provider call {call}"),
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }

    fn name(&self) -> &str {
        "interactive-exec-provider"
    }
}

#[async_trait]
impl ModelProviderSDK for BackgroundExecProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("background exec test uses streaming completion")
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests
            .lock()
            .expect("requests lock")
            .push(request.clone());
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let events = match (self.workflow, call) {
            (workflow, 0) => tool_call_events(
                "background-exec-1",
                "exec_command",
                serde_json::json!({
                    "cmd": background_command(workflow),
                    "login": false,
                    "tty": true,
                    "execution_mode": "background",
                    "max_output_tokens": 1000
                }),
            ),
            (BackgroundWorkflow::Complete, 1) => {
                tool_call_events("list-tasks-1", "list_tasks", serde_json::json!({}))
            }
            (BackgroundWorkflow::Complete, 2) => tool_call_events(
                "await-task-1",
                "await_task",
                serde_json::json!({
                    "task_id": extract_background_task_id(&request)?,
                    "timeout_secs": 2
                }),
            ),
            (BackgroundWorkflow::Cancel, 1) => tool_call_events(
                "await-task-1",
                "await_task",
                serde_json::json!({
                    "task_id": extract_background_task_id(&request)?,
                    "timeout_secs": 0
                }),
            ),
            (BackgroundWorkflow::Cancel, 2) => tool_call_events(
                "cancel-task-1",
                "cancel_task",
                serde_json::json!({
                    "task_id": extract_background_task_id(&request)?
                }),
            ),
            (BackgroundWorkflow::Cancel, 3) => {
                tool_call_events("list-tasks-1", "list_tasks", serde_json::json!({}))
            }
            (BackgroundWorkflow::Complete, 3) | (BackgroundWorkflow::Cancel, 4) => {
                text_events("background workflow completed")
            }
            (_, unexpected) => anyhow::bail!("unexpected provider call {unexpected}"),
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }

    fn name(&self) -> &str {
        "background-exec-provider"
    }
}

#[cfg(unix)]
fn interactive_command() -> &'static str {
    "printf 'ready\\n'; IFS= read -r line; printf 'received:%s\\n' \"$line\"; sleep 0.1; printf 'done\\n'"
}

#[cfg(unix)]
fn background_command(workflow: BackgroundWorkflow) -> &'static str {
    match workflow {
        BackgroundWorkflow::Complete => "sleep 0.2; printf 'background-done\\n'",
        BackgroundWorkflow::Cancel => "sleep 5; printf 'should-not-finish\\n'",
    }
}

#[cfg(windows)]
fn background_command(workflow: BackgroundWorkflow) -> &'static str {
    match workflow {
        BackgroundWorkflow::Complete => {
            "Start-Sleep -Milliseconds 200; Write-Output 'background-done'"
        }
        BackgroundWorkflow::Cancel => "Start-Sleep -Seconds 5; Write-Output 'should-not-finish'",
    }
}

#[cfg(windows)]
fn interactive_command() -> &'static str {
    "Write-Output 'ready'; $line = Read-Host; Write-Output \"received:$line\"; Start-Sleep -Milliseconds 100; Write-Output 'done'"
}

fn extract_process_id(request: &ModelRequest) -> Result<i64> {
    let content = request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|content| match content {
            RequestContent::ToolResult {
                tool_use_id,
                content,
                ..
            } if tool_use_id == "exec-1" => Some(content.as_str()),
            _ => None,
        })
        .context("exec_command tool result")?;
    let marker = "Process running with process ID ";
    let process_id = content
        .lines()
        .find_map(|line| line.strip_prefix(marker))
        .context("running process id")?;
    process_id.parse().context("parse process id")
}

fn extract_background_task_id(request: &ModelRequest) -> Result<String> {
    let content = tool_result(request, "background-exec-1").context("background exec result")?;
    let marker = "Command running as background task ";
    content
        .lines()
        .find_map(|line| line.strip_prefix(marker))
        .map(str::to_string)
        .context("background task id")
}

fn tool_call_events(id: &str, name: &str, input: serde_json::Value) -> Vec<StreamEvent> {
    vec![
        StreamEvent::ToolCallStart {
            index: 0,
            id: id.to_string(),
            name: name.to_string(),
            input: input.clone(),
        },
        StreamEvent::MessageDone {
            response: ModelResponse {
                id: format!("response-{id}"),
                content: vec![ResponseContent::ToolUse {
                    id: id.to_string(),
                    name: name.to_string(),
                    input,
                }],
                stop_reason: Some(StopReason::ToolUse),
                usage: Usage::default(),
                metadata: ResponseMetadata::default(),
            },
        },
    ]
}

fn text_events(text: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::TextDelta {
            index: 0,
            text: text.to_string(),
        },
        StreamEvent::MessageDone {
            response: ModelResponse {
                id: "response-final".to_string(),
                content: vec![ResponseContent::Text(text.to_string())],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
                metadata: ResponseMetadata::default(),
            },
        },
    ]
}

#[tokio::test]
async fn long_running_exec_command_accepts_write_stdin_and_exits() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(InteractiveExecProvider::default());
    let runtime = build_runtime(data_root.path(), Arc::clone(&provider) as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;

    start_turn(
        &runtime,
        connection_id,
        session_id,
        "run the interactive command",
    )
    .await?;
    wait_for_parent_turn_completed(&mut notifications_rx, session_id).await?;

    let requests = provider.requests();
    assert_eq!(requests.len(), 3);
    let exec_result = tool_result(&requests[1], "exec-1").context("exec result")?;
    assert!(exec_result.contains("Process running with process ID"));
    assert!(exec_result.contains("ready"));
    let stdin_result = tool_result(&requests[2], "stdin-1").context("stdin result")?;
    assert!(stdin_result.contains("received:hello"));
    assert!(stdin_result.contains("done"));
    assert!(stdin_result.contains("Process exited with code 0"));

    Ok(())
}

#[tokio::test]
async fn background_exec_lists_and_awaits_terminal_command_task() -> Result<()> {
    let provider = Arc::new(BackgroundExecProvider::new(BackgroundWorkflow::Complete));
    run_background_workflow(Arc::clone(&provider)).await?;

    let requests = provider.requests();
    assert_eq!(requests.len(), 4);
    let listed = tool_result(&requests[2], "list-tasks-1").context("list task result")?;
    assert!(listed.contains("\"kind\":\"command\""));
    assert!(listed.contains("\"state\":\"running\""));
    assert!(listed.contains("process_id"));
    assert!(!listed.contains("process_session_id"));
    let awaited = tool_result(&requests[3], "await-task-1").context("await task result")?;
    assert!(awaited.contains("\"outcome\":\"terminal\""));
    assert!(awaited.contains("\"state\":\"completed\""));
    assert!(awaited.contains("background-done"));

    Ok(())
}

#[tokio::test]
async fn cancel_task_terminates_background_command_and_preserves_canceled_listing() -> Result<()> {
    let provider = Arc::new(BackgroundExecProvider::new(BackgroundWorkflow::Cancel));
    run_background_workflow(Arc::clone(&provider)).await?;

    let requests = provider.requests();
    assert_eq!(requests.len(), 5);
    let timed_out = tool_result(&requests[2], "await-task-1").context("await task result")?;
    assert!(timed_out.contains("\"outcome\":\"timed_out\""));
    assert!(!timed_out.contains("\"output\""));
    let canceled = tool_result(&requests[3], "cancel-task-1").context("cancel task result")?;
    assert!(canceled.contains("\"state\":\"canceled\""));
    let listed = tool_result(&requests[4], "list-tasks-1").context("list task result")?;
    assert!(listed.contains("\"kind\":\"command\""));
    assert!(listed.contains("\"state\":\"canceled\""));

    Ok(())
}

async fn run_background_workflow(provider: Arc<BackgroundExecProvider>) -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path(), provider as _)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_parent_session(&runtime, connection_id, data_root.path()).await?;

    start_turn(
        &runtime,
        connection_id,
        session_id,
        "run the background workflow",
    )
    .await?;
    wait_for_parent_turn_completed(&mut notifications_rx, session_id).await?;

    Ok(())
}

fn tool_result<'a>(request: &'a ModelRequest, tool_use_id: &str) -> Option<&'a str> {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|content| match content {
            RequestContent::ToolResult {
                tool_use_id: result_id,
                content,
                ..
            } if result_id == tool_use_id => Some(content.as_str()),
            _ => None,
        })
}
