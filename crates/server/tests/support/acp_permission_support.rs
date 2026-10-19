#![allow(dead_code)]

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_protocol::AcpPromptResult;
use devo_protocol::AcpSessionNotification;
use devo_protocol::AcpSessionUpdate;
use devo_protocol::AcpStopReason;
use devo_protocol::AcpToolCallStatus;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::SessionId;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_server::AcpSuccessResponse;
use devo_server::ServerRuntime;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::timeout;

use super::acp_runtime_harness::AcpTestClient;
use super::acp_runtime_harness::connect_acp_stdio;
use super::acp_runtime_harness::create_acp_session_id;

pub(crate) struct PermissionCase {
    pub outcome: Value,
    pub expect_tool_calls: usize,
    pub expect_success: bool,
    pub assert_legacy_removed: bool,
}

pub(crate) struct PermissionPrompt {
    pub _data_root: TempDir,
    pub runtime: Arc<ServerRuntime>,
    pub connection_id: u64,
    pub notifications_rx: mpsc::Receiver<Value>,
    pub session_id: SessionId,
    pub call_record_path: PathBuf,
}

const IPYTHON_TOOL_NAME: &str = "ipython";

pub(crate) async fn run_permission_case(case: &PermissionCase) -> Result<()> {
    let mut prompt = start_permission_prompt(3).await?;
    if case.assert_legacy_removed {
        assert_legacy_approval_respond_removed(&prompt.runtime, prompt.connection_id).await?;
    }
    let permission_request_id = wait_for_pending_tool_call_and_permission_request(
        &mut prompt.notifications_rx,
        &prompt.session_id,
    )
    .await?;
    assert_eq!(
        prompt
            .runtime
            .handle_incoming(
                prompt.connection_id,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": permission_request_id,
                    "result": { "outcome": case.outcome }
                }),
            )
            .await,
        None
    );
    if case.expect_success {
        wait_for_prompt_completion(&mut prompt.notifications_rx, 3, true).await?;
    } else {
        wait_for_prompt_completion(&mut prompt.notifications_rx, 3, false).await?;
    }
    assert_eq!(
        prompt.call_record_path.is_file(),
        case.expect_tool_calls == 1
    );
    Ok(())
}

pub(crate) async fn start_permission_prompt(prompt_request_id: u64) -> Result<PermissionPrompt> {
    let data_root = TempDir::new()?;
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;
    let call_record_path = cwd.join("ipython_permission_calls.txt");
    let runtime = devo_server::test_support::TestRuntime::new(Arc::new(PermissionToolProvider {
        stream_calls: AtomicUsize::new(0),
        call_record_path: call_record_path.clone(),
    }))
    .with_test_model()
    .db_file("acp_permission_tool_status_contract.db")
    .runtime(data_root.path());
    let connection = connect_acp_stdio(
        &runtime,
        AcpTestClient::new(
            "acp-permission-tool-status-test",
            "ACP Permission Tool Status Test",
        ),
    )
    .await?;
    let session_id = create_acp_session_id(&runtime, connection.connection_id, &cwd, 2).await?;
    assert_eq!(
        runtime
            .handle_incoming(
                connection.connection_id,
                serde_json::json!({
                    "id": prompt_request_id,
                    "method": "session/prompt",
                    "params": {
                        "sessionId": session_id,
                        "prompt": [{ "type": "text", "text": "run an approved Python cell" }]
                    }
                }),
            )
            .await,
        None
    );
    Ok(PermissionPrompt {
        _data_root: data_root,
        runtime,
        connection_id: connection.connection_id,
        notifications_rx: connection.notifications_rx,
        session_id,
        call_record_path,
    })
}

struct PermissionToolProvider {
    stream_calls: AtomicUsize,
    call_record_path: PathBuf,
}

#[async_trait]
impl ModelProviderSDK for PermissionToolProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title-1".to_string(),
            content: vec![ResponseContent::Text(
                "Generated ACP permission title".to_string(),
            )],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let stream_call = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        if stream_call == 0 {
            // Deliberately simulate a provider that adds permission fields not
            // advertised for `ipython`. The server must still require an ACP
            // decision before running this cell.
            let marker_path =
                serde_json::to_string(self.call_record_path.to_string_lossy().as_ref())?;
            let input = serde_json::json!({
                "code": format!(
                    "from pathlib import Path; Path({marker_path}).write_text('called')"
                ),
                "sandbox_permissions": "with_additional_permissions",
                "additional_permissions": { "network": { "enabled": true } },
                "justification": "ACP permission contract"
            });
            return Ok(Box::pin(stream::iter(vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "tool-1".to_string(),
                    name: IPYTHON_TOOL_NAME.to_string(),
                    input: serde_json::json!({}),
                }),
                Ok(StreamEvent::ToolCallInputDelta {
                    index: 0,
                    partial_json: input.to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "tool-call-response".to_string(),
                        content: vec![ResponseContent::ToolUse {
                            id: "tool-1".to_string(),
                            name: IPYTHON_TOOL_NAME.to_string(),
                            input,
                        }],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                }),
            ])));
        }
        Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::MessageDone {
            response: ModelResponse {
                id: "final-response".to_string(),
                content: vec![ResponseContent::Text("Done.".to_string())],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
                metadata: ResponseMetadata::default(),
            },
        })])))
    }

    fn name(&self) -> &str {
        "acp-permission-tool-status-provider"
    }
}

