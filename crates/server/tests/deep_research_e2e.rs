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
use devo_protocol::RequestContent;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::ServerEvent;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_provider::anthropic::AnthropicProvider;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::Notify;
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
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        anyhow::bail!("unused provider should not receive streaming requests")
    }

    fn name(&self) -> &str {
        "unused-provider"
    }
}

struct ScriptedResearchProvider {
    stream_calls: AtomicUsize,
    final_report_stream_calls: AtomicUsize,
    delegated_worker_failures_before_success: AtomicUsize,
    researcher_gate: Option<Arc<Notify>>,
    final_report_mode: ScriptedFinalReportMode,
    expected_cwd: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScriptedFinalReportMode {
    Text,
    WriteToolOnly,
}

impl ScriptedResearchProvider {
    fn new(expected_cwd: &std::path::Path) -> Self {
        Self {
            stream_calls: AtomicUsize::new(0),
            final_report_stream_calls: AtomicUsize::new(0),
            delegated_worker_failures_before_success: AtomicUsize::new(0),
            researcher_gate: None,
            final_report_mode: ScriptedFinalReportMode::Text,
            expected_cwd: expected_cwd.display().to_string(),
        }
    }

    fn with_researcher_gate(expected_cwd: &std::path::Path, researcher_gate: Arc<Notify>) -> Self {
        Self {
            stream_calls: AtomicUsize::new(0),
            final_report_stream_calls: AtomicUsize::new(0),
            delegated_worker_failures_before_success: AtomicUsize::new(0),
            researcher_gate: Some(researcher_gate),
            final_report_mode: ScriptedFinalReportMode::Text,
            expected_cwd: expected_cwd.display().to_string(),
        }
    }

    fn with_write_tool_only_final_report(expected_cwd: &std::path::Path) -> Self {
        Self {
            stream_calls: AtomicUsize::new(0),
            final_report_stream_calls: AtomicUsize::new(0),
            delegated_worker_failures_before_success: AtomicUsize::new(0),
            researcher_gate: None,
            final_report_mode: ScriptedFinalReportMode::WriteToolOnly,
            expected_cwd: expected_cwd.display().to_string(),
        }
    }

    fn with_delegated_worker_failure_once(expected_cwd: &std::path::Path) -> Self {
        Self::with_delegated_worker_failures_before_success(expected_cwd, 1)
    }

