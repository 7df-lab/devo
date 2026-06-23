use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AppConfigStore;
use devo_core::BundledSkillsConfig;
use devo_core::FileSystemSkillCatalog;
use devo_core::Model;
use devo_core::PresetModelCatalog;
use devo_core::ProviderVendorCatalog;
use devo_core::ReasoningCapability;
use devo_core::SkillsConfig;
use devo_core::tools::create_default_tool_registry;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningEffort;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::ServerEvent;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::timeout;

struct UnusedProvider;

#[async_trait]
impl ModelProviderSDK for UnusedProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("unused provider should not receive completion requests")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        anyhow::bail!("unused provider should not receive streaming requests")
    }

    fn name(&self) -> &str {
        "unused-provider"
    }
}

#[derive(Default)]
struct IncompleteFinalReportProvider {
    stream_calls: AtomicUsize,
    final_report_stream_calls: AtomicUsize,
}

#[async_trait]
impl ModelProviderSDK for IncompleteFinalReportProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        let prompt = request_text(&request);
        let text = if prompt_has_stage(&prompt, "clarification gate")
            || prompt.contains("\"need_clarification\"")
        {
            r#"{"need_clarification":false,"question":"","verification":"Research DeepSeek official website."}"#
                .to_string()
        } else if prompt_has_stage(&prompt, "research brief") {
            "## Objective\nResearch DeepSeek official website.\n\n## Scope\nCurrent official website.\n\n## Report Language\nEnglish"
                .to_string()
        } else if prompt_has_stage(&prompt, "supervisor task plan") {
            r#"{"tasks":[{"title":"Official website","research_topic":"Find the current official DeepSeek website.","purpose":"Answer the brief","source_strategy":"Use official sources","success_criteria":"Capture the official domain"}]}"#
                .to_string()
        } else if prompt_has_stage(&prompt, "evidence pack compression") {
            "Evidence pack: DeepSeek official website is https://www.deepseek.com/".to_string()
        } else {
            "Deep research mock title".to_string()
        };
        Ok(model_response(text))
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let prompt = request_text(&request);
        let _stream_call_index = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let events = if prompt_has_stage(&prompt, "clarification gate")
            || prompt.contains("\"need_clarification\"")
        {
            streamed_text_events(
                r#"{"need_clarification":false,"question":"","verification":"Research DeepSeek official website."}"#,
            )
        } else if prompt_has_stage(&prompt, "research brief") {
            streamed_text_events(
                "## Objective\nResearch DeepSeek official website.\n\n## Scope\nCurrent official website.\n\n## Report Language\nEnglish",
            )
        } else if prompt_has_stage(&prompt, "supervisor task plan") {
            streamed_text_events(
                r#"{"tasks":[{"title":"Official website","research_topic":"Find the current official DeepSeek website.","purpose":"Answer the brief","source_strategy":"Use official sources","success_criteria":"Capture the official domain"}]}"#,
            )
        } else if prompt_has_stage(&prompt, "evidence pack compression") {
            streamed_text_events(
                "Evidence pack: DeepSeek official website is https://www.deepseek.com/",
            )
        } else if prompt_has_stage(&prompt, "delegated deep research worker") {
            vec![
                Ok(StreamEvent::TextDelta {
                    index: 1,
                    text: "Researcher notes: official source https://www.deepseek.com/".to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: model_response(
                        "Researcher notes: official source https://www.deepseek.com/",
                    ),
                }),
            ]
        } else if prompt_has_stage(&prompt, "final report writing") {
            let final_report_call_index = self
                .final_report_stream_calls
                .fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                final_report_call_index, 0,
                "final report should be the first final-report stream call"
            );
            vec![Ok(StreamEvent::TextDelta {
                index: 1,
                text: "Partial final report before stream completion".to_string(),
            })]
        } else {
            assert!(
                !prompt.contains("Partial final report before stream completion"),
                "regular turn leaked partial final report: {prompt}"
            );
            assert!(
                !prompt.contains("Research Context Reference"),
                "regular turn leaked compact reference from failed research: {prompt}"
            );
            assert!(
                !prompt.contains("Evidence pack:"),
                "regular turn leaked compressed research internals: {prompt}"
            );
            vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "Ordinary turn did not see partial research context.".to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: model_response("Ordinary turn did not see partial research context."),
                }),
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }

    fn name(&self) -> &str {
        "incomplete-final-report-provider"
    }
}