async fn wait_for_pending_tool_call_and_permission_request(
    notifications_rx: &mut mpsc::Receiver<Value>,
    session_id: &SessionId,
) -> Result<u64> {
    let started = Instant::now();
    let mut saw_pending_tool_call = false;
    let mut permission_request_id = None;
    let mut seen = Vec::new();
    loop {
        let value = match timeout(Duration::from_millis(100), notifications_rx.recv()).await {
            Ok(Some(value)) => value,
            Ok(None) => anyhow::bail!(
                "notification channel closed before ACP permission request: {seen:#?}"
            ),
            Err(_) if started.elapsed() > Duration::from_secs(5) => {
                anyhow::bail!("timed out waiting for ACP permission request: {seen:#?}")
            }
            Err(_) => continue,
        };
        seen.push(value.clone());
        if let Some(params) = acp_session_notification_params(&value) {
            let notification: AcpSessionNotification = serde_json::from_value(params)?;
            if notification.session_id != *session_id {
                continue;
            }
            if matches!(
                notification.update,
                AcpSessionUpdate::ToolCall {
                    ref tool_call_id,
                    status: Some(AcpToolCallStatus::Pending),
                    ..
                } if tool_call_id == "tool-1"
            ) {
                saw_pending_tool_call = true;
            }
        } else if value.get("method") == Some(&serde_json::json!("session/request_permission")) {
            assert_eq!(value["params"]["sessionId"], serde_json::json!(session_id));
            assert_eq!(
                value["params"]["toolCall"]["toolCallId"],
                serde_json::json!("tool-1")
            );
            assert_eq!(
                value["params"]["toolCall"]["status"],
                serde_json::json!("pending")
            );
            assert_eq!(
                value["params"]["options"][0]["optionId"],
                serde_json::json!("allow_once")
            );
            permission_request_id = value.get("id").and_then(Value::as_u64);
        }
        if saw_pending_tool_call && let Some(permission_request_id) = permission_request_id {
            return Ok(permission_request_id);
        }
    }
}

async fn wait_for_prompt_completion(
    notifications_rx: &mut mpsc::Receiver<Value>,
    prompt_request_id: u64,
    expect_success: bool,
) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        let mut saw_in_progress = false;
        let mut saw_terminal = false;
        let mut saw_prompt_response = false;
        while let Some(value) = notifications_rx.recv().await {
            if let Some(params) = acp_session_notification_params(&value) {
                let notification: AcpSessionNotification = serde_json::from_value(params)?;
                match notification.update {
                    AcpSessionUpdate::ToolCallUpdate {
                        tool_call_id,
                        status: Some(AcpToolCallStatus::InProgress),
                        ..
                    } if tool_call_id == "tool-1" => {
                        if !expect_success {
                            anyhow::bail!("denied permission unexpectedly started tool execution");
                        }
                        saw_in_progress = true;
                    }
                    AcpSessionUpdate::ToolCallUpdate {
                        tool_call_id,
                        status: Some(AcpToolCallStatus::Completed),
                        ..
                    } if tool_call_id == "tool-1" && expect_success => saw_terminal = true,
                    AcpSessionUpdate::ToolCallUpdate {
                        tool_call_id,
                        status: Some(AcpToolCallStatus::Failed),
                        ..
                    } if tool_call_id == "tool-1" && !expect_success => saw_terminal = true,
                    _ => {}
                }
            } else if value.get("id") == Some(&serde_json::json!(prompt_request_id)) {
                let response: AcpSuccessResponse<AcpPromptResult> = serde_json::from_value(value)?;
                assert_eq!(response.result.stop_reason, AcpStopReason::EndTurn);
                saw_prompt_response = true;
            }
            if saw_prompt_response
                && (expect_success && saw_in_progress && saw_terminal
                    || !expect_success && saw_terminal)
            {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before ACP prompt completed")
    })
    .await
    .context("timed out waiting for ACP prompt completion")?
}

fn acp_session_notification_params(value: &Value) -> Option<Value> {
    if value.get("method") == Some(&serde_json::json!("session/update")) {
        return value.get("params").cloned();
    }
    value.get("update").is_some().then(|| value.clone())
}

async fn assert_legacy_approval_respond_removed(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({ "id": 99, "method": "approval/respond", "params": {} }),
        )
        .await
        .context("legacy approval/respond response")?;
    assert_eq!(response["id"], serde_json::json!(99));
    assert_eq!(
        response["error"]["code"],
        serde_json::json!(devo_protocol::AcpErrorCode::MethodNotFound as i32)
    );
    assert_eq!(
        response["error"]["message"],
        serde_json::json!("unknown ACP method: approval/respond")
    );
    Ok(())
}