    fn with_delegated_worker_failures_before_success(
        expected_cwd: &std::path::Path,
        failures: usize,
    ) -> Self {
        Self {
            stream_calls: AtomicUsize::new(0),
            final_report_stream_calls: AtomicUsize::new(0),
            delegated_worker_failures_before_success: AtomicUsize::new(failures),
            researcher_gate: None,
            final_report_mode: ScriptedFinalReportMode::Text,
            expected_cwd: expected_cwd.display().to_string(),
        }
    }
}

#[async_trait]
impl ModelProviderSDK for ScriptedResearchProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        let prompt = request_text(&request);
        if is_research_request(&request) {
            assert_research_environment_contains_cwd(&request, &self.expected_cwd);
        }
        let text = if prompt_has_stage(&prompt, "clarification gate")
            || prompt.contains("\"need_clarification\"")
        {
            assert!(
                request.messages.iter().any(|message| {
                    request_message_text(message)
                        == "Research the current official DeepSeek website domain. Use web search, keep the final report short, and include source URLs."
                }),
                "research question should remain a standalone user-role message: {prompt}"
            );
            assert!(
                !request
                    .system
                    .as_deref()
                    .unwrap_or_default()
                    .contains("Research the current official DeepSeek website domain"),
                "research question should not be injected into system prompt"
            );
            r#"{"need_clarification":false,"question":"","verification":"Research DeepSeek official website."}"#
                .to_string()
        } else if prompt_has_stage(&prompt, "research brief") {
            "## Objective\nResearch DeepSeek official website.\n\n## Scope\nCurrent official website.\n\n## Constraints And Preferences\nKeep it short.\n\n## Source Preferences\nOpen-ended.\n\n## Open Dimensions\nNone.\n\n## Report Language\nEnglish"
                .to_string()
        } else if prompt_has_stage(&prompt, "supervisor task plan") {
            r#"{"tasks":[{"title":"Official website","research_topic":"Find the current official DeepSeek website and citation details.","purpose":"Answer the brief","source_strategy":"Use official and search-result sources","success_criteria":"A visible URL and citation details are captured"}]}"#
                .to_string()
        } else if prompt_has_stage(&prompt, "evidence pack compression") {
            assert_compress_request_uses_structured_context(&request);
            "Evidence pack: DeepSeek official website is https://www.deepseek.com/".to_string()
        } else {
            "Deep research mock title".to_string()
        };
        Ok(model_response(text))
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        let prompt = request_text(&request);
        if is_research_request(&request) {
            assert_research_environment_contains_cwd(&request, &self.expected_cwd);
        }
        let _stream_call_index = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        let events = if prompt_has_stage(&prompt, "clarification gate")
            || prompt.contains("\"need_clarification\"")
        {
            assert!(
                request.messages.iter().any(|message| {
                    request_message_text(message)
                        == "Research the current official DeepSeek website domain. Use web search, keep the final report short, and include source URLs."
                }),
                "research question should remain a standalone user-role message: {prompt}"
            );
            assert!(
                !request
                    .system
                    .as_deref()
                    .unwrap_or_default()
                    .contains("Research the current official DeepSeek website domain"),
                "research question should not be injected into system prompt"
            );
            streamed_text_events(
                r#"{"need_clarification":false,"question":"","verification":"Research DeepSeek official website."}"#,
            )
        } else if prompt_has_stage(&prompt, "research brief") {
            streamed_text_events(
                "## Objective\nResearch DeepSeek official website.\n\n## Scope\nCurrent official website.\n\n## Constraints And Preferences\nKeep it short.\n\n## Source Preferences\nOpen-ended.\n\n## Open Dimensions\nNone.\n\n## Report Language\nEnglish",
            )
        } else if prompt_has_stage(&prompt, "supervisor task plan") {
            streamed_text_events(
                r#"{"tasks":[{"title":"Official website","research_topic":"Find the current official DeepSeek website and citation details.","purpose":"Answer the brief","source_strategy":"Use official and search-result sources","success_criteria":"A visible URL and citation details are captured"}]}"#,
            )
        } else if prompt_has_stage(&prompt, "evidence pack compression") {
            assert_compress_request_uses_structured_context(&request);
            streamed_text_events(
                "Evidence pack: DeepSeek official website is https://www.deepseek.com/",
            )
        } else if prompt_has_stage(&prompt, "fetched webpage summarization") {
            streamed_text_events(r#"{"summary":"DeepSeek official website details."}"#)
        } else if prompt_has_stage(&prompt, "delegated deep research worker") {
            assert!(
                prompt.contains("<research_brief>"),
                "delegated worker prompt should include overall brief"
            );
            assert!(
                prompt.contains("<original_research_question>"),
                "delegated worker prompt should include original question"
            );
            let tool_names = request
                .tools
                .as_ref()
                .map(|tools| {
                    tools
                        .iter()
                        .map(|tool| tool.name.as_str())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            assert_eq!(
                tool_names,
                vec!["read", "write", "apply_patch", "webfetch"],
                "delegated worker requests should expose research worker tools without coordination tools"
            );
            assert!(
                request
                    .hosted_tools
                    .iter()
                    .any(|tool| matches!(tool, devo_protocol::HostedToolDefinition::WebSearch(_))),
                "delegated worker requests should preserve provider-hosted web search"
            );
            if self
                .delegated_worker_failures_before_success
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    if remaining > 0 {
                        Some(remaining - 1)
                    } else {
                        None
                    }
                })
                .is_ok()
            {
                return Ok(Box::pin(stream::iter(vec![Err(anyhow::anyhow!(
                    "simulated delegated worker failure"
                ))])));
            }
            if let Some(researcher_gate) = self.researcher_gate.as_ref().cloned() {
                return Ok(Box::pin(stream::unfold(
                    (0_u8, researcher_gate),
                    |(state, researcher_gate)| async move {
                        match state {
                            0 => Some((
                                Ok(StreamEvent::TextDelta {
                                    index: 1,
                                    text: "Researcher notes before completion".to_string(),
                                }),
                                (1, researcher_gate),
                            )),
                            1 => {
                                researcher_gate.notified().await;
                                Some((
                                    Ok(StreamEvent::MessageDone {
                                        response: model_response(
                                            "Researcher notes before completion",
                                        ),
                                    }),
                                    (2, researcher_gate),
                                ))
                            }
                            2 => None,
                            _ => None,
                        }
                    },
                )));
            }
            vec![
                Ok(StreamEvent::ReasoningDelta {
                    index: 0,
                    text: "checking source".to_string(),
                }),
                Ok(StreamEvent::TextDelta {
                    index: 1,
                    text: "Researcher notes: official source https://www.deepseek.com/".to_string(),
                }),
                Ok(StreamEvent::ReasoningDone { index: 0 }),
                Ok(StreamEvent::MessageDone {
                    response: hosted_web_search_researcher_response(),
                }),
            ]
        } else if prompt_has_stage(&prompt, "final report writing") {
            let final_report_call_index = self
                .final_report_stream_calls
                .fetch_add(1, Ordering::SeqCst);
            match self.final_report_mode {
                ScriptedFinalReportMode::Text => {
                    assert_eq!(
                        final_report_call_index, 0,
                        "final report should be the first final-report stream call"
                    );
                    vec![
                        Ok(StreamEvent::ReasoningDelta {
                            index: 0,
                            text: "writing report".to_string(),
                        }),
                        Ok(StreamEvent::TextDelta {
                            index: 1,
                            text: "Final report: DeepSeek official website is ".to_string(),
                        }),
                        Ok(StreamEvent::TextDelta {
                            index: 1,
                            text: "https://www.deepseek.com/.\n\n## Sources\n- https://www.deepseek.com/"
                                .to_string(),
                        }),
                        Ok(StreamEvent::ReasoningDone { index: 0 }),
                        Ok(StreamEvent::MessageDone {
                            response: model_response(
                                "Final report: DeepSeek official website is https://www.deepseek.com/.\n\n## Sources\n- https://www.deepseek.com/",
                            ),
                        }),
                    ]
                }
                ScriptedFinalReportMode::WriteToolOnly if final_report_call_index == 0 => {
                    let input = serde_json::json!({
                        "filePath": "tool-written-report.md",
                        "content": "Final report: DeepSeek official website is https://www.deepseek.com/.\n\n## Sources\n- https://www.deepseek.com/",
                    });
                    vec![
                        Ok(StreamEvent::ToolCallStart {
                            index: 0,
                            id: "write-final-report".to_string(),
                            name: "write".to_string(),
                            input: input.clone(),
                        }),
                        Ok(StreamEvent::MessageDone {
                            response: ModelResponse {
                                id: "write-final-report-response".to_string(),
                                content: vec![ResponseContent::ToolUse {
                                    id: "write-final-report".to_string(),
                                    name: "write".to_string(),
                                    input,
                                }],
                                stop_reason: Some(StopReason::ToolUse),
                                usage: Usage::default(),
                                metadata: ResponseMetadata::default(),
                            },
                        }),
                    ]
                }
                ScriptedFinalReportMode::WriteToolOnly => {
                    assert_eq!(
                        final_report_call_index, 1,
                        "final report should complete after the write tool result"
                    );
                    vec![Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "write-final-report-done".to_string(),
                            content: Vec::new(),
                            stop_reason: Some(StopReason::EndTurn),
                            usage: Usage::default(),
                            metadata: ResponseMetadata::default(),
                        },
                    })]
                }
            }
        } else {
            assert!(
                prompt.contains(
                    "Final report: DeepSeek official website is https://www.deepseek.com/."
                ),
                "regular turn should receive final report: {prompt}"
            );
            assert_eq!(
                prompt
                    .matches(
                        "Final report: DeepSeek official website is https://www.deepseek.com/."
                    )
                    .count(),
                1,
                "regular turn should not receive a duplicated final report excerpt: {prompt}"
            );
            assert!(
                prompt.contains("Research Context Reference"),
                "regular turn should receive compact research reference: {prompt}"
            );
            for hidden in [
                "Researcher notes:",
                "Evidence pack:",
                "<research_brief>",
                "You are a research assistant",
                "Stage: researcher evidence gathering",
                "Stage: delegated deep research worker",
                "Stage: evidence pack compression",
                "checking source",
            ] {
                assert!(
                    !prompt.contains(hidden),
                    "regular turn leaked research-internal context {hidden:?}: {prompt}"
                );
            }
            vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "Ordinary turn saw compact research context.".to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: model_response("Ordinary turn saw compact research context."),
                }),
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }

    fn name(&self) -> &str {
        "scripted-research-provider"
    }
}

struct ClarifyingResearchProvider;