#[tokio::test]
async fn regular_turn_after_incomplete_research_does_not_receive_partial_handoff() -> Result<()> {
    // Trace: L2-DES-RESEARCH-001
    // Verifies: failed partial final-report streams do not become prompt-visible research handoffs.
    let workspace = TempDir::new()?;
    write_research_config(workspace.path())?;
    let runtime = build_runtime(workspace.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, workspace.path()).await?;

    start_research_turn(&runtime, connection_id, session_id).await?;
    let failed_events = wait_for_turn_failed(&mut notifications_rx, session_id).await?;
    assert!(
        failed_events.iter().any(|event| {
            event.get("method") == Some(&serde_json::json!("item/completed"))
                && event["params"]["item"]["item_kind"] == serde_json::json!("research_artifact")
                && event["params"]["item"]["payload"]["artifact_type"]
                    == serde_json::json!("failure")
        }),
        "research failures should complete as research artifacts: {failed_events:#?}"
    );
    assert!(
        failed_events.iter().all(|event| {
            event.get("method") != Some(&serde_json::json!("item/completed"))
                || event["params"]["item"]["item_kind"] != serde_json::json!("agent_message")
                || !event["params"]["item"]["payload"]["text"]
                    .as_str()
                    .is_some_and(|text| {
                        text.contains("Partial final report before stream completion")
                    })
        }),
        "partial final report should not complete as an agent message: {failed_events:#?}"
    );

    start_regular_turn(&runtime, connection_id, session_id).await?;
    wait_for_turn_completed(&mut notifications_rx, session_id).await?;

    Ok(())
}

fn write_research_config(root: &std::path::Path) -> Result<()> {
    std::fs::write(
        root.join("config.toml"),
        r#"
[tools.web_search]
mode = "provider"

[research]
max_concurrent_tasks = 1
max_tasks = 1
max_researcher_iterations = 1
fetch_summary_threshold_chars = 2000
max_summary_chars = 1000
"#,
    )?;
    Ok(())
}

fn build_runtime(data_root: &std::path::Path) -> Result<Arc<ServerRuntime>> {
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(IncompleteFinalReportProvider::default());
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("deep_research_boundary_failure.db"),
    )?);
    Ok(ServerRuntime::new(
        data_root.to_path_buf(),
        ServerRuntimeDependencies::new(
            Arc::new(UnusedProvider),
            Arc::new(SingleProviderRouter::new(provider)),
            Arc::new(create_default_tool_registry()),
            "deepseek-v4-flash".to_string(),
            Arc::new(PresetModelCatalog::new(vec![deepseek_model()])),
            Arc::new(ProviderVendorCatalog::default()),
            Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                bundled: Some(BundledSkillsConfig { enabled: false }),
                ..SkillsConfig::default()
            })),
            devo_core::AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(AppConfigStore::load(
                data_root.to_path_buf(),
                /*workspace_root*/ None,
            )?)),
        ),
    ))
}

fn deepseek_model() -> Model {
    Model {
        slug: "deepseek-v4-flash".to_string(),
        display_name: "DeepSeek V4 Flash".to_string(),
        provider: ProviderWireApi::AnthropicMessages,
        reasoning_capability: ReasoningCapability::Unsupported,
        default_reasoning_effort: Some(ReasoningEffort::Low),
        base_instructions: "Follow the developer instructions.".to_string(),
        max_tokens: Some(2048),
        temperature: Some(0.1),
        ..Model::default()
    }
}

fn request_text(request: &ModelRequest) -> String {
    let mut parts = Vec::new();
    if let Some(system) = request.system.as_deref() {
        parts.push(system.to_string());
    }
    parts.extend(request.messages.iter().map(request_message_text));
    parts.join("\n")
}