#[async_trait]
impl ModelProviderSDK for ClarifyingResearchProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        let prompt = request_text(&request);
        let text = if prompt_has_stage(&prompt, "clarification gate")
            || prompt.contains("\"need_clarification\"")
        {
            r#"{"need_clarification":true,"question":"Which scope should the research use?","verification":""}"#
        } else {
            "Deep research mock title"
        };
        Ok(model_response(text))
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        let prompt = request_text(&request);
        let text = if prompt_has_stage(&prompt, "clarification gate")
            || prompt.contains("\"need_clarification\"")
        {
            r#"{"need_clarification":true,"question":"Which scope should the research use?","verification":""}"#
        } else {
            "Deep research mock title"
        };
        Ok(Box::pin(stream::iter(streamed_text_events(text))))
    }

    fn name(&self) -> &str {
        "clarifying-research-provider"
    }
}

#[tokio::test]
async fn deep_research_turn_streams_artifact_reasoning_and_final_report() -> Result<()> {
    // Trace: L2-DES-RESEARCH-001
    // Verifies: deep research emits research artifact, reasoning, and final report deltas through normal turn events.
    let workspace = TempDir::new()?;
    write_live_research_config(workspace.path())?;
    let runtime = build_scripted_research_runtime(workspace.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, workspace.path()).await?;
    start_research_turn(&runtime, connection_id, session_id).await?;

    let events = wait_for_research_completion(
        &runtime,
        connection_id,
        session_id,
        &mut notifications_rx,
        "Use the provided scope.",
    )
    .await?;

    assert!(
        events
            .iter()
            .any(|event| child_turn_session_id(event, session_id).is_some()),
        "expected supervisor task to start delegated child work: {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.get("method")
                == Some(&serde_json::json!("item/researchArtifact/delta"))),
        "expected streamed research artifact delta: {events:#?}"
    );
    assert!(
        events.iter().any(is_reasoning_delta),
        "expected reasoning delta: {events:#?}"
    );
    assert!(
        events.iter().any(is_agent_message_delta),
        "expected final report assistant delta: {events:#?}"
    );
    let report_path = assert_final_report_file_written(&events);
    let report_contents =
        std::fs::read_to_string(&report_path).context("read written research report")?;
    assert!(
        report_contents.contains("DeepSeek official website"),
        "expected written report to contain final report content: {report_contents}"
    );
    let final_agent_messages = events
        .iter()
        .filter(|event| {
            event.get("method") == Some(&serde_json::json!("item/completed"))
                && event["params"]["item"]["item_kind"] == serde_json::json!("agent_message")
                && event["params"]["context"]["session_id"]
                    == serde_json::json!(session_id.to_string())
        })
        .count();
    assert_eq!(final_agent_messages, 1);
    let completed_turn = events
        .iter()
        .find(|event| {
            event.get("method") == Some(&serde_json::json!("turn/completed"))
                && event["params"]["session_id"] == serde_json::json!(session_id.to_string())
        })
        .context("missing research turn completion")?;
    assert_eq!(
        completed_turn["params"]["turn"]["usage"],
        serde_json::json!({
            "input_tokens": 5,
            "output_tokens": 5,
            "total_tokens": 10,
            "cache_creation_input_tokens": null,
            "cache_read_input_tokens": null
        })
    );

    Ok(())
}

#[tokio::test]
async fn deep_research_accepts_write_tool_only_final_report() -> Result<()> {
    // Trace: L2-DES-RESEARCH-001
    // Verifies: a final report written only via the write tool is accepted as the report body.
    let workspace = TempDir::new()?;
    write_live_research_config(workspace.path())?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(
        ScriptedResearchProvider::with_write_tool_only_final_report(workspace.path()),
    );
    let runtime = build_scripted_research_runtime_with_provider(workspace.path(), provider)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, workspace.path()).await?;
    start_research_turn(&runtime, connection_id, session_id).await?;

    let events = wait_for_research_completion(
        &runtime,
        connection_id,
        session_id,
        &mut notifications_rx,
        "Use the provided scope.",
    )
    .await?;

    let report_path = assert_final_report_file_written(&events);
    let report_contents =
        std::fs::read_to_string(&report_path).context("read written research report")?;
    assert!(
        report_contents.contains("DeepSeek official website"),
        "expected written report to contain final report content: {report_contents}"
    );
    let final_report = latest_agent_message(&events).context("expected final report message")?;
    assert!(
        final_report.contains("Wrote the full research report"),
        "expected final response to point at written report: {final_report}"
    );
    assert!(
        final_report.contains("DeepSeek official website"),
        "expected final response to summarize written report: {final_report}"
    );

    Ok(())
}

#[tokio::test]
async fn deep_research_read_only_write_permission_uses_active_connection() -> Result<()> {
    // Trace: L2-DES-RESEARCH-001
    // Verifies: research turns can route write-tool approval requests to the active client.
    let workspace = TempDir::new()?;
    write_live_research_config(workspace.path())?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(
        ScriptedResearchProvider::with_write_tool_only_final_report(workspace.path()),
    );
    let runtime = build_scripted_research_runtime_with_provider(workspace.path(), provider)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, workspace.path()).await?;
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 21,
                "method": "_devo/session/permissions/update",
                "params": {
                    "session_id": session_id,
                    "preset": "read-only"
                }
            }),
        )
        .await
        .context("session/permissions/update response")?;
    let response: devo_server::SuccessResponse<devo_server::SessionPermissionsUpdateResult> =
        serde_json::from_value(response)?;
    assert_eq!(
        response.result.preset,
        devo_protocol::PermissionPreset::ReadOnly
    );
    start_research_turn(&runtime, connection_id, session_id).await?;

    let (events, saw_permission_request) = wait_for_research_completion_allowing_permission(
        &runtime,
        connection_id,
        session_id,
        &mut notifications_rx,
        "Use the provided scope.",
    )
    .await?;

    assert!(
        saw_permission_request,
        "expected read-only research report write to request client permission"
    );
    let report_path = assert_final_report_file_written(&events);
    let report_contents =
        std::fs::read_to_string(&report_path).context("read written research report")?;
    assert!(
        report_contents.contains("DeepSeek official website"),
        "expected approved write to produce report content: {report_contents}"
    );

    Ok(())
}

#[tokio::test]
async fn deep_research_streams_researcher_delta_before_query_finishes() -> Result<()> {
    // Trace: L2-DES-RESEARCH-001
    // Verifies: researcher QueryEvent deltas are broadcast while query() is still running.
    let workspace = TempDir::new()?;
    write_live_research_config(workspace.path())?;
    let researcher_gate = Arc::new(Notify::new());
    let provider: Arc<dyn ModelProviderSDK> =
        Arc::new(ScriptedResearchProvider::with_researcher_gate(
            workspace.path(),
            Arc::clone(&researcher_gate),
        ));
    let runtime = build_scripted_research_runtime_with_provider(workspace.path(), provider)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, workspace.path()).await?;
    start_research_turn(&runtime, connection_id, session_id).await?;

    timeout(Duration::from_secs(5), async {
        while let Some(event) = notifications_rx.recv().await {
            let event = legacy_event_from_acp_notification(event);
            let method = event
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if method == "item/researchArtifact/delta"
                && event["params"]["payload"]["delta"]
                    .as_str()
                    .is_some_and(|delta| delta.contains("Researcher notes before completion"))
            {
                return Ok(());
            }
            if method == "turn/failed" {
                anyhow::bail!(
                    "research turn failed before streaming artifact delta: {}",
                    latest_agent_message(&[event])
                        .unwrap_or_else(|| "no failure message was emitted".to_string())
                );
            }
        }
        anyhow::bail!("notification channel closed before artifact delta")
    })
    .await
    .context("timed out waiting for live research artifact delta")??;

    researcher_gate.notify_waiters();
    wait_for_research_completion(
        &runtime,
        connection_id,
        session_id,
        &mut notifications_rx,
        "Use the provided scope.",
    )
    .await?;

    Ok(())
}

#[tokio::test]
async fn deep_research_continues_after_delegated_worker_failure() -> Result<()> {
    // Trace: L2-DES-RESEARCH-001
    // Verifies: a failed delegated worker turn is continued instead of failing the whole research turn.
    let workspace = TempDir::new()?;
    write_live_research_config(workspace.path())?;
    let provider: Arc<dyn ModelProviderSDK> =
        Arc::new(ScriptedResearchProvider::with_delegated_worker_failure_once(workspace.path()));
    let runtime = build_scripted_research_runtime_with_provider(workspace.path(), provider)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, workspace.path()).await?;
    start_research_turn(&runtime, connection_id, session_id).await?;

    let events = wait_for_research_completion(
        &runtime,
        connection_id,
        session_id,
        &mut notifications_rx,
        "Use the provided scope.",
    )
    .await?;

    assert!(
        events.iter().any(|event| {
            event.get("method") == Some(&serde_json::json!("turn/failed"))
                && event["params"]["session_id"] != serde_json::json!(session_id.to_string())
        }),
        "expected child turn failure to be visible before recovery: {events:#?}"
    );
    let completed_turn = events
        .iter()
        .find(|event| {
            event.get("method") == Some(&serde_json::json!("turn/completed"))
                && event["params"]["session_id"] == serde_json::json!(session_id.to_string())
        })
        .context("missing parent research turn completion")?;
    assert_eq!(
        completed_turn["params"]["turn"]["status"],
        serde_json::json!("Completed")
    );
    let final_report = latest_agent_message(&events).context("expected final report message")?;
    assert!(
        final_report.contains("DeepSeek official website"),
        "expected final report after delegated worker recovery: {final_report}"
    );

    Ok(())
}

#[tokio::test]
async fn deep_research_restarts_worker_when_continuation_fails() -> Result<()> {
    // Trace: L2-DES-RESEARCH-001
    // Verifies: if the original delegated worker and its continuation both fail, research restarts a replacement worker.
    let workspace = TempDir::new()?;
    write_live_research_config(workspace.path())?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(
        ScriptedResearchProvider::with_delegated_worker_failures_before_success(
            workspace.path(),
            2,
        ),
    );
    let runtime = build_scripted_research_runtime_with_provider(workspace.path(), provider)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, workspace.path()).await?;
    start_research_turn(&runtime, connection_id, session_id).await?;

    let events = wait_for_research_completion(
        &runtime,
        connection_id,
        session_id,
        &mut notifications_rx,
        "Use the provided scope.",
    )
    .await?;

    let child_failures = events
        .iter()
        .filter(|event| {
            event.get("method") == Some(&serde_json::json!("turn/failed"))
                && event["params"]["session_id"] != serde_json::json!(session_id.to_string())
        })
        .count();
    assert_eq!(child_failures, 2);
    let child_sessions = unique_child_turn_sessions(&events, session_id);
    assert_eq!(child_sessions.len(), 2);
    let final_report = latest_agent_message(&events).context("expected final report message")?;
    assert!(
        final_report.contains("DeepSeek official website"),
        "expected final report after replacement worker recovery: {final_report}"
    );

    Ok(())
}

#[tokio::test]
async fn interrupted_research_closes_delegated_child_agent() -> Result<()> {
    // Trace: L2-DES-RESEARCH-001
    // Verifies: interrupting a parent research turn closes delegated child agents owned by the pipeline.
    let workspace = TempDir::new()?;
    write_live_research_config(workspace.path())?;
    let researcher_gate = Arc::new(Notify::new());
    let provider: Arc<dyn ModelProviderSDK> =
        Arc::new(ScriptedResearchProvider::with_researcher_gate(
            workspace.path(),
            Arc::clone(&researcher_gate),
        ));
    let runtime = build_scripted_research_runtime_with_provider(workspace.path(), provider)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, workspace.path()).await?;
    let turn_id = start_research_turn(&runtime, connection_id, session_id).await?;

    let child_session_id = timeout(Duration::from_secs(5), async {
        let mut child_session_id = None;
        let mut saw_researcher_delta = false;
        while let Some(event) = notifications_rx.recv().await {
            let event = legacy_event_from_acp_notification(event);
            let method = event
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if let Some(session_id) = child_turn_session_id(&event, session_id) {
                child_session_id = Some(session_id);
            }
            if method == "item/researchArtifact/delta"
                && event["params"]["payload"]["delta"]
                    .as_str()
                    .is_some_and(|delta| delta.contains("Researcher notes before completion"))
            {
                saw_researcher_delta = true;
            }
            if saw_researcher_delta && let Some(child_session_id) = child_session_id.clone() {
                return Ok(child_session_id);
            }
            if method == "turn/failed" {
                anyhow::bail!(
                    "research turn failed before child was running: {}",
                    latest_agent_message(&[event])
                        .unwrap_or_else(|| "no failure message was emitted".to_string())
                );
            }
        }
        anyhow::bail!("notification channel closed before delegated child started")
    })
    .await
    .context("timed out waiting for delegated child to start")??;

    let interrupt_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 33,
                "method": "_devo/turn/interrupt",
                "params": {
                    "session_id": session_id,
                    "turn_id": turn_id,
                    "reason": "test interrupt"
                }
            }),
        )
        .await
        .context("turn/interrupt response")?;
    let interrupt_response: devo_server::SuccessResponse<devo_server::TurnInterruptResult> =
        serde_json::from_value(interrupt_response)?;
    assert_eq!(
        interrupt_response.result.status,
        devo_core::TurnStatus::Interrupted
    );

    let list_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 34,
                "method": "_devo/agent/list",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("agent/list response")?;
    let list_response: devo_server::SuccessResponse<devo_protocol::AgentListResult> =
        serde_json::from_value(list_response)?;
    let child = list_response
        .result
        .agents
        .iter()
        .find(|agent| agent.session_id.to_string() == child_session_id)
        .with_context(|| format!("missing delegated child {child_session_id}"))?;
    assert_eq!(child.status, "closed");

    Ok(())
}