fn request_message_text(message: &devo_protocol::RequestMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|content| match content {
            devo_protocol::RequestContent::Text { text } => Some(text.as_str()),
            devo_protocol::RequestContent::Reasoning { .. }
            | devo_protocol::RequestContent::ProviderReasoning { .. }
            | devo_protocol::RequestContent::ToolUse { .. }
            | devo_protocol::RequestContent::HostedToolUse { .. }
            | devo_protocol::RequestContent::ToolResult { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn prompt_has_stage(prompt: &str, stage: &str) -> bool {
    prompt
        .to_ascii_lowercase()
        .contains(&format!("stage: {stage}"))
}

fn model_response(text: impl Into<String>) -> ModelResponse {
    ModelResponse {
        id: "scripted-response".to_string(),
        content: vec![ResponseContent::Text(text.into())],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage {
            input_tokens: 1,
            output_tokens: 1,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            reasoning_output_tokens: None,
            total_tokens: None,
        },
        metadata: ResponseMetadata::default(),
    }
}

fn streamed_text_events(text: impl Into<String>) -> Vec<Result<StreamEvent>> {
    let text = text.into();
    vec![
        Ok(StreamEvent::TextDelta {
            index: 0,
            text: text.clone(),
        }),
        Ok(StreamEvent::MessageDone {
            response: model_response(text),
        }),
    ]
}

async fn initialize_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = mpsc::channel(/*buffer*/ 256);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let initialize_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": {
                        "name": "deep-research-boundary-test",
                        "title": "deep-research-boundary-test",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    let response: serde_json::Value = initialize_response;
    assert_eq!(
        response["result"]["agentInfo"]["name"],
        serde_json::json!("devo-server")
    );
    Ok((connection_id, notifications_rx))
}

async fn start_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &std::path::Path,
) -> Result<devo_core::SessionId> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": cwd,
                    "additionalDirectories": []
                }
            }),
        )
        .await
        .context("session/new response")?;
    let response: devo_server::SuccessResponse<devo_protocol::AcpNewSessionResult> =
        serde_json::from_value(response.clone())
            .with_context(|| format!("session/new returned {response}"))?;
    Ok(response.result.session_id)
}

async fn start_research_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_core::SessionId,
) -> Result<()> {
    start_turn(runtime, connection_id, session_id, "research").await
}

async fn start_regular_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_core::SessionId,
) -> Result<()> {
    start_turn(runtime, connection_id, session_id, "regular").await
}

async fn start_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_core::SessionId,
    execution_mode: &str,
) -> Result<()> {
    let text = if execution_mode == "research" {
        "Research the current official DeepSeek website domain."
    } else {
        "Now answer as a normal coding turn using the research context."
    };
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": text }],
                    "model": "deepseek-v4-flash",
                    "model_binding_id": null,
                    "reasoning_effort_selection": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null,
                    "collaboration_mode": "build",
                    "execution_mode": execution_mode
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(response.clone())
            .with_context(|| format!("turn/start returned {response}"))?;
    Ok(())
}

async fn wait_for_turn_failed(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    session_id: devo_core::SessionId,
) -> Result<Vec<serde_json::Value>> {
    wait_for_terminal_turn_event(notifications_rx, session_id, "turn/failed").await
}

async fn wait_for_turn_completed(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    session_id: devo_core::SessionId,
) -> Result<Vec<serde_json::Value>> {
    wait_for_terminal_turn_event(notifications_rx, session_id, "turn/completed").await
}

async fn wait_for_terminal_turn_event(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    session_id: devo_core::SessionId,
    expected_method: &str,
) -> Result<Vec<serde_json::Value>> {
    let mut events = Vec::new();
    timeout(Duration::from_secs(60), async {
        while let Some(event) = notifications_rx.recv().await {
            let event = legacy_event_from_acp_notification(event);
            let method = event
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let matches_session =
                event["params"]["session_id"] == serde_json::json!(session_id.to_string());
            events.push(event);
            if method == expected_method && matches_session {
                return Ok(events);
            }
            if matches_session && matches!(method.as_str(), "turn/completed" | "turn/failed") {
                anyhow::bail!("expected {expected_method}, received {method}");
            }
        }
        anyhow::bail!("notification channel closed before {expected_method}")
    })
    .await
    .with_context(|| format!("timed out waiting for {expected_method}"))?
}

fn legacy_event_from_acp_notification(value: serde_json::Value) -> serde_json::Value {
    if value.get("method") != Some(&serde_json::json!("session/update")) {
        return value;
    }
    let Ok(notification) =
        serde_json::from_value::<devo_protocol::AcpSessionNotification>(value["params"].clone())
    else {
        return value;
    };
    let Some((method, event)) = devo_protocol::original_event_from_acp_notification(&notification)
    else {
        return value;
    };
    let params = match event {
        ServerEvent::TurnCompleted(payload)
        | ServerEvent::TurnInterrupted(payload)
        | ServerEvent::TurnFailed(payload)
        | ServerEvent::TurnStarted(payload) => serde_json::to_value(payload),
        ServerEvent::ItemCompleted(payload) | ServerEvent::ItemStarted(payload) => {
            serde_json::to_value(payload)
        }
        other => serde_json::to_value(other),
    }
    .expect("serialize legacy event params");
    serde_json::json!({
        "method": method,
        "params": params,
    })
}