#[tokio::test]
async fn queued_regular_turn_starts_after_research_completes() -> Result<()> {
    // Trace: L2-DES-RESEARCH-001
    // Verifies: a normal turn queued during research is drained after the research turn finishes.
    let workspace = TempDir::new()?;
    write_live_research_config(workspace.path())?;
    let researcher_gate = Arc::new(Notify::new());
    let provider: Arc<dyn ModelProviderSDK> =
        Arc::new(ScriptedResearchProvider::with_researcher_gate(
            workspace.path(),
            Arc::clone(&researcher_gate),
        ));
    let runtime = build_scripted_research_runtime_with_provider(workspace.path(), provider)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, workspace.path()).await?;
    start_research_turn(&runtime, connection_id, session_id).await?;

    timeout(Duration::from_secs(5), async {
        while let Some(event) = notifications_rx.recv().await {
            let event = legacy_event_from_acp_notification(event);
            if event.get("method") == Some(&serde_json::json!("item/researchArtifact/delta"))
                && event["params"]["payload"]["delta"]
                    .as_str()
                    .is_some_and(|delta| delta.contains("Researcher notes before completion"))
            {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before researcher delta")
    })
    .await
    .context("timed out waiting for live research artifact delta")??;

    queue_regular_turn_during_research(&runtime, connection_id, session_id).await?;
    researcher_gate.notify_waiters();
    let events = wait_for_completed_turns(&mut notifications_rx, session_id, 2).await?;

    assert!(
        events.iter().any(|event| {
            event.get("method") == Some(&serde_json::json!("turn/started"))
                && event["params"]["turn"]["kind"] == serde_json::json!("regular")
        }),
        "expected queued regular turn to start after research completion: {events:#?}"
    );

    Ok(())
}

#[tokio::test]
async fn interrupted_research_clears_pending_clarification_request() -> Result<()> {
    // Trace: L2-DES-RESEARCH-001
    // Verifies: interrupting a research clarification clears the pending request_user_input entry.
    let workspace = TempDir::new()?;
    write_live_research_config(workspace.path())?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ClarifyingResearchProvider);
    let runtime = build_scripted_research_runtime_with_provider(workspace.path(), provider)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, workspace.path()).await?;
    let turn_id = start_research_turn(&runtime, connection_id, session_id).await?;
    let clarification_event = wait_for_clarification_request(&mut notifications_rx).await?;

    let interrupt_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 32,
                "method": "_devo/turn/interrupt",
                "params": {
                    "session_id": session_id,
                    "turn_id": turn_id.clone(),
                    "reason": "test interrupt"
                }
            }),
        )
        .await
        .context("turn/interrupt response")?;
    let interrupt_response: devo_server::SuccessResponse<devo_server::TurnInterruptResult> =
        serde_json::from_value(interrupt_response)?;
    assert_eq!(interrupt_response.result.turn_id.to_string(), turn_id);

    let stale_response = respond_to_clarification_raw(
        &runtime,
        connection_id,
        &clarification_event,
        "Official site",
    )
    .await?;
    let stale_error: devo_server::ErrorResponse = serde_json::from_value(stale_response)?;
    assert_eq!(
        stale_error.error.code,
        devo_server::ProtocolErrorCode::InvalidParams
    );
    assert_eq!(
        stale_error.error.message,
        "no pending request_user_input request exists for this runtime"
    );

    Ok(())
}

#[tokio::test]
async fn regular_turn_after_research_receives_only_compact_handoff() -> Result<()> {
    // Trace: L2-DES-RESEARCH-001
    // Verifies: follow-up coding turns receive the research final report and compact reference, not internal artifacts.
    let workspace = TempDir::new()?;
    write_live_research_config(workspace.path())?;
    let runtime = build_scripted_research_runtime(workspace.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, workspace.path()).await?;
    start_research_turn(&runtime, connection_id, session_id).await?;
    wait_for_research_completion(
        &runtime,
        connection_id,
        session_id,
        &mut notifications_rx,
        "Use the provided scope.",
    )
    .await?;

    start_regular_turn_after_research(&runtime, connection_id, session_id).await?;
    wait_for_research_completion(
        &runtime,
        connection_id,
        session_id,
        &mut notifications_rx,
        "Use the provided scope.",
    )
    .await?;

    Ok(())
}

#[tokio::test]
async fn resumed_regular_turn_after_research_receives_only_compact_handoff() -> Result<()> {
    // Trace: L2-DES-RESEARCH-001
    // Verifies: rollout replay uses the same research context projection as the live session.
    let workspace = TempDir::new()?;
    write_live_research_config(workspace.path())?;
    let runtime = build_scripted_research_runtime(workspace.path())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, workspace.path()).await?;
    start_research_turn(&runtime, connection_id, session_id).await?;
    wait_for_research_completion(
        &runtime,
        connection_id,
        session_id,
        &mut notifications_rx,
        "Use the provided scope.",
    )
    .await?;

    let rebuilt_runtime = build_scripted_research_runtime(workspace.path())?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, mut rebuilt_notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    resume_session(&rebuilt_runtime, rebuilt_connection_id, session_id).await?;
    start_regular_turn_after_research(&rebuilt_runtime, rebuilt_connection_id, session_id).await?;
    wait_for_research_completion(
        &rebuilt_runtime,
        rebuilt_connection_id,
        session_id,
        &mut rebuilt_notifications_rx,
        "Use the provided scope.",
    )
    .await?;

    Ok(())
}

#[tokio::test]
#[ignore = "requires DEEPSEEK_API_KEY and live DeepSeek Anthropic Messages hosted web search access"]
async fn deep_research_turn_live_with_deepseek_anthropic_messages() -> Result<()> {
    // Trace: L2-DES-RESEARCH-001
    // Verifies: deep research runs via turn/start with DeepSeek Anthropic Messages, provider-hosted web search, and local web fetch.
    let Some(api_key) = deepseek_api_key() else {
        eprintln!("skipping live deep research e2e test: DEEPSEEK_API_KEY is not set or is empty");
        return Ok(());
    };

    let workspace = TempDir::new()?;
    write_live_research_config(workspace.path())?;
    let runtime = build_live_deepseek_runtime(workspace.path(), api_key)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, workspace.path()).await?;
    let turn_id = start_research_turn(&runtime, connection_id, session_id).await?;

    let events = wait_for_research_completion(
        &runtime,
        connection_id,
        session_id,
        &mut notifications_rx,
        "Use the provided scope. Research the current official DeepSeek website domain and include one source URL.",
    )
    .await?;

    assert_turn_started_as_research(&events);
    assert_research_artifacts(&events);
    assert_normal_web_search_items(&events);
    assert_final_report(&events);
    assert_eq!(
        events
            .iter()
            .find(|event| event.get("method") == Some(&serde_json::json!("turn/completed")))
            .and_then(|event| event["params"]["turn"]["turn_id"].as_str()),
        Some(turn_id.as_str())
    );

    Ok(())
}

fn deepseek_api_key() -> Option<String> {
    std::env::var("DEEPSEEK_API_KEY")
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

fn write_live_research_config(root: &std::path::Path) -> Result<()> {
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

fn build_live_deepseek_runtime(
    data_root: &std::path::Path,
    api_key: String,
) -> Result<Arc<ServerRuntime>> {
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(
        AnthropicProvider::new("https://api.deepseek.com/anthropic").with_api_key(api_key),
    );
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("deep_research_e2e.db"),
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

fn build_scripted_research_runtime(data_root: &std::path::Path) -> Result<Arc<ServerRuntime>> {
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedResearchProvider::new(data_root));
    build_scripted_research_runtime_with_provider(data_root, provider)
}

fn build_scripted_research_runtime_with_provider(
    data_root: &std::path::Path,
    provider: Arc<dyn ModelProviderSDK>,
) -> Result<Arc<ServerRuntime>> {
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("scripted_research_e2e.db"),
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
        base_instructions: "Follow the developer instructions. Keep all live test outputs concise."
            .to_string(),
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

fn is_research_request(request: &ModelRequest) -> bool {
    request
        .system
        .as_deref()
        .is_some_and(|system| system.contains("You are Devo `/research`"))
}

fn assert_research_environment_contains_cwd(request: &ModelRequest, expected_cwd: &str) {
    let expected = format!("<cwd>{expected_cwd}</cwd>");
    let texts = request
        .messages
        .iter()
        .map(request_message_text)
        .collect::<Vec<_>>();
    assert!(
        texts.iter().any(|text| {
            text.starts_with("<research_environment>") && text.contains(expected.as_str())
        }),
        "research request should include cwd in research environment: expected {expected:?}, texts: {texts:?}"
    );
}

fn assert_compress_request_uses_structured_context(request: &ModelRequest) {
    assert!(
        request.hosted_tools.is_empty(),
        "compression must not expose new provider-hosted tools"
    );
    let runtime_text = request
        .messages
        .iter()
        .map(request_message_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !runtime_text.contains("<visible_tool_transcript>"),
        "structured or empty tool evidence must not be flattened into visible tool transcript: {runtime_text}"
    );

    let hosted_blocks = request
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|content| match content {
            RequestContent::HostedToolUse {
                id,
                name,
                input,
                output,
                status,
            } => Some((id, name, input, output, status)),
            RequestContent::Text { .. }
            | RequestContent::Reasoning { .. }
            | RequestContent::ProviderReasoning { .. }
            | RequestContent::ToolUse { .. }
            | RequestContent::ToolResult { .. } => None,
        })
        .collect::<Vec<_>>();

    if hosted_blocks.is_empty() {
        assert!(
            runtime_text.contains("Researcher notes before completion"),
            "default research compression should carry hosted web evidence: {runtime_text}"
        );
        return;
    }

    assert_eq!(
        hosted_blocks.len(),
        2,
        "expected hosted start/result blocks"
    );
    let (start_id, start_name, start_input, start_output, start_status) = hosted_blocks[0];
    assert_eq!(start_id.as_str(), "hosted_ws_1");
    assert_eq!(start_name.as_str(), "web_search");
    assert_eq!(
        start_input,
        &serde_json::json!({ "query": "DeepSeek official website" })
    );
    assert!(start_output.is_none());
    assert!(start_status.is_none());

    let (done_id, done_name, done_input, done_output, done_status) = hosted_blocks[1];
    assert_eq!(done_id.as_str(), "hosted_ws_1");
    assert_eq!(done_name.as_str(), "web_search");
    assert_eq!(
        done_input,
        &serde_json::json!({ "query": "DeepSeek official website" })
    );
    assert_eq!(done_status.as_deref(), Some("completed"));
    assert_eq!(
        done_output.as_ref(),
        Some(&serde_json::json!([{
            "title": "DeepSeek",
            "url": "https://www.deepseek.com/"
        }]))
    );
}

fn hosted_web_search_researcher_response() -> ModelResponse {
    ModelResponse {
        id: "hosted-search-researcher-response".to_string(),
        content: vec![
            ResponseContent::Text(
                "Researcher notes: official source https://www.deepseek.com/".to_string(),
            ),
            ResponseContent::HostedToolUse {
                id: "hosted_ws_1".to_string(),
                name: "web_search".to_string(),
                input: serde_json::json!({ "query": "DeepSeek official website" }),
                output: None,
                status: None,
            },
            ResponseContent::HostedToolUse {
                id: "hosted_ws_1".to_string(),
                name: "web_search".to_string(),
                input: serde_json::json!({}),
                output: Some(serde_json::json!([{
                    "title": "DeepSeek",
                    "url": "https://www.deepseek.com/"
                }])),
                status: Some("completed".to_string()),
            },
        ],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage::default(),
        metadata: ResponseMetadata::default(),
    }
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
                        "name": "deep-research-e2e-test",
                        "title": "deep-research-e2e-test",
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

async fn resume_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_core::SessionId,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 20,
                "method": "_devo/session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response")?;
    let response: devo_server::SuccessResponse<devo_server::SessionResumeResult> =
        serde_json::from_value(response)?;
    assert_eq!(response.result.session.session_id, session_id);
    Ok(())
}

async fn start_research_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_core::SessionId,
) -> Result<String> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{
                        "type": "text",
                        "text": "Research the current official DeepSeek website domain. Use web search, keep the final report short, and include source URLs."
                    }],
                    "model": "deepseek-v4-flash",
                    "model_binding_id": null,
                    "reasoning_effort_selection": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null,
                    "collaboration_mode": "build",
                    "execution_mode": "research"
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let response: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(response)?;
    Ok(response
        .result
        .turn_id()
        .expect("research turn should have started")
        .to_string())
}

async fn start_regular_turn_after_research(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_core::SessionId,
) -> Result<String> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 30,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{
                        "type": "text",
                        "text": "Now answer as a normal coding turn using the research context."
                    }],
                    "model": "deepseek-v4-flash",
                    "model_binding_id": null,
                    "reasoning_effort_selection": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null,
                    "collaboration_mode": "build",
                    "execution_mode": "regular"
                }
            }),
        )
        .await
        .context("regular turn/start after research response")?;
    let response: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(response)?;
    Ok(response
        .result
        .turn_id()
        .expect("regular turn should have started")
        .to_string())
}

async fn queue_regular_turn_during_research(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_core::SessionId,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 31,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{
                        "type": "text",
                        "text": "Now answer as a normal coding turn using the research context."
                    }],
                    "model": "deepseek-v4-flash",
                    "model_binding_id": null,
                    "reasoning_effort_selection": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null,
                    "collaboration_mode": "build",
                    "execution_mode": "regular"
                }
            }),
        )
        .await;
    let response = response.context("queued turn/start response")?;
    let response: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(response)?;
    assert!(
        matches!(response.result, devo_server::TurnStartResult::Queued { .. }),
        "regular turn should be queued while research is active"
    );
    Ok(())
}

async fn wait_for_research_completion(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_core::SessionId,
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    clarification_answer: &str,
) -> Result<Vec<serde_json::Value>> {
    let mut events = Vec::new();
    timeout(Duration::from_secs(240), async {
        while let Some(event) = notifications_rx.recv().await {
            let event = legacy_event_from_acp_notification(event);
            let method = event
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if method == "item/tool/requestUserInput" {
                respond_to_clarification(runtime, connection_id, &event, clarification_answer)
                    .await?;
            }
            let is_parent_event =
                event["params"]["session_id"] == serde_json::json!(session_id.to_string());
            let done = method == "turn/completed" && is_parent_event;
            if method == "turn/failed" && is_parent_event {
                anyhow::bail!(
                    "research turn failed: {}",
                    latest_agent_message(&events)
                        .unwrap_or_else(|| "no failure message was emitted".to_string())
                );
            }
            events.push(event);
            if done {
                return Ok(events);
            }
        }
        anyhow::bail!("notification channel closed before turn/completed")
    })
    .await
    .context("timed out waiting for research completion")?
}

async fn wait_for_research_completion_allowing_permission(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_core::SessionId,
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    clarification_answer: &str,
) -> Result<(Vec<serde_json::Value>, bool)> {
    let mut events = Vec::new();
    timeout(Duration::from_secs(240), async {
        let mut saw_permission_request = false;
        while let Some(raw_event) = notifications_rx.recv().await {
            if raw_event.get("method") == Some(&serde_json::json!("session/request_permission")) {
                saw_permission_request = true;
                assert_eq!(
                    raw_event["params"]["sessionId"],
                    serde_json::json!(session_id)
                );
                let response = runtime
                    .handle_incoming(
                        connection_id,
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": raw_event["id"].clone(),
                            "result": {
                                "outcome": {
                                    "outcome": "selected",
                                    "optionId": "allow_once"
                                }
                            }
                        }),
                    )
                    .await;
                assert_eq!(response, None);
                events.push(raw_event);
                continue;
            }
            let event = legacy_event_from_acp_notification(raw_event);
            let method = event
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if method == "item/tool/requestUserInput" {
                respond_to_clarification(runtime, connection_id, &event, clarification_answer)
                    .await?;
            }
            let is_parent_event =
                event["params"]["session_id"] == serde_json::json!(session_id.to_string());
            let done = method == "turn/completed" && is_parent_event;
            if method == "turn/failed" && is_parent_event {
                anyhow::bail!(
                    "research turn failed: {}",
                    latest_agent_message(&events)
                        .unwrap_or_else(|| "no failure message was emitted".to_string())
                );
            }
            events.push(event);
            if done {
                return Ok((events, saw_permission_request));
            }
        }
        anyhow::bail!("notification channel closed before turn/completed")
    })
    .await
    .context("timed out waiting for research completion")?
}

async fn wait_for_completed_turns(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    session_id: devo_core::SessionId,
    expected_completed: usize,
) -> Result<Vec<serde_json::Value>> {
    let mut events = Vec::new();
    timeout(Duration::from_secs(240), async {
        let mut completed = 0usize;
        while let Some(event) = notifications_rx.recv().await {
            let event = legacy_event_from_acp_notification(event);
            let method = event
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let is_parent_event =
                event["params"]["session_id"] == serde_json::json!(session_id.to_string());
            if method == "turn/failed" && is_parent_event {
                anyhow::bail!(
                    "turn failed while waiting for queued completion: {}",
                    latest_agent_message(&events)
                        .unwrap_or_else(|| "no failure message was emitted".to_string())
                );
            }
            if method == "turn/completed" && is_parent_event {
                completed += 1;
            }
            events.push(event);
            if completed >= expected_completed {
                return Ok(events);
            }
        }
        anyhow::bail!("notification channel closed before expected turn completions")
    })
    .await
    .context("timed out waiting for turn completions")?
}

async fn wait_for_clarification_request(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<serde_json::Value> {
    timeout(Duration::from_secs(1), async {
        while let Some(event) = notifications_rx.recv().await {
            let event = legacy_event_from_acp_notification(event);
            let method = event
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if method == "item/tool/requestUserInput" {
                return Ok(event);
            }
            if method == "turn/failed" {
                anyhow::bail!(
                    "research turn failed before clarification request: {}",
                    latest_agent_message(&[event])
                        .unwrap_or_else(|| "no failure message was emitted".to_string())
                );
            }
        }
        anyhow::bail!("notification channel closed before clarification request")
    })
    .await
    .context("timed out waiting for clarification request")?
}

fn is_reasoning_delta(event: &serde_json::Value) -> bool {
    event.get("method") == Some(&serde_json::json!("item/reasoning/textDelta"))
        || (event.get("method") == Some(&serde_json::json!("session/update"))
            && event["params"]["update"]["sessionUpdate"]
                == serde_json::json!("agent_thought_chunk"))
}

fn is_agent_message_delta(event: &serde_json::Value) -> bool {
    event.get("method") == Some(&serde_json::json!("item/agentMessage/delta"))
        || (event.get("method") == Some(&serde_json::json!("session/update"))
            && event["params"]["update"]["sessionUpdate"]
                == serde_json::json!("agent_message_chunk"))
}
fn child_turn_session_id(
    event: &serde_json::Value,
    parent_session_id: devo_core::SessionId,
) -> Option<String> {
    if event.get("method") != Some(&serde_json::json!("turn/started")) {
        return None;
    }
    let session_id = event["params"]["session_id"].as_str()?;
    (session_id != parent_session_id.to_string()).then(|| session_id.to_string())
}

fn unique_child_turn_sessions(
    events: &[serde_json::Value],
    parent_session_id: devo_core::SessionId,
) -> Vec<String> {
    let mut sessions = Vec::new();
    for event in events {
        let Some(session_id) = child_turn_session_id(event, parent_session_id) else {
            continue;
        };
        if !sessions.contains(&session_id) {
            sessions.push(session_id);
        }
    }
    sessions
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

fn latest_agent_message(events: &[serde_json::Value]) -> Option<String> {
    events.iter().rev().find_map(|event| {
        if event.get("method") != Some(&serde_json::json!("item/completed")) {
            return None;
        }
        let item = &event["params"]["item"];
        if item["item_kind"] == serde_json::json!("agent_message") {
            return item["payload"]["text"].as_str().map(str::to_string);
        }
        if item["item_kind"] == serde_json::json!("research_artifact")
            && item["payload"]["artifact_type"] == serde_json::json!("failure")
        {
            return item["payload"]["content"].as_str().map(str::to_string);
        }
        None
    })
}

async fn respond_to_clarification(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    event: &serde_json::Value,
    answer: &str,
) -> Result<()> {
    let response = respond_to_clarification_raw(runtime, connection_id, event, answer).await?;
    let _: devo_server::SuccessResponse<serde_json::Value> = serde_json::from_value(response)?;
    Ok(())
}

async fn respond_to_clarification_raw(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    event: &serde_json::Value,
    answer: &str,
) -> Result<serde_json::Value> {
    let params = &event["params"];
    let request = &params["request"];
    let turn_id = request["turn_id"]
        .as_str()
        .context("clarification request missing turn_id")?;
    let request_id = request["request_id"]
        .as_str()
        .context("clarification request missing request_id")?;
    let session_id = request["session_id"]
        .as_str()
        .context("clarification request missing session_id")?;
    let question_id = params["questions"]
        .as_array()
        .and_then(|questions| questions.first())
        .and_then(|question| question["id"].as_str())
        .context("clarification request missing question id")?;
    let mut answers = serde_json::Map::new();
    answers.insert(
        question_id.to_string(),
        serde_json::json!({
            "answers": [answer]
        }),
    );

    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 4,
                "method": "_devo/request_user_input/respond",
                "params": {
                    "session_id": session_id,
                    "turn_id": turn_id,
                    "request_id": request_id,
                    "response": {
                        "answers": serde_json::Value::Object(answers)
                    }
                }
            }),
        )
        .await
        .context("request_user_input/respond response")?;
    Ok(response)
}

fn assert_turn_started_as_research(events: &[serde_json::Value]) {
    assert!(
        events.iter().any(|event| {
            event.get("method") == Some(&serde_json::json!("turn/started"))
                && event["params"]["turn"]["kind"] == serde_json::json!("research")
        }),
        "expected turn/started for research turn: {events:#?}"
    );
}

fn assert_research_artifacts(events: &[serde_json::Value]) {
    let artifact_types = events
        .iter()
        .filter(|event| {
            event.get("method") == Some(&serde_json::json!("item/completed"))
                && event["params"]["item"]["item_kind"] == serde_json::json!("research_artifact")
        })
        .filter_map(|event| event["params"]["item"]["payload"]["artifact_type"].as_str())
        .collect::<Vec<_>>();

    assert!(
        artifact_types.contains(&"brief"),
        "expected brief artifact: {events:#?}"
    );
    assert!(
        artifact_types.contains(&"plan"),
        "expected plan artifact: {events:#?}"
    );
    assert!(
        artifact_types.contains(&"compressed_finding"),
        "expected compressed finding artifact: {events:#?}"
    );
}

fn assert_normal_web_search_items(events: &[serde_json::Value]) {
    let web_tool_call_ids = events
        .iter()
        .filter(|event| {
            event.get("method") == Some(&serde_json::json!("item/completed"))
                && event["params"]["item"]["item_kind"] == serde_json::json!("tool_call")
        })
        .filter_map(|event| {
            let payload = &event["params"]["item"]["payload"];
            let tool_name = payload["tool_name"].as_str()?;
            (tool_name == "web_search")
                .then(|| payload["tool_call_id"].as_str().map(str::to_string))
                .flatten()
        })
        .collect::<Vec<_>>();

    assert!(
        !web_tool_call_ids.is_empty(),
        "expected provider-hosted web_search to emit normal tool_call items: {events:#?}"
    );

    assert!(
        web_tool_call_ids.iter().any(|tool_call_id| {
            events.iter().any(|event| {
                event.get("method") == Some(&serde_json::json!("item/completed"))
                    && event["params"]["item"]["item_kind"] == serde_json::json!("tool_result")
                    && event["params"]["item"]["payload"]["tool_call_id"]
                        == serde_json::json!(tool_call_id)
            })
        }),
        "expected provider-hosted web_search to emit normal tool_result items: {events:#?}"
    );
}

fn assert_final_report(events: &[serde_json::Value]) {
    let final_report = events.iter().rev().find_map(|event| {
        (event.get("method") == Some(&serde_json::json!("item/completed"))
            && event["params"]["item"]["item_kind"] == serde_json::json!("agent_message"))
        .then(|| event["params"]["item"]["payload"]["text"].as_str())
        .flatten()
    });
    let final_report = final_report.expect("expected final report agent message");
    assert!(
        final_report.len() > 40,
        "expected non-trivial final report: {final_report}"
    );
    assert!(
        final_report.to_ascii_lowercase().contains("deepseek"),
        "expected final report to address DeepSeek: {final_report}"
    );
}

fn assert_final_report_file_written(events: &[serde_json::Value]) -> std::path::PathBuf {
    let path = events.iter().find_map(final_report_written_path);
    let path = path.unwrap_or_else(|| {
        panic!("expected final report write tool result: {events:#?}");
    });
    std::path::PathBuf::from(path)
}

fn final_report_written_path(event: &serde_json::Value) -> Option<&str> {
    if event.get("method") == Some(&serde_json::json!("item/completed"))
        && event["params"]["item"]["item_kind"] == serde_json::json!("tool_result")
        && event["params"]["item"]["payload"]["tool_name"] == serde_json::json!("write")
    {
        return event["params"]["item"]["payload"]["content"]["files"]
            .as_array()
            .and_then(|files| files.first())
            .and_then(|file| file["path"].as_str());
    }
    if event.get("method") == Some(&serde_json::json!("session/update"))
        && event["params"]["update"]["sessionUpdate"] == serde_json::json!("tool_call_update")
        && event["params"]["update"]["status"] == serde_json::json!("completed")
        && event["params"]["update"]["kind"] == serde_json::json!("edit")
    {
        return event["params"]["update"]["rawOutput"]["files"]
            .as_array()
            .and_then(|files| files.first())
            .and_then(|file| file["path"].as_str());
    }
    None
}
