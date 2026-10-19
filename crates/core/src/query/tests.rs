//! Unit tests for the query loop and its submodules.

use devo_protocol::Usage;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crate::EventCallback;
use crate::ModelQueryRetryPhase;
use crate::ProviderRetryStatus;
use crate::tools::ToolAgentScope;
use crate::tools::ToolContent;
use crate::tools::ToolPreparationFeedback;
use crate::tools::ToolRegistry;
use crate::tools::ToolRuntime;
use crate::tools::ToolRuntimeContext;
use crate::tools::handlers::IpythonHandler;
use crate::tools::json_schema::JsonSchema;
use crate::tools::registry::ToolExposure;
use crate::tools::registry::ToolRegistryBuilder;
use crate::tools::router::PermissionChecker;
use crate::tools::router::ToolExecutionOptions;
use crate::tools::tool_handler::ToolHandler;
use crate::tools::tool_spec::ToolExecutionMode;
use crate::tools::tool_spec::ToolOutputMode;
use crate::tools::tool_spec::ToolSpec;
use anyhow::Result;
use async_trait::async_trait;
use devo_protocol::CollaborationMode;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseExtra;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::ThreadGoal;
use devo_protocol::ThreadGoalStatus;
use devo_provider::ModelProviderSDK;
use devo_safety::PermissionMode;
use futures::Stream;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::QueryEvent;
use super::QueryOptions;
use super::SharedLastModelRequest;
use super::hosted_tools_for_web_search;
use super::insert_subagent_request_reminders;
use super::query;
use super::test_model_connection;
use super::truncate_tool_result_for_model;
use crate::AgentError;
use crate::ContentBlock;
use crate::Message;
use crate::Model;
use crate::ReasoningCapability;
use crate::ReasoningEffort;
use crate::ReasoningImplementation;
use crate::ReasoningVariant;
use crate::ReasoningVariantConfig;
use crate::Role;
use crate::SessionConfig;
use crate::SessionState;
use crate::TruncationMode;
use crate::TruncationPolicyConfig;
use crate::TurnConfig;
use crate::context::ContextualUserFragment;
use crate::context::compaction_summary::CompactionSummary;
use crate::history::compaction::CompactionKind;
use crate::response_item::ResponseItem;

#[test]
fn assistant_content_visibility() {
    assert!(!super::assistant_content_has_visible_content(&[]));
    assert!(!super::assistant_content_has_visible_content(&[
        ContentBlock::Text {
            text: " \n\t".to_string(),
        },
    ]));
    assert!(!super::assistant_content_has_visible_content(&[
        ContentBlock::ToolResult {
            tool_use_id: "call-1".to_string(),
            content: String::new(),
            is_error: false,
        },
    ]));

    for content in [
        vec![ContentBlock::Text {
            text: "visible".to_string(),
        }],
        vec![ContentBlock::ToolUse {
            id: "call-1".to_string(),
            name: "ipython".to_string(),
            input: json!({"code":"print(1)"}),
        }],
    ] {
        assert!(super::assistant_content_has_visible_content(&content));
    }
}

#[test]
fn hosted_tool_schemas_follow_provider_capabilities() {
    assert!(matches!(
        hosted_tools_for_web_search(&devo_config::ResolvedWebSearchConfig::Provider).as_slice(),
        [devo_protocol::HostedToolDefinition::WebSearch(_)]
    ));
    assert!(
        hosted_tools_for_web_search(&devo_config::ResolvedWebSearchConfig::Disabled).is_empty()
    );
    assert!(
        super::hosted_tools_for_web_capabilities(
            &devo_config::ResolvedWebSearchConfig::Provider,
            devo_config::ResolvedWebFetchConfig::Disabled,
            devo_protocol::ProviderWireApi::OpenAIChatCompletions,
        )
        .is_empty()
    );
    assert!(matches!(
        super::hosted_tools_for_web_capabilities(
            &devo_config::ResolvedWebSearchConfig::Provider,
            devo_config::ResolvedWebFetchConfig::Disabled,
            devo_protocol::ProviderWireApi::OpenAIResponses,
        )
        .as_slice(),
        [devo_protocol::HostedToolDefinition::WebSearch(_)]
    ));
}

#[test]
fn error_classification_and_retry_policy() {
    let network_cases = [
        anyhow::anyhow!("request timed out while connecting"),
        anyhow::anyhow!(
            "error sending request for url (https://api.example.test): connection refused"
        ),
        anyhow::anyhow!("dns error: failed to lookup address information"),
        anyhow::anyhow!("network is unreachable"),
        anyhow::anyhow!(
            "anthropic stream error for model deepseek-v4-flash: invalid header value: \"text/html; charset=utf-8\"; debug=InvalidContentType(\"text/html; charset=utf-8\")"
        ),
        anyhow::anyhow!("Invalid status code: 408 Request Timeout"),
        anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "socket timed out",
        )),
        anyhow::Error::new(devo_provider::error::ProviderError::ProviderTimeoutError {
            message: "provider request timed out".into(),
            provider_name: Some("test-provider".into()),
        }),
    ];

    for error in network_cases {
        assert_eq!(
            super::classify_error(&error),
            super::ErrorClass::NetworkError
        );
        let mut retry_count = 0;
        let mut context_compacted = false;
        assert!(matches!(
            super::provider_retry_decision(&error, &mut retry_count, &mut context_compacted),
            super::ProviderRetryDecision::RetryAfter(_)
        ));
        assert_eq!(retry_count, 1);
        assert!(!context_compacted);
    }

    let stream_400 = anyhow::anyhow!(
        "openai-responses stream error for model deepseek-v4-flash: Invalid status code: 400 Bad Request; response body: {{\"error\":{{\"message\":\"Failed to deserialize the JSON body into the target type: input: unknown variant `reasoning`\",\"type\":\"invalid_request_error\",\"code\":\"invalid_request_error\"}}}}"
    );
    assert_eq!(
        super::classify_error(&stream_400),
        super::ErrorClass::ParameterError,
        "400 Bad Request inside a stream error wrapper must not retry"
    );
    let mut retry_count = 0;
    let mut context_compacted = false;
    assert!(matches!(
        super::provider_retry_decision(&stream_400, &mut retry_count, &mut context_compacted),
        super::ProviderRetryDecision::Fail
    ));
    assert_eq!(retry_count, 0);

    let auth_error = anyhow::anyhow!("token timeout");
    assert_eq!(
        super::classify_error(&auth_error),
        super::ErrorClass::AuthenticationFailure
    );
    let mut retry_count = 0;
    let mut context_compacted = false;
    assert!(matches!(
        super::provider_retry_decision(&auth_error, &mut retry_count, &mut context_compacted),
        super::ProviderRetryDecision::Fail
    ));
    assert_eq!(retry_count, 0);
    assert!(!context_compacted);
}

#[test]
fn model_tool_result_truncation_policies() {
    let long = "abcdefghijklmnopqrstuvwxyz".to_string();
    for (content, tool_name, policy, expected) in [
        (
            "short",
            Some("read"),
            TruncationPolicyConfig::bytes(100),
            "short",
        ),
        (
            long.as_str(),
            Some("read"),
            TruncationPolicyConfig::bytes(20),
            "abcde\n...[truncated]",
        ),
        (
            long.as_str(),
            Some("read"),
            TruncationPolicyConfig::tokens(5),
            "abcde\n...[truncated]",
        ),
    ] {
        assert_eq!(
            truncate_tool_result_for_model(content.to_string(), tool_name, policy.into()),
            expected
        );
    }

    let truncated = truncate_tool_result_for_model(
        "éééééabcdefghij".to_string(),
        Some("read"),
        TruncationPolicyConfig::bytes(18).into(),
    );
    assert_eq!(truncated, "é\n...[truncated]");
    assert!(truncated.len() <= 18);

    for tool_name in [
        Some("await_task"),
        Some("wait_agent"),
        Some("subagent_result"),
    ] {
        assert_eq!(
            truncate_tool_result_for_model(
                long.clone(),
                tool_name,
                TruncationPolicyConfig::bytes(20).into(),
            ),
            long
        );
    }
}

#[test]
fn model_visible_mixed_tool_content_serializes_for_model() {
    use super::serialize_tool_content_for_model;

    let stream = "hello\nworld".to_string();
    let shell_json = json!({
        "command": "echo hello",
        "exit": 0,
        "cwd": "/tmp",
        "description": "say hello",
    });
    let shell_content = ToolContent::Mixed {
        text: Some(stream.clone()),
        json: Some(shell_json.clone()),
    };
    let shell_model =
        serialize_tool_content_for_model(shell_content.clone(), Some("shell_command"));
    // Shell metadata (exit/cwd/duration) reaches the model — it holds no copy
    // of the stream, and timing is how the model notices slow or hung runs.
    let expected_shell_model = format!("{stream}\n{shell_json}");
    assert_eq!(shell_model, expected_shell_model);
    assert_eq!(
        truncate_tool_result_for_model(
            shell_model.clone(),
            Some("shell_command"),
            TruncationPolicyConfig::bytes(10_000).into(),
        ),
        expected_shell_model
    );
    assert_eq!(shell_model.matches("hello\nworld").count(), 1);
    assert!(shell_model.contains("\"exit\""));
    assert_eq!(
        serialize_tool_content_for_model(shell_content, Some("bash")),
        shell_model
    );

    let webfetch_content = ToolContent::Mixed {
        text: Some("Image fetched successfully".into()),
        json: Some(json!({
            "title": "https://example.com/a.png (image/png)",
            "mime": "image/png",
            "image_base64": "abc123",
        })),
    };
    let webfetch_model = serialize_tool_content_for_model(webfetch_content, Some("webfetch"));
    assert!(webfetch_model.contains("Image fetched successfully"));
    assert!(webfetch_model.contains("image_base64"));
    assert!(webfetch_model.contains("abc123"));

    let file_body = "line one\nline two\nline three".to_string();
    let read_text =
        format!("<path>/tmp/a.rs</path>\n<type>file</type>\n<content>\n{file_body}\n</content>");
    let read_content = ToolContent::Mixed {
        text: Some(read_text.clone()),
        json: Some(json!({
            "preview": "line one\nline two\nline three",
            "truncated": false,
            "loaded": [],
        })),
    };
    let read_model = serialize_tool_content_for_model(read_content, Some("read"));
    assert_eq!(read_model, read_text);
    assert_eq!(read_model.matches("line one").count(), 1);
    assert!(!read_model.contains("\"preview\""));
    assert!(!read_model.contains("\"truncated\""));
}

const HOSTED_DSML_TEXT: &str = "<｜｜DSML｜｜tool_calls>\n<｜｜DSML｜｜invoke name=\"web_search\">\n<｜｜DSML｜｜parameter name=\"query\" string=\"true\">current Rust docs</｜｜DSML｜｜parameter>\n</｜｜DSML｜｜invoke>\n</｜｜DSML｜｜tool_calls>";

type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>;

fn empty_schema() -> JsonSchema {
    JsonSchema::object(Default::default(), None, None)
}

fn named_spec(name: &str, description: &str, mode: ToolExecutionMode, parallel: bool) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.into(),
        input_schema: empty_schema(),
        output_mode: ToolOutputMode::Text,
        execution_mode: mode,
        capability_tags: vec![],
        supports_parallel: parallel,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    }
}

fn leak_spec(name: &'static str, description: &'static str) -> &'static ToolSpec {
    Box::leak(Box::new(ToolSpec::new(name, description, empty_schema())))
}

fn ok_tool_result(text: &str, display: Option<String>) -> crate::tools::contracts::ToolResult {
    let mut result = crate::tools::contracts::ToolResult::success(
        crate::tools::contracts::ToolResultContent::Text(text.into()),
        "done",
    );
    result.display_content = display;
    result
}

fn final_text_stream(text: &str) -> EventStream {
    Box::pin(futures::stream::iter(vec![Ok(text_done(
        "resp-final",
        text,
    ))]))
}

fn text_done(id: &str, text: &str) -> StreamEvent {
    StreamEvent::MessageDone {
        response: ModelResponse {
            id: id.into(),
            content: vec![ResponseContent::Text(text.into())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: Default::default(),
        },
    }
}

fn hosted_tool_stream(
    id: &str,
    name: &str,
    input: serde_json::Value,
    output: serde_json::Value,
) -> EventStream {
    Box::pin(futures::stream::iter(vec![
        Ok(StreamEvent::HostedToolCallStart {
            index: 0,
            id: id.into(),
            name: name.into(),
            input: input.clone(),
        }),
        Ok(StreamEvent::MessageDone {
            response: ModelResponse {
                id: "resp".into(),
                content: vec![
                    ResponseContent::HostedToolUse {
                        id: id.into(),
                        name: name.into(),
                        input: input.clone(),
                        output: None,
                        status: None,
                    },
                    ResponseContent::HostedToolUse {
                        id: id.into(),
                        name: name.into(),
                        input,
                        output: Some(output),
                        status: Some("completed".into()),
                    },
                ],
                stop_reason: Some(StopReason::ToolUse),
                usage: Usage::default(),
                metadata: Default::default(),
            },
        }),
    ]))
}

fn tool_done(id: &str, call_id: &str, name: &str, input: serde_json::Value) -> StreamEvent {
    StreamEvent::MessageDone {
        response: ModelResponse {
            id: id.into(),
            content: vec![ResponseContent::ToolUse {
                id: call_id.into(),
                name: name.into(),
                input,
            }],
            stop_reason: Some(StopReason::ToolUse),
            usage: Usage::default(),
            metadata: Default::default(),
        },
    }
}

fn tool_use_pair(
    index: usize,
    id: &str,
    name: &str,
    input: serde_json::Value,
) -> (StreamEvent, ResponseContent) {
    (
        StreamEvent::ToolCallStart {
            index,
            id: id.into(),
            name: name.into(),
            input: input.clone(),
        },
        ResponseContent::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        },
    )
}

#[derive(Clone, Copy)]
enum StreamScript {
    AlwaysDone,
    FailCreateThenDone(&'static str),
    FailEventThenDone(&'static str),
    RateLimitThenDone,
    HostedWebSearch,
    HostedDsml,
    HostedWebFetch,
    SingleMutating,
    InterleavedMutating,
    ParallelDelay,
    CapturingMutating,
    UnadvertisedWrite,
    CacheUsageThenDone,
    RepeatedToolCall(u32),
}

struct ScriptedProvider {
    name: &'static str,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    attempts: AtomicUsize,
    script: StreamScript,
}

impl ScriptedProvider {
    fn capturing(name: &'static str, requests: Arc<Mutex<Vec<ModelRequest>>>) -> Self {
        Self {
            name,
            requests,
            attempts: AtomicUsize::new(0),
            script: StreamScript::AlwaysDone,
        }
    }

    fn scripted(name: &'static str, script: StreamScript) -> Self {
        Self {
            name,
            requests: Arc::new(Mutex::new(Vec::new())),
            attempts: AtomicUsize::new(0),
            script,
        }
    }

    fn capturing_script(
        name: &'static str,
        requests: Arc<Mutex<Vec<ModelRequest>>>,
        script: StreamScript,
    ) -> Self {
        Self {
            name,
            requests,
            attempts: AtomicUsize::new(0),
            script,
        }
    }
}

#[async_trait]
impl ModelProviderSDK for ScriptedProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        unreachable!("tests stream responses only")
    }

    async fn completion_stream(&self, request: ModelRequest) -> Result<EventStream> {
        self.requests.lock().expect("lock requests").push(request);
        let n = self.attempts.fetch_add(1, Ordering::SeqCst);
        match self.script {
            StreamScript::AlwaysDone => Ok(Box::pin(futures::stream::iter(vec![Ok(text_done(
                "resp", "done",
            ))]))),
            StreamScript::FailCreateThenDone(msg) => {
                if n == 0 {
                    return Err(anyhow::anyhow!(msg));
                }
                Ok(Box::pin(futures::stream::iter(vec![Ok(text_done(
                    "resp", "done",
                ))])))
            }
            StreamScript::FailEventThenDone(msg) => {
                if n == 0 {
                    return Ok(Box::pin(futures::stream::iter(vec![Err(anyhow::anyhow!(
                        msg
                    ))])));
                }
                Ok(Box::pin(futures::stream::iter(vec![Ok(text_done(
                    "resp", "done",
                ))])))
            }
            StreamScript::RateLimitThenDone => {
                if n < 2 {
                    return Err(anyhow::anyhow!("429 rate limit exceeded"));
                }
                Ok(final_text_stream("done"))
            }
            StreamScript::HostedWebSearch if n == 0 => Ok(hosted_tool_stream(
                "hosted_ws_1",
                "web_search",
                json!({ "query": "current Rust docs" }),
                json!({
                    "results": [{
                        "title": "Rust documentation",
                        "url": "https://example.test/rust"
                    }]
                }),
            )),
            StreamScript::HostedWebSearch => Ok(final_text_stream("done")),
            StreamScript::HostedDsml if n == 0 => Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: HOSTED_DSML_TEXT.to_string(),
                }),
                Ok(text_done("resp-dsml", HOSTED_DSML_TEXT)),
            ]))),
            StreamScript::HostedDsml => Ok(final_text_stream("done")),
            StreamScript::HostedWebFetch if n == 0 => Ok(hosted_tool_stream(
                "hosted_wf_1",
                "web_fetch",
                json!({ "url": "https://example.test/docs" }),
                json!({ "title": "Docs", "url": "https://example.test/docs" }),
            )),
            StreamScript::HostedWebFetch => Ok(final_text_stream("done")),
            StreamScript::SingleMutating if n == 0 => Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "tool-1".into(),
                    name: "mutating_tool".into(),
                    input: json!({}),
                }),
                Ok(StreamEvent::ToolCallInputDelta {
                    index: 0,
                    partial_json: r#"{"value":1}"#.into(),
                }),
                Ok(tool_done(
                    "resp-1",
                    "tool-1",
                    "mutating_tool",
                    json!({ "value": 1 }),
                )),
            ]))),
            StreamScript::SingleMutating => Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "done".into(),
                }),
                Ok(text_done("resp-2", "done")),
            ]))),
            StreamScript::CapturingMutating if n == 0 => Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "tool-1".into(),
                    name: "mutating_tool".into(),
                    input: json!({}),
                }),
                Ok(tool_done("resp-1", "tool-1", "mutating_tool", json!({}))),
            ]))),
            StreamScript::CapturingMutating => Ok(Box::pin(futures::stream::iter(vec![Ok(
                text_done("resp-2", "done"),
            )]))),
            StreamScript::UnadvertisedWrite if n == 0 => {
                Ok(Box::pin(futures::stream::iter(vec![Ok(tool_done(
                    "resp-write",
                    "call-write",
                    "write",
                    json!({}),
                ))])))
            }
            StreamScript::UnadvertisedWrite => Ok(final_text_stream("done")),
            StreamScript::CacheUsageThenDone => Ok(Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-cache-usage".into(),
                        content: vec![ResponseContent::Text("cached response".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage {
                            input_tokens: 100,
                            output_tokens: 4,
                            cache_creation_input_tokens: None,
                            cache_read_input_tokens: Some(73),
                            reasoning_output_tokens: None,
                            total_tokens: Some(104),
                        },
                        metadata: Default::default(),
                    },
                },
            )]))),
            StreamScript::RepeatedToolCall(repeats) => {
                // Same (name, input) every attempt — the pathological loop the
                // query loop guard steers against. Vary only the ids, exactly
                // like a real provider would.
                if (n as u32) < repeats {
                    Ok(Box::pin(futures::stream::iter(vec![Ok(tool_done(
                        &format!("resp-{n}"),
                        &format!("call-{n}"),
                        "read",
                        json!({"delay_ms": 0, "output": "same"}),
                    ))])))
                } else {
                    Ok(final_text_stream("done"))
                }
            }
            StreamScript::InterleavedMutating if n == 0 => {
                let (start1, use1) = tool_use_pair(0, "tool-1", "mutating_tool", json!({}));
                let (start2, use2) = tool_use_pair(1, "tool-2", "mutating_tool", json!({}));
                Ok(Box::pin(futures::stream::iter(vec![
                    Ok(start1),
                    Ok(start2),
                    Ok(StreamEvent::ToolCallInputDelta {
                        index: 0,
                        partial_json: r#"{"value":1}"#.into(),
                    }),
                    Ok(StreamEvent::ToolCallInputDelta {
                        index: 1,
                        partial_json: r#"{"value":2}"#.into(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-1".into(),
                            content: vec![use1, use2],
                            stop_reason: Some(StopReason::ToolUse),
                            usage: Usage::default(),
                            metadata: Default::default(),
                        },
                    }),
                ])))
            }
            StreamScript::InterleavedMutating => Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "done".into(),
                }),
                Ok(text_done("resp-2", "done")),
            ]))),
            StreamScript::ParallelDelay if n == 0 => {
                let slow = json!({ "delay_ms": 50, "output": "slow complete" });
                let fast = json!({ "delay_ms": 5, "output": "fast complete" });
                let (start1, use1) = tool_use_pair(0, "slow", "parallel_tool", slow);
                let (start2, use2) = tool_use_pair(1, "fast", "parallel_tool", fast);
                Ok(Box::pin(futures::stream::iter(vec![
                    Ok(start1),
                    Ok(start2),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-1".into(),
                            content: vec![use1, use2],
                            stop_reason: Some(StopReason::ToolUse),
                            usage: Usage::default(),
                            metadata: Default::default(),
                        },
                    }),
                ])))
            }
            StreamScript::ParallelDelay => Ok(Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-2".into(),
                        content: vec![ResponseContent::Text("done".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                },
            )]))),
        }
    }

    fn name(&self) -> &str {
        self.name
    }
}

enum CompactionProviderOutcome {
    Summary,
    Error,
}

struct CompactionProvider {
    completion_calls: AtomicUsize,
    outcome: CompactionProviderOutcome,
}

#[async_trait]
impl ModelProviderSDK for CompactionProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        self.completion_calls.fetch_add(1, Ordering::SeqCst);
        match &self.outcome {
            CompactionProviderOutcome::Summary => Ok(ModelResponse {
                id: "compaction-response".to_string(),
                content: vec![ResponseContent::Text("summary".to_string())],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
                metadata: Default::default(),
            }),
            CompactionProviderOutcome::Error => Err(anyhow::anyhow!("compaction provider failed")),
        }
    }

    async fn completion_stream(&self, _request: ModelRequest) -> Result<EventStream> {
        unreachable!("tests call the non-streaming compaction path only")
    }

    fn name(&self) -> &str {
        "compaction-provider"
    }
}

#[derive(Clone)]
struct ScriptStreamResponse {
    preamble: Vec<StreamEvent>,
    content: Vec<ResponseContent>,
    metadata: ResponseMetadata,
}

struct ScriptStreamProvider {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    calls: AtomicUsize,
    responses: Vec<ScriptStreamResponse>,
    name: &'static str,
}

impl ScriptStreamProvider {
    fn new(name: &'static str, responses: Vec<ScriptStreamResponse>) -> Arc<Self> {
        Arc::new(Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            calls: AtomicUsize::new(0),
            responses,
            name,
        })
    }
}

#[async_trait]
impl ModelProviderSDK for ScriptStreamProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        unreachable!("tests stream responses only")
    }

    async fn completion_stream(&self, request: ModelRequest) -> Result<EventStream> {
        self.requests.lock().expect("lock requests").push(request);
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let script = self
            .responses
            .get(call)
            .or_else(|| self.responses.last())
            .expect("script stream provider needs at least one response");
        let mut events: Vec<Result<StreamEvent>> =
            script.preamble.iter().cloned().map(Ok).collect();
        events.push(Ok(StreamEvent::MessageDone {
            response: ModelResponse {
                id: format!("resp-{call}"),
                content: script.content.clone(),
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
                metadata: script.metadata.clone(),
            },
        }));
        Ok(Box::pin(futures::stream::iter(events)))
    }

    fn name(&self) -> &str {
        self.name
    }
}

struct StaticTool {
    spec: &'static ToolSpec,
    text: &'static str,
    display: Option<&'static str>,
    executions: Option<Arc<AtomicUsize>>,
}

#[async_trait]
impl ToolHandler for StaticTool {
    fn spec(&self) -> &ToolSpec {
        self.spec
    }

    async fn handle(
        &self,
        _ctx: crate::tools::contracts::ToolContext,
        _input: serde_json::Value,
        _progress: Option<crate::tools::contracts::ToolProgressSender>,
    ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError> {
        if let Some(executions) = &self.executions {
            executions.fetch_add(1, Ordering::SeqCst);
        }
        Ok(ok_tool_result(self.text, self.display.map(str::to_string)))
    }
}

fn mutating_tool() -> StaticTool {
    static_tool("write", "ok", None, None)
}

fn display_content_tool() -> StaticTool {
    static_tool("read", "canonical", Some("display"), None)
}

fn streaming_mutating_tool() -> StaticTool {
    static_tool("write", "stream complete", None, None)
}

fn counting_web_search_tool(executions: Arc<AtomicUsize>) -> StaticTool {
    static_tool("web_search", "local search", None, Some(executions))
}

fn counting_web_fetch_tool(executions: Arc<AtomicUsize>) -> StaticTool {
    static_tool("webfetch", "local fetch", None, Some(executions))
}

fn counting_write_tool(executions: Arc<AtomicUsize>) -> StaticTool {
    static_tool("write", "write a file", None, Some(executions))
}

fn static_tool(
    name: &'static str,
    text: &'static str,
    display: Option<&'static str>,
    executions: Option<Arc<AtomicUsize>>,
) -> StaticTool {
    StaticTool {
        spec: leak_spec(name, "tool"),
        text,
        display,
        executions,
    }
}

struct LargeToolResultTool {
    spec: &'static ToolSpec,
    content: String,
    display_content: Option<String>,
}

impl LargeToolResultTool {
    fn new(content: String, display_content: Option<String>) -> Self {
        Self {
            spec: leak_spec("read", "read tool"),
            content,
            display_content,
        }
    }
}

#[async_trait]
impl ToolHandler for LargeToolResultTool {
    fn spec(&self) -> &ToolSpec {
        self.spec
    }

    async fn handle(
        &self,
        _ctx: crate::tools::contracts::ToolContext,
        _input: serde_json::Value,
        _progress: Option<crate::tools::contracts::ToolProgressSender>,
    ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError> {
        Ok(ok_tool_result(&self.content, self.display_content.clone()))
    }
}

struct ParallelDelayTool {
    spec: &'static ToolSpec,
}

impl ParallelDelayTool {
    fn new() -> Self {
        Self {
            spec: leak_spec("read", "read tool"),
        }
    }
}

#[async_trait]
impl ToolHandler for ParallelDelayTool {
    fn spec(&self) -> &ToolSpec {
        self.spec
    }

    async fn handle(
        &self,
        _ctx: crate::tools::contracts::ToolContext,
        input: serde_json::Value,
        _progress: Option<crate::tools::contracts::ToolProgressSender>,
    ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError> {
        let delay_ms = input
            .get("delay_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        let output = input
            .get("output")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        Ok(ok_tool_result(output, None))
    }
}

fn empty_query_env(user: &str) -> (SessionState, Arc<ToolRegistry>, ToolRuntime) {
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user(user));
    (session, registry, runtime)
}

struct CapturingFixtures {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    provider: Arc<dyn ModelProviderSDK>,
    registry: Arc<ToolRegistry>,
    runtime: ToolRuntime,
}

impl CapturingFixtures {
    fn new(name: &'static str) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> =
            Arc::new(ScriptedProvider::capturing(name, Arc::clone(&requests)));
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        Self {
            requests,
            provider,
            registry,
            runtime,
        }
    }

    async fn query(
        &self,
        session: &mut SessionState,
        turn_config: &TurnConfig,
        options: QueryOptions,
        callback: Option<EventCallback>,
    ) -> Result<(), AgentError> {
        query(
            session,
            turn_config,
            Arc::clone(&self.provider),
            Arc::clone(&self.registry),
            &self.runtime,
            callback,
            options,
        )
        .await
    }
}

struct ScriptToolFixtures {
    registry: Arc<ToolRegistry>,
    runtime: ToolRuntime,
    session: SessionState,
}

impl ScriptToolFixtures {
    fn new(handler: Arc<dyn ToolHandler>, spec: ToolSpec, user_message: &str) -> Self {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(&spec.name, handler);
        builder.push_spec(spec);
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user(user_message));
        Self {
            registry,
            runtime,
            session,
        }
    }

    async fn query(
        &mut self,
        provider: Arc<dyn ModelProviderSDK>,
        callback: Option<EventCallback>,
        turn_config: TurnConfig,
        options: QueryOptions,
    ) -> Result<(), AgentError> {
        query(
            &mut self.session,
            &turn_config,
            provider,
            Arc::clone(&self.registry),
            &self.runtime,
            callback,
            options,
        )
        .await
    }
}

fn compaction_req<'a>(
    provider: &'a Arc<dyn ModelProviderSDK>,
) -> super::CompactionModelRequest<'a> {
    super::CompactionModelRequest {
        journal: None,
        provider,
        model_slug: "compaction-model",
        request_model: "compaction-request-model",
        max_tokens: 4096,
    }
}

fn retry_statuses(events: &[QueryEvent]) -> Vec<ProviderRetryStatus> {
    events
        .iter()
        .filter_map(|event| match event {
            QueryEvent::ProviderRetryStatus(status) => Some(status.clone()),
            _ => None,
        })
        .collect()
}

fn tool_use_start_events(events: &[QueryEvent]) -> Vec<(String, String, serde_json::Value)> {
    events
        .iter()
        .filter_map(|event| match event {
            QueryEvent::ToolUseStart { id, name, input } => {
                Some((id.clone(), name.clone(), input.clone()))
            }
            _ => None,
        })
        .collect()
}

type HostedToolResultRow = (
    String,
    String,
    serde_json::Value,
    Option<String>,
    Option<serde_json::Value>,
    bool,
);

fn tool_result_events(events: &[QueryEvent]) -> Vec<HostedToolResultRow> {
    events
        .iter()
        .filter_map(|event| match event {
            QueryEvent::ToolResult {
                tool_use_id,
                tool_name,
                input,
                content,
                is_error,
                ..
            } => {
                let (text, json) = match content {
                    ToolContent::Text(text) => (Some(text.clone()), None),
                    ToolContent::Json(json) => (None, Some(json.clone())),
                    ToolContent::Mixed { text, json } => (text.clone(), json.clone()),
                };
                Some((
                    tool_use_id.clone(),
                    tool_name.clone(),
                    input.clone(),
                    text,
                    json,
                    *is_error,
                ))
            }
            _ => None,
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum RecordedCompactionEvent {
    Started,
    Completed,
    Failed { message: String },
}

fn recorded_compaction_events(events: &[QueryEvent]) -> Vec<RecordedCompactionEvent> {
    events
        .iter()
        .filter_map(|event| match event {
            QueryEvent::ContextCompactionStarted => Some(RecordedCompactionEvent::Started),
            QueryEvent::ContextCompactionCompleted { .. } => {
                Some(RecordedCompactionEvent::Completed)
            }
            QueryEvent::ContextCompactionFailed { message } => {
                Some(RecordedCompactionEvent::Failed {
                    message: message.clone(),
                })
            }
            _ => None,
        })
        .collect()
}

fn recording_callback(events: &Arc<Mutex<Vec<QueryEvent>>>) -> EventCallback {
    let captured_events = Arc::clone(events);
    Arc::new(move |event| {
        let captured_events = Arc::clone(&captured_events);
        Box::pin(async move {
            captured_events.lock().expect("lock events").push(event);
        })
    })
}

fn compaction_test_session(total_input_tokens: usize) -> SessionState {
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("x".repeat(80_004)));
    session.push_message(Message::user("latest"));
    session.total_input_tokens = total_input_tokens;
    session
}

#[tokio::test]
async fn compaction_events_follow_outcome() {
    for (kind, total_input_tokens, outcome, expected_events, expected_calls) in [
        (
            CompactionKind::Auto,
            200_000,
            CompactionProviderOutcome::Summary,
            vec![
                RecordedCompactionEvent::Started,
                RecordedCompactionEvent::Completed,
            ],
            1,
        ),
        (
            CompactionKind::Auto,
            0,
            CompactionProviderOutcome::Summary,
            vec![
                RecordedCompactionEvent::Started,
                RecordedCompactionEvent::Failed {
                    message: "Context compaction skipped: nothing to compact".to_string(),
                },
            ],
            0,
        ),
        (
            CompactionKind::Proactive,
            0,
            CompactionProviderOutcome::Error,
            vec![
                RecordedCompactionEvent::Started,
                RecordedCompactionEvent::Failed {
                    message: "summarization failed: compaction provider failed".to_string(),
                },
            ],
            5,
        ),
    ] {
        let provider = Arc::new(CompactionProvider {
            completion_calls: AtomicUsize::new(0),
            outcome,
        });
        let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let on_event = Some(recording_callback(&events));
        let mut session = compaction_test_session(total_input_tokens);
        let original_messages = session.prompt_source_messages().to_vec();

        super::summarize_and_compact(
            &mut session,
            &on_event,
            compaction_req(&provider_sdk),
            kind,
            /*cancel_token*/ None,
        )
        .await;

        assert_eq!(
            recorded_compaction_events(&events.lock().expect("lock events")),
            expected_events
        );
        assert_eq!(
            provider.completion_calls.load(Ordering::SeqCst),
            expected_calls
        );
        if expected_calls == 1 {
            let ResponseItem::Message(expected_summary) =
                CompactionSummary::new("summary").to_response_item()
            else {
                unreachable!("compaction summaries are messages");
            };
            assert_eq!(
                session.prompt_source_messages(),
                &[expected_summary, Message::user("latest")]
            );
        } else {
            assert_eq!(session.prompt_source_messages(), original_messages);
        }
    }
}

#[tokio::test]
async fn query_retries_transient_stream_errors() {
    #[derive(Clone, Copy)]
    enum RetryExpectation {
        None,
        EventStatuses {
            backoff_ms: u64,
            message: &'static str,
        },
    }

    for (script, provider_name, expectation) in [
        (
            StreamScript::FailCreateThenDone("503 service unavailable"),
            "transient-stream-create-provider",
            RetryExpectation::None,
        ),
        (
            StreamScript::FailEventThenDone("500 internal server error"),
            "transient-stream-event-provider",
            RetryExpectation::EventStatuses {
                backoff_ms: 250,
                message: "500 internal server error",
            },
        ),
    ] {
        let provider = Arc::new(ScriptedProvider::scripted(provider_name, script));
        let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
        let (mut session, registry, runtime) = empty_query_env("hello");
        let turn_config = TurnConfig::new(Model::default(), None);
        let model = turn_config.model.slug.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let callback = recording_callback(&events);

        query(
            &mut session,
            &turn_config,
            provider_sdk,
            registry,
            &runtime,
            Some(callback),
            QueryOptions::default(),
        )
        .await
        .expect("query should retry and succeed");

        assert_eq!(provider.attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            session
                .messages
                .iter()
                .filter(|message| message.role == Role::Assistant)
                .cloned()
                .collect::<Vec<_>>(),
            vec![Message::assistant_text("done")]
        );
        match expectation {
            RetryExpectation::None => {}
            RetryExpectation::EventStatuses {
                backoff_ms,
                message,
            } => {
                let retry_statuses = retry_statuses(&events.lock().expect("lock events"));
                let expected: Vec<_> = [
                    (ModelQueryRetryPhase::Scheduled, backoff_ms),
                    (ModelQueryRetryPhase::Resumed, 0),
                ]
                .into_iter()
                .map(|(phase, backoff_ms)| ProviderRetryStatus {
                    provider: provider_name.to_string(),
                    model: model.clone(),
                    attempt: 1,
                    max_attempts: 5,
                    backoff_ms,
                    phase,
                    message: message.to_string(),
                })
                .collect();
                assert_eq!(retry_statuses, expected);
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn query_waits_sixty_seconds_for_each_rate_limit_retry() {
    let provider = Arc::new(ScriptedProvider::scripted(
        "rate-limited-stream-create-provider",
        StreamScript::RateLimitThenDone,
    ));
    let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
    let (mut session, registry, runtime) = empty_query_env("hello");
    let turn_config = TurnConfig::new(Model::default(), None);
    let model = turn_config.model.slug.clone();
    let events = Arc::new(Mutex::new(Vec::new()));
    let callback = recording_callback(&events);
    let started_at = tokio::time::Instant::now();

    query(
        &mut session,
        &turn_config,
        provider_sdk,
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should retry and succeed");

    assert_eq!(
        tokio::time::Instant::now().duration_since(started_at),
        std::time::Duration::from_secs(120)
    );
    let retry_statuses = retry_statuses(&events.lock().expect("lock events"));
    let expected: Vec<_> = [1_u32, 2]
        .into_iter()
        .flat_map(|attempt| {
            [
                (attempt, ModelQueryRetryPhase::Scheduled, 60_000_u64),
                (attempt, ModelQueryRetryPhase::Resumed, 0),
            ]
        })
        .map(|(attempt, phase, backoff_ms)| ProviderRetryStatus {
            provider: "rate-limited-stream-create-provider".to_string(),
            model: model.clone(),
            attempt: attempt as usize,
            max_attempts: 5,
            backoff_ms,
            phase,
            message: "429 rate limit exceeded".to_string(),
        })
        .collect();
    assert_eq!(retry_statuses, expected);
    assert_eq!(provider.attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test(start_paused = true)]
async fn query_cancels_retry_backoff_before_second_attempt() {
    for script in [
        StreamScript::FailCreateThenDone("503 service unavailable"),
        StreamScript::FailEventThenDone("500 internal server error"),
    ] {
        let provider = Arc::new(ScriptedProvider::scripted("retry-provider", script));
        let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
        let (mut session, registry, runtime) = empty_query_env("hello");
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();

        let result = query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider_sdk,
            registry,
            &runtime,
            None,
            QueryOptions {
                cancel_token: Some(cancel_token),
                ..QueryOptions::default()
            },
        )
        .await;

        assert!(matches!(result, Err(AgentError::Aborted)));
        assert_eq!(provider.attempts.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn query_injects_steering_note_for_identical_tool_batch_loop() {
    let provider = Arc::new(ScriptedProvider::scripted(
        "looping-provider",
        StreamScript::RepeatedToolCall(7),
    ));
    let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
    let mut fixtures = ScriptToolFixtures::new(
        Arc::new(ParallelDelayTool::new()),
        leak_spec("read", "read tool").clone(),
        "loop please",
    );
    let turn_config = TurnConfig::new(Model::default(), None);

    fixtures
        .query(provider_sdk, None, turn_config, QueryOptions::default())
        .await
        .expect("query should complete after the loop is steered");

    // 7 identical batches → streak crosses 5 exactly once (next note at 10).
    let notes = fixtures
        .session
        .prompt_source_messages()
        .iter()
        .filter(|message| {
            message.role == Role::User
                && message
                    .content
                    .iter()
                    .any(|block| matches!(block, crate::ContentBlock::Text { text } if text.contains("repeated 5 times in a row")))
        })
        .count();
    assert_eq!(notes, 1, "expected exactly one loop-guard steering note");
    // The note must be model-visible: the request after the 5th batch
    // carries it above the repeated tool results.
    let requests = provider.requests.lock().expect("lock requests");
    assert!(requests.len() >= 7, "expected 8 provider attempts");
    assert!(
        requests[5]
            .messages
            .iter()
            .any(|message| message
                .content
                .iter()
                .any(|content| matches!(content, RequestContent::Text { text } if text.contains("repeated 5 times in a row")))),
        "6th request should carry the steering note"
    );
}

#[tokio::test]
async fn model_request_exposes_only_python_kernel_schema() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::capturing_script(
        "python-only-capture-provider",
        Arc::clone(&requests),
        StreamScript::AlwaysDone,
    ));
    let ipython = IpythonHandler::new();
    let mut builder = ToolRegistryBuilder::new();
    builder.push_spec_with_exposure(ipython.spec().clone(), ToolExposure::Direct);
    builder.register_handler("ipython", Arc::new(ipython));
    for (name, description) in [
        ("shell_command", "Run a shell command."),
        ("ToolSearch", "Load another tool schema."),
        ("web_search", "Search the web."),
        ("spawn_agent", "Delegate work."),
    ] {
        builder.push_spec_with_exposure(
            ToolSpec::new(name, description, empty_schema()),
            ToolExposure::Direct,
        );
    }
    let registry = Arc::new(builder.build());
    assert_eq!(registry.tool_definitions().len(), 5);
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("inspect available tools"));
    let runtime = ToolRuntime::new_with_context(
        Arc::clone(&registry),
        PermissionChecker::always_allow(),
        ToolRuntimeContext {
            session_id: devo_protocol::SessionId::from(session.id.as_str()),
            ..ToolRuntimeContext::default()
        },
    );
    let options = QueryOptions {
        harness_digest: Some("SYNTHETIC_HARNESS_DIGEST".to_string()),
        ..QueryOptions::default()
    };

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider,
        Arc::clone(&registry),
        &runtime,
        None,
        options,
    )
    .await
    .expect("query should finish");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 1);
    let tools = captured[0].tools.as_ref().expect("Python tool schema");
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["ipython"],
        "only the Python kernel schema may reach a provider"
    );
    let schema = serde_json::to_value(&tools[0].input_schema).expect("serialize schema");
    assert_eq!(schema["required"], json!(["code"]));
    assert_eq!(schema["properties"]["code"]["type"], "string");
    assert!(captured[0].hosted_tools.is_empty());
    assert_eq!(
        captured[0]
            .messages
            .iter()
            .filter(|message| message_contains(message, "SYNTHETIC_HARNESS_DIGEST"))
            .count(),
        1,
        "inject the synthetic memory digest once per model request"
    );
}
#[tokio::test]
async fn query_accounts_provider_cache_read_tokens_without_double_counting_input() {
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::scripted(
        "cache-usage-capture-provider",
        StreamScript::CacheUsageThenDone,
    ));
    let registry = Arc::new(ToolRegistryBuilder::new().build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("synthetic cache-accounting probe"));

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider,
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    assert_eq!(session.total_input_tokens, 100);
    assert_eq!(session.total_output_tokens, 4);
    assert_eq!(session.total_tokens, 104);
    assert_eq!(session.total_cache_read_tokens, 73);
}

#[tokio::test]
async fn query_exposes_only_ipython_and_appends_subagent_warning() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::capturing(
        "capturing-provider",
        Arc::clone(&requests),
    ));
    let mut builder = ToolRegistryBuilder::new();
    let ipython = IpythonHandler::new();
    builder.push_spec_with_exposure(ipython.spec().clone(), ToolExposure::Direct);
    builder.register_handler("ipython", Arc::new(ipython));
    for (name, description) in [
        ("ToolSearch", "Search available tools."),
        ("web_search", "Search the web."),
        ("spawn_agent", "Create a child agent."),
        ("send_message", "Send input to a child agent."),
        ("await_task", "Wait for task completion."),
        ("list_tasks", "List child tasks."),
        ("cancel_task", "Cancel a child task."),
    ] {
        builder.push_spec_with_exposure(
            ToolSpec::new(name, description, empty_schema()),
            ToolExposure::Direct,
        );
    }
    builder.restrict_model_exposure_to(&["ipython"]);
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_with_context(
        Arc::clone(&registry),
        PermissionChecker::always_allow(),
        ToolRuntimeContext {
            agent_scope: ToolAgentScope::Subagent,
            ..ToolRuntimeContext::default()
        },
    );
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("work on the delegated task"));
    let mut turn_config = TurnConfig::new(
        Model {
            base_instructions: "base system".to_string(),
            ..Model::default()
        },
        None,
    );
    turn_config.web_search =
        devo_config::ResolvedWebSearchConfig::Local(devo_config::ResolvedLocalWebSearchConfig {
            provider_id: "test".into(),
            kind: devo_config::LocalWebSearchProviderKind::Exa,
            api_key: "secret".into(),
            base_url: None,
            max_results: None,
        });

    query(
        &mut session,
        &turn_config,
        provider,
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 1);
    let request = &captured[0];
    let tool_names = request
        .tools
        .as_ref()
        .expect("tools should be present")
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(tool_names, vec!["ipython"]);
    assert!(request.hosted_tools.is_empty());
    let system = request.system.as_deref().expect("system prompt");
    let mode_prompt =
        crate::collaboration_mode_prompts::mode_introductions_prompt(CollaborationMode::Build);
    assert!(system.contains("base system"));
    assert!(!system.contains("<collaboration_mode_plan>"));
    assert!(system.contains(&mode_prompt));
    assert!(system.contains("await rlm.web_search(query)"));
    assert!(!system.contains("spawn_agent"));
    assert!(
        request
            .messages
            .iter()
            .all(|message| !message_contains(message, "web_search: Search the web."))
    );
    assert!(
        request_message_index_containing(request, "You are running as a sub-agent")
            < request_message_index_containing(request, "work on the delegated task")
    );
    assert!(
        request
            .messages
            .iter()
            .any(|message| message_contains(message, "<context_changes>"))
    );
}

/// Trace: L2-DES-RESEARCH-001
/// Verifies: provider-hosted tools emit normal tool events without local execution.
#[tokio::test]
async fn provider_hosted_tools_emit_events_without_local_execution() {
    enum HostedToolKind {
        WebSearch,
        WebFetch,
    }

    for (kind, script, local_tool, hosted_id, hosted_name, user_message, tool_input, result_json) in [
        (
            HostedToolKind::WebSearch,
            StreamScript::HostedWebSearch,
            "web_search",
            "hosted_ws_1",
            "web_search",
            "search current docs",
            json!({ "query": "current Rust docs" }),
            json!({
                "results": [{
                    "title": "Rust documentation",
                    "url": "https://example.test/rust"
                }]
            }),
        ),
        (
            HostedToolKind::WebFetch,
            StreamScript::HostedWebFetch,
            "webfetch",
            "hosted_wf_1",
            "web_fetch",
            "fetch docs",
            json!({ "url": "https://example.test/docs" }),
            json!({ "title": "Docs", "url": "https://example.test/docs" }),
        ),
    ] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::capturing_script(
            "hosted-tool-provider",
            Arc::clone(&requests),
            script,
        ));
        let executions = Arc::new(AtomicUsize::new(0));
        let mut builder = ToolRegistryBuilder::new();
        let ipython = IpythonHandler::new();
        builder.push_spec_with_exposure(ipython.spec().clone(), ToolExposure::Direct);
        builder.register_handler("ipython", Arc::new(ipython));
        let handler: Arc<dyn ToolHandler> = match kind {
            HostedToolKind::WebSearch => {
                Arc::new(counting_web_search_tool(Arc::clone(&executions)))
            }
            HostedToolKind::WebFetch => Arc::new(counting_web_fetch_tool(Arc::clone(&executions))),
        };
        builder.register_handler(local_tool, handler);
        builder.push_spec(match kind {
            HostedToolKind::WebSearch => named_spec(
                local_tool,
                "Search the web.",
                ToolExecutionMode::ReadOnly,
                false,
            ),
            HostedToolKind::WebFetch => {
                let mut spec = named_spec(
                    local_tool,
                    "Fetch a URL.",
                    ToolExecutionMode::ReadOnly,
                    false,
                );
                spec.output_mode = ToolOutputMode::Mixed;
                spec
            }
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user(user_message));
        let mut turn_config = TurnConfig::new(
            Model {
                // Use a wire that supports provider-hosted web search.
                provider: devo_protocol::ProviderWireApi::AnthropicMessages,
                base_instructions: if matches!(kind, HostedToolKind::WebSearch) {
                    "base system".to_string()
                } else {
                    String::new()
                },
                ..Model::default()
            },
            None,
        );
        match kind {
            HostedToolKind::WebSearch => {
                turn_config.web_search = devo_config::ResolvedWebSearchConfig::Provider;
            }
            HostedToolKind::WebFetch => {
                turn_config.web_fetch = devo_config::ResolvedWebFetchConfig::Provider;
            }
        }
        let seen = Arc::new(Mutex::new(Vec::new()));
        let callback = recording_callback(&seen);

        query(
            &mut session,
            &turn_config,
            provider,
            registry,
            &runtime,
            Some(callback),
            QueryOptions::default(),
        )
        .await
        .expect("query should complete");

        assert_eq!(executions.load(Ordering::SeqCst), 0);
        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 2);
        let request = &captured[0];
        match kind {
            HostedToolKind::WebSearch => assert!(matches!(
                request.hosted_tools.as_slice(),
                [devo_protocol::HostedToolDefinition::WebSearch(_)]
            )),
            HostedToolKind::WebFetch => assert!(matches!(
                request.hosted_tools.as_slice(),
                [devo_protocol::HostedToolDefinition::WebFetch(_)]
            )),
        }
        assert_eq!(
            request
                .tools
                .as_ref()
                .expect("Python tool schema")
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["ipython"]
        );
        if matches!(kind, HostedToolKind::WebSearch) {
            let system = request.system.as_deref().expect("system prompt");
            assert!(system.contains("base system"));
            assert!(system.contains("Sources:"));
            assert!(system.contains("The current month is "));
        }
        assert!(
            request
                .tools
                .as_ref()
                .is_none_or(|tools| { tools.iter().all(|tool| tool.name != local_tool) })
        );
        let continuation = &captured[1];
        assert!(continuation.messages.iter().any(|message| {
            message.content.iter().any(|content| {
                matches!(
                    content,
                    RequestContent::HostedToolUse {
                        id,
                        name,
                        input,
                        output: Some(_),
                        status,
                    } if id == hosted_id
                        && name == hosted_name
                        && input == &tool_input
                        && status.as_deref() == Some("completed")
                )
            })
        }));

        let events = seen.lock().unwrap();
        assert_eq!(
            tool_use_start_events(&events),
            vec![(
                hosted_id.to_string(),
                hosted_name.to_string(),
                tool_input.clone()
            )]
        );
        assert_eq!(
            tool_result_events(&events),
            vec![(
                hosted_id.to_string(),
                hosted_name.to_string(),
                tool_input,
                Some("status: completed".into()),
                Some(result_json),
                false,
            )]
        );
        if matches!(kind, HostedToolKind::WebSearch) {
            assert!(events.iter().any(|event| matches!(
                event,
                QueryEvent::TurnComplete {
                    stop_reason: StopReason::EndTurn
                }
            )));
            assert!(session.messages.iter().all(|message| {
                message.content.iter().all(|block| {
                    !matches!(
                        block,
                        ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
                    )
                })
            }));
        }
    }
}

/// Trace: L2-DES-RESEARCH-001
/// Verifies: DSML text that represents a provider-hosted web_search does not end the query loop.
#[tokio::test]
async fn provider_hosted_dsml_text_tool_call_continues_query_loop() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::capturing_script(
        "hosted-dsml-text-provider",
        Arc::clone(&requests),
        StreamScript::HostedDsml,
    ));
    let mut builder = ToolRegistryBuilder::new();
    let ipython = IpythonHandler::new();
    builder.push_spec_with_exposure(ipython.spec().clone(), ToolExposure::Direct);
    builder.register_handler("ipython", Arc::new(ipython));
    for (name, description) in [
        ("spawn_agent", "Create a child agent."),
        ("await_task", "Wait for task completion."),
    ] {
        builder.push_spec_with_exposure(
            ToolSpec::new(name, description, empty_schema()),
            ToolExposure::Direct,
        );
    }
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("search current docs"));
    let mut turn_config = TurnConfig::new(
        Model {
            provider: devo_protocol::ProviderWireApi::AnthropicMessages,
            ..Model::default()
        },
        None,
    );
    turn_config.web_search = devo_config::ResolvedWebSearchConfig::Provider;

    let seen = Arc::new(Mutex::new(Vec::new()));
    let callback = recording_callback(&seen);

    query(
        &mut session,
        &turn_config,
        provider,
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should continue after DSML text and complete");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    let request = &captured[0];
    assert!(matches!(
        request.hosted_tools.as_slice(),
        [devo_protocol::HostedToolDefinition::WebSearch(_)]
    ));
    assert_eq!(
        request
            .tools
            .as_ref()
            .expect("Python tool schema")
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["ipython"]
    );
    let continuation = &captured[1];
    assert!(continuation.messages.iter().any(|message| {
        message_contains(message, "DSML tagged tool-call text")
            && message_contains(message, "ipython")
    }));
    assert!(continuation.messages.iter().all(|message| {
        !message_contains(message, "spawn_agent") && !message_contains(message, "await_task")
    }));

    assert_eq!(
        session
            .messages
            .iter()
            .filter(|message| message.role == Role::Assistant)
            .cloned()
            .collect::<Vec<_>>(),
        vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: HOSTED_DSML_TEXT.to_string(),
                }],
            },
            Message::assistant_text("done"),
        ]
    );
    assert!(seen.lock().unwrap().iter().any(|event| matches!(
        event,
        QueryEvent::TurnComplete {
            stop_reason: StopReason::EndTurn
        }
    )));
}

#[tokio::test]
async fn query_exposes_only_ipython_for_all_model_channels() {
    let mut builder = ToolRegistryBuilder::new();
    let ipython = IpythonHandler::new();
    builder.push_spec_with_exposure(ipython.spec().clone(), ToolExposure::Direct);
    builder.register_handler("ipython", Arc::new(ipython));
    for (name, description) in [
        ("apply_patch", "Apply a patch."),
        ("write", "Write a file."),
    ] {
        builder.push_spec_with_exposure(
            ToolSpec::new(name, description, empty_schema()),
            ToolExposure::Direct,
        );
    }
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

    for channel in [Some("OpenAI"), Some("Poolside"), None] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::capturing(
            "capturing-provider",
            Arc::clone(&requests),
        ));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));
        query(
            &mut session,
            &TurnConfig::new(
                Model {
                    channel: channel.map(str::to_string),
                    ..Model::default()
                },
                None,
            ),
            provider,
            Arc::clone(&registry),
            &runtime,
            None,
            QueryOptions::default(),
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        let tools = captured[0].tools.as_ref().expect("ipython schema");
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["ipython"]
        );
        assert!(captured[0].hosted_tools.is_empty());
    }
}

#[tokio::test]
async fn configured_local_web_tools_are_described_as_rlm_host_helpers() {
    let mut builder = ToolRegistryBuilder::new();
    let ipython = IpythonHandler::new();
    builder.push_spec_with_exposure(ipython.spec().clone(), ToolExposure::Direct);
    builder.register_handler("ipython", Arc::new(ipython));
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::capturing(
        "capturing-provider",
        Arc::clone(&requests),
    ));
    let mut turn_config = TurnConfig::new(Model::default(), None);
    turn_config.web_search =
        devo_config::ResolvedWebSearchConfig::Local(devo_config::ResolvedLocalWebSearchConfig {
            provider_id: "tavily".into(),
            kind: devo_config::LocalWebSearchProviderKind::Tavily,
            api_key: "test-key".into(),
            base_url: None,
            max_results: Some(5),
        });
    turn_config.web_fetch = devo_config::ResolvedWebFetchConfig::Local;
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("find current Rust release info"));

    query(
        &mut session,
        &turn_config,
        provider,
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0]
            .tools
            .as_ref()
            .expect("ipython schema")
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["ipython"]
    );
    assert!(captured[0].hosted_tools.is_empty());
    let system = captured[0].system.as_deref().expect("system prompt");
    assert!(system.contains("await rlm.web_search(query)"));
    assert!(system.contains("await rlm.web_fetch(url)"));
}

#[tokio::test]
async fn hidden_model_tools_cannot_execute_from_unadvertised_calls() {
    let executions = Arc::new(AtomicUsize::new(0));
    let write_handler = counting_write_tool(Arc::clone(&executions));
    let mut builder = ToolRegistryBuilder::new();
    let ipython = IpythonHandler::new();
    builder.push_spec_with_exposure(ipython.spec().clone(), ToolExposure::Direct);
    builder.register_handler("ipython", Arc::new(ipython));
    builder.push_spec(write_handler.spec().clone());
    builder.register_handler("write", Arc::new(write_handler));
    builder.restrict_model_exposure_to(&["ipython"]);
    let registry = Arc::new(builder.build());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::capturing_script(
        "capturing-provider",
        Arc::clone(&requests),
        StreamScript::UnadvertisedWrite,
    ));
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("write a file"));

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider,
        Arc::clone(&registry),
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should continue after the unadvertised tool is rejected");

    assert_eq!(executions.load(Ordering::SeqCst), 0);
    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    for request in captured.iter() {
        assert_eq!(
            request
                .tools
                .as_ref()
                .expect("ipython schema")
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["ipython"]
        );
    }
}

#[test]
fn subagent_reminder_insertion_preserves_tool_result_adjacency() {
    let mut messages = vec![
        RequestMessage {
            role: Role::User.as_str().to_string(),
            content: vec![RequestContent::Text {
                text: "child task input".to_string(),
            }],
        },
        RequestMessage {
            role: Role::Assistant.as_str().to_string(),
            content: vec![RequestContent::ToolUse {
                id: "tool-1".to_string(),
                name: "read".to_string(),
                input: json!({}),
            }],
        },
        RequestMessage {
            role: Role::User.as_str().to_string(),
            content: vec![RequestContent::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "tool output".to_string(),
                is_error: None,
            }],
        },
    ];

    insert_subagent_request_reminders(&mut messages);

    assert!(message_contains(
        &messages[0],
        "You are running as a sub-agent"
    ));
    assert!(message_contains(&messages[1], "child task input"));
    assert!(
        matches!(messages[2].content.as_slice(), [RequestContent::ToolUse { id, .. }] if id == "tool-1")
    );
    assert!(
        matches!(messages[3].content.as_slice(), [RequestContent::ToolResult { tool_use_id, .. }] if tool_use_id == "tool-1")
    );
}

fn request_message_index_containing(request: &ModelRequest, needle: &str) -> usize {
    request
        .messages
        .iter()
        .position(|message| message_contains(message, needle))
        .unwrap_or_else(|| panic!("expected request message containing {needle:?}: {request:?}"))
}

fn message_contains(message: &RequestMessage, needle: &str) -> bool {
    message
        .content
        .iter()
        .any(|content| matches!(content, RequestContent::Text { text } if text.contains(needle)))
}

fn active_goal(objective: &str) -> ThreadGoal {
    ThreadGoal {
        thread_id: devo_protocol::SessionId::new(),
        objective: objective.to_string(),
        status: ThreadGoalStatus::Active,
        token_budget: Some(10_000),
        tokens_used: 250,
        time_used_seconds: 0,
        created_at: 1,
        updated_at: 1,
    }
}

#[tokio::test]
async fn query_uses_session_permission_mode_for_mutating_tools() {
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler("mutating_tool", Arc::new(mutating_tool()));
    builder.push_spec(named_spec(
        "mutating_tool",
        "A test-only mutating tool.",
        ToolExecutionMode::Mutating,
        false,
    ));
    let registry = Arc::new(builder.build());
    let deny_checker = PermissionChecker::new(|request| {
        let n = request.tool_name;
        Box::pin(async move { Err(format!("{n} denied")) })
    });
    let runtime = ToolRuntime::new(Arc::clone(&registry), deny_checker);

    let mut session = SessionState::new(
        SessionConfig {
            permission_mode: PermissionMode::Deny,
            ..Default::default()
        },
        std::env::temp_dir(),
    );
    session.push_message(Message::user("run the tool"));

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        Arc::new(ScriptedProvider::scripted(
            "test-provider",
            StreamScript::SingleMutating,
        )),
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should complete and append a tool_result");

    let tool_result_message = session
        .messages
        .iter()
        .find(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        })
        .expect("tool_result message should be appended");
    let ContentBlock::ToolResult {
        tool_use_id,
        content,
        is_error,
    } = &tool_result_message.content[0]
    else {
        panic!("expected tool_result content block");
    };

    assert_eq!(tool_use_id, "tool-1");
    assert!(
        *is_error,
        "denied permission should surface as a tool error"
    );
    assert!(
        content.contains("permission denied"),
        "expected tool_result to mention permission denial, got: {content}"
    );
}

#[tokio::test]
async fn query_request_model_routing() {
    for (turn_config, expected_model, expected_slug) in [
        (
            TurnConfig::with_request_model(
                Model {
                    slug: "kimi-k2.5".into(),
                    reasoning_capability: ReasoningCapability::Toggle,
                    default_reasoning_effort: Some(ReasoningEffort::Medium),
                    reasoning_implementation: Some(ReasoningImplementation::ModelVariant(
                        ReasoningVariantConfig {
                            variants: vec![
                                ReasoningVariant {
                                    selection_value: "disabled".into(),
                                    model: "kimi-k2.5".into(),
                                    reasoning_effort: None,
                                    label: "Off".into(),
                                    description: "Use the standard model".into(),
                                    extra_body: None,
                                },
                                ReasoningVariant {
                                    selection_value: "enabled".into(),
                                    model: "kimi-k2.5-thinking".into(),
                                    reasoning_effort: Some(ReasoningEffort::Medium),
                                    label: "On".into(),
                                    description: "Use the reasoning model".into(),
                                    extra_body: None,
                                },
                            ],
                        },
                    )),
                    truncation_policy: TruncationPolicyConfig {
                        mode: TruncationMode::Tokens,
                        limit: 10_000,
                    },
                    ..Model::default()
                },
                "vendor/kimi-k2.5".into(),
                HashMap::from([(
                    "kimi-k2.5-thinking".into(),
                    "vendor/kimi-k2.5-thinking".into(),
                )])
                .into(),
                Some("enabled".into()),
            ),
            "vendor/kimi-k2.5-thinking",
            None,
        ),
        (
            TurnConfig::with_request_model(
                Model {
                    slug: "catalog-slug".into(),
                    display_name: "Catalog Model".into(),
                    base_instructions: "catalog instructions".into(),
                    ..Model::default()
                },
                "vendor/model-name".into(),
                HashMap::new().into(),
                None,
            ),
            "vendor/model-name",
            Some("catalog-slug"),
        ),
    ] {
        let fixtures = CapturingFixtures::new("capturing-provider");
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));
        fixtures
            .query(&mut session, &turn_config, QueryOptions::default(), None)
            .await
            .expect("query should succeed");
        let captured = fixtures.requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].model, expected_model);
        if let Some(slug) = expected_slug {
            assert_eq!(
                session
                    .session_context
                    .as_ref()
                    .expect("session context")
                    .model
                    .slug,
                slug
            );
        } else {
            assert_eq!(captured[0].request_thinking, None);
        }
    }
}

/// Trace: L2-DES-CONTEXT-001
#[tokio::test]
async fn query_collaboration_mode_prompts_follow_plan_to_build_transition() {
    let fixtures = CapturingFixtures::new("capturing-provider");
    let model = Model {
        slug: "model-a".into(),
        base_instructions: "base instructions".into(),
        ..Model::default()
    };
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.collaboration_mode = CollaborationMode::Plan;
    session.push_message(Message::user("plan this"));

    fixtures
        .query(
            &mut session,
            &TurnConfig::new(model.clone(), None),
            QueryOptions::default(),
            None,
        )
        .await
        .expect("plan query should succeed");

    {
        let captured = fixtures.requests.lock().expect("lock requests");
        let system = captured[0].system.as_deref().expect("system prompt");
        let mode_prompt =
            crate::collaboration_mode_prompts::mode_introductions_prompt(CollaborationMode::Plan);
        assert!(system.contains("base instructions"));
        assert!(!system.contains("<collaboration_mode_build>"));
        assert!(system.contains(&mode_prompt));
        let mode_index = request_message_index_containing(&captured[0], "<collaboration_mode>");
        assert!(message_contains(
            &captured[0].messages[mode_index],
            "<current>plan</current>"
        ));
    }

    session.collaboration_mode = CollaborationMode::Build;
    session.push_message(Message::user("implement this"));
    fixtures
        .query(
            &mut session,
            &TurnConfig::new(model, None),
            QueryOptions::default(),
            None,
        )
        .await
        .expect("build query should succeed");

    let captured = fixtures.requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    assert_ne!(captured[0].system, captured[1].system);
    let build_system = captured[1].system.as_deref().expect("build system prompt");
    let build_mode_prompt =
        crate::collaboration_mode_prompts::mode_introductions_prompt(CollaborationMode::Build);
    assert!(build_system.contains(&build_mode_prompt));
    assert!(!build_system.contains("<collaboration_mode_plan>"));
    let mode_change_index =
        request_message_index_containing(&captured[1], "<transition>plan -> build</transition>");
    let request_index = request_message_index_containing(&captured[1], "implement this");
    assert!(mode_change_index < request_index);
    assert!(message_contains(
        &captured[1].messages[mode_change_index],
        "<previous>plan</previous>"
    ));
    assert!(message_contains(
        &captured[1].messages[mode_change_index],
        "<current>build</current>"
    ));
}

#[tokio::test]
async fn query_inserts_goal_context_relative_to_latest_request() {
    for (objective, user_messages, goal_after_assistant) in [
        ("ship /goal", vec!["finish implementation"], false),
        (
            "continue the active goal",
            vec!["older user prompt", "older assistant reply"],
            true,
        ),
    ] {
        let fixtures = CapturingFixtures::new("capturing-provider");
        let model = Model {
            slug: "model-a".into(),
            base_instructions: "base instructions".into(),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.set_active_goal(active_goal(objective));
        for message in user_messages {
            if message == "older assistant reply" {
                session.push_message(Message::assistant_text(message));
            } else {
                session.push_message(Message::user(message));
            }
        }

        fixtures
            .query(
                &mut session,
                &TurnConfig::new(model, None),
                QueryOptions::default(),
                None,
            )
            .await
            .expect("query should succeed");

        let captured = fixtures.requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        assert!(
            !captured[0]
                .system
                .as_deref()
                .unwrap_or_default()
                .contains(objective)
        );
        let messages = &captured[0].messages;
        let goal_index = messages
            .iter()
            .position(|message| message_contains(message, objective))
            .expect("goal context message");
        if goal_after_assistant {
            let assistant_index = messages
                .iter()
                .position(|message| message_contains(message, "older assistant reply"))
                .expect("assistant history message");
            assert!(goal_index > assistant_index);
            assert_eq!(goal_index, messages.len() - 1);
        } else {
            let request_index = messages
                .iter()
                .position(|message| message_contains(message, "finish implementation"))
                .expect("latest user request message");
            assert!(goal_index < request_index);
        }
    }
}

#[tokio::test]
async fn query_locks_system_prompt_and_environment_prefix_per_session() {
    let fixtures = CapturingFixtures::new("capturing-provider");
    let temp_root = std::env::temp_dir().join(format!("devo-query-lock-{}", uuid::Uuid::new_v4()));
    let second_cwd = temp_root.join("nested");
    let first_model = Model {
        slug: "model-a".into(),
        base_instructions: "base-a".into(),
        ..Model::default()
    };
    let second_model = Model {
        slug: "model-b".into(),
        base_instructions: "base-b".into(),
        ..Model::default()
    };

    let mut session = SessionState::new(SessionConfig::default(), temp_root.clone());
    session.push_message(Message::user("hello"));

    fixtures
        .query(
            &mut session,
            &TurnConfig::new(first_model, None),
            QueryOptions::default(),
            None,
        )
        .await
        .expect("first query should succeed");

    session.cwd = second_cwd;
    session.push_message(Message::user("follow up"));

    fixtures
        .query(
            &mut session,
            &TurnConfig::new(second_model, Some("enabled".into())),
            QueryOptions::default(),
            None,
        )
        .await
        .expect("second query should succeed");

    let captured = fixtures.requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    // The system prompt uses the session-locked RLM doctrine and catalog
    // instructions. Both turns stay in Build mode; the second changes model and
    // cwd, so equality proves those changes do not leak into the locked prompt.
    let system = captured[0].system.as_deref().unwrap_or_default();
    assert!(system.starts_with("You are Devo, a coding agent"));
    assert!(system.contains("\n# Additional Guidance\n\nbase-a"));
    assert!(!system.contains("base-b"));
    assert!(
        system.contains(
            crate::collaboration_mode_prompts::mode_introductions_prompt(CollaborationMode::Build)
                .trim()
        )
    );
    assert_eq!(captured[0].system, captured[1].system);
    assert_eq!(captured[0].messages[0].role, captured[1].messages[0].role);
    assert_eq!(
        serde_json::to_value(&captured[0].messages[0].content).expect("serialize first content"),
        serde_json::to_value(&captured[1].messages[0].content).expect("serialize second content")
    );
}

#[tokio::test]
async fn query_publishes_last_model_request_for_prefix_reuse() {
    let fixtures = CapturingFixtures::new("capturing-provider");
    let (mut session, _, _) = empty_query_env("hello");
    let last_model_request: SharedLastModelRequest = Arc::new(Mutex::new(None));

    fixtures
        .query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            QueryOptions {
                last_model_request: Some(Arc::clone(&last_model_request)),
                ..QueryOptions::default()
            },
            None,
        )
        .await
        .expect("query should succeed");

    let captured = fixtures.requests.lock().expect("lock requests");
    let published = last_model_request
        .lock()
        .expect("lock last request")
        .clone()
        .expect("query should publish the assembled request");
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].system, published.system);
    assert_eq!(captured[0].model, published.model);
    assert_eq!(captured[0].messages.len(), published.messages.len());
    assert_eq!(
        captured[0].tools.as_ref().map(Vec::len),
        published.tools.as_ref().map(Vec::len)
    );
}

#[tokio::test]
async fn query_context_diff_follows_turn_metadata_changes() {
    for (second_reasoning, expect_diff) in [(Some("enabled".into()), true), (None, false)] {
        let fixtures = CapturingFixtures::new("capturing-provider");
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        let first_model = Model {
            slug: "model-a".into(),
            ..Model::default()
        };
        let second_model = Model {
            slug: if expect_diff {
                "model-b".into()
            } else {
                "model-a".into()
            },
            ..Model::default()
        };

        session.push_message(Message::user("hello"));
        fixtures
            .query(
                &mut session,
                &TurnConfig::new(first_model, None),
                QueryOptions::default(),
                None,
            )
            .await
            .expect("first query should succeed");

        session.push_message(Message::user("follow up"));
        fixtures
            .query(
                &mut session,
                &TurnConfig::new(second_model, second_reasoning),
                QueryOptions::default(),
                None,
            )
            .await
            .expect("second query should succeed");

        if expect_diff {
            let diff_message = &session.messages[session.messages.len() - 3];
            let user_message = &session.messages[session.messages.len() - 2];
            assert_eq!(user_message, &Message::user("follow up"));
            let ContentBlock::Text { text } = &diff_message.content[0] else {
                panic!("expected text diff message");
            };
            assert!(text.contains("<context_changes>"));
            assert!(text.contains("<name>model</name>"));
            assert!(text.contains("<previous>model-a</previous>"));
            assert!(text.contains("<current>model-b</current>"));
        } else {
            let captured = fixtures.requests.lock().expect("lock requests");
            let follow_up_index = request_message_index_containing(&captured[1], "follow up");
            assert!(follow_up_index > 0);
            assert!(!message_contains(
                &captured[1].messages[follow_up_index - 1],
                "<context_changes>"
            ));
        }
    }
}

#[tokio::test]
async fn query_inserts_interrupted_notice_before_next_user_message() {
    let fixtures = CapturingFixtures::new("capturing-provider");
    let (mut session, _, _) = empty_query_env("hello");
    session.push_message(Message::assistant_text("partial"));
    session.mark_last_turn_interrupted();
    session.push_message(Message::user("continue please"));

    fixtures
        .query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            QueryOptions::default(),
            None,
        )
        .await
        .expect("query should succeed");

    let abort_index = session
        .messages
        .iter()
        .position(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::Text { text } if text.contains("<turn_aborted>")
                )
            })
        })
        .expect("interrupted notice should be inserted");
    let continue_index = session
        .messages
        .iter()
        .position(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::Text { text } if text.contains("continue please")
                )
            })
        })
        .expect("user message should remain");
    assert!(abort_index < continue_index);
    assert!(!session.last_turn_interrupted);
}

#[tokio::test]
async fn query_pairs_interrupted_tool_result_when_cancel_fires_during_tool() {
    struct HangingMutatingTool {
        spec: &'static ToolSpec,
    }

    #[async_trait]
    impl ToolHandler for HangingMutatingTool {
        fn spec(&self) -> &ToolSpec {
            self.spec
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            _input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            std::future::pending::<()>().await;
            unreachable!("tool should be cancelled")
        }
    }

    let spec = named_spec(
        "mutating_tool",
        "hangs until cancelled",
        ToolExecutionMode::Mutating,
        false,
    );
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler(
        "mutating_tool",
        Arc::new(HangingMutatingTool {
            spec: Box::leak(Box::new(spec.clone())),
        }),
    );
    builder.push_spec(spec);
    let registry = Arc::new(builder.build());
    let cancel_token = CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let runtime = ToolRuntime::new_with_context_and_options(
        Arc::clone(&registry),
        PermissionChecker::always_allow(),
        ToolRuntimeContext::default(),
        ToolExecutionOptions {
            cancel_token: cancel_token.clone(),
            ..ToolExecutionOptions::default()
        },
    );
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("run the tool"));
    let turn_config = TurnConfig::new(Model::default(), None);
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(ScriptedProvider::scripted(
        "test-provider",
        StreamScript::SingleMutating,
    ));

    let result = {
        let mut query_future = std::pin::pin!(query(
            &mut session,
            &turn_config,
            provider,
            registry,
            &runtime,
            None,
            QueryOptions {
                cancel_token: Some(cancel_token),
                ..QueryOptions::default()
            },
        ));
        tokio::select! {
            result = &mut query_future => {
                panic!("query completed before cancel: {result:?}");
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                cancel_for_task.cancel();
            }
        }
        query_future.await
    };
    assert!(matches!(result, Err(AgentError::Aborted)));
    let (interrupted_content, is_error) = session
        .messages
        .iter()
        .find_map(|message| {
            message.content.iter().find_map(|block| match block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } if tool_use_id == "tool-1" => Some((content.clone(), *is_error)),
                _ => None,
            })
        })
        .expect("interrupted tool result should exist");
    // The cancel message carries how long the call ran (time-awareness), so
    // match the marker as a prefix rather than the exact static string.
    assert!(interrupted_content.starts_with(crate::tools::INTERRUPTED_TOOL_RESULT_MESSAGE));
    assert!(interrupted_content.contains("ms"));
    assert!(is_error);
}

#[tokio::test]
async fn query_drops_orphaned_tool_calls_from_prompt_history() {
    let fixtures = CapturingFixtures::new("capturing-provider");
    let (mut session, _, _) = empty_query_env("first");
    session.push_message(Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Text {
                text: "Calling tool".into(),
            },
            ContentBlock::ToolUse {
                id: "call-1".into(),
                name: "bash".into(),
                input: json!({ "cmd": "pwd" }),
            },
        ],
    });
    session.push_message(Message::user("follow up"));

    fixtures
        .query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            QueryOptions::default(),
            None,
        )
        .await
        .expect("query should succeed");

    let captured = fixtures.requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 1);
    assert!(
        captured[0]
            .messages
            .iter()
            .flat_map(|message| message.content.iter())
            .all(|content| !matches!(content, devo_protocol::RequestContent::ToolUse { .. })),
        "expected orphaned tool calls to be removed from prompt history"
    );
}

#[tokio::test]
async fn test_model_connection_sends_minimal_request() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = ScriptedProvider::capturing("capturing-provider", Arc::clone(&requests));
    let model = Model {
        slug: "glm-4.5".into(),
        reasoning_capability: devo_protocol::ReasoningCapability::Toggle,
        top_p: Some(0.95),
        ..Model::default()
    };
    let preview = test_model_connection(
        &provider,
        &model,
        devo_protocol::ModelProfileKey::CatalogSlug(model.slug.clone()),
        "renamed-provider-model",
        "Reply with OK only.",
    )
    .await
    .expect("probe request should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(preview, "done");
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].model_slug,
        devo_protocol::ModelProfileKey::CatalogSlug("glm-4.5".to_string())
    );
    assert_eq!(captured[0].model, "renamed-provider-model");
    assert_eq!(captured[0].request_thinking.as_deref(), Some("enabled"));
    assert_eq!(captured[0].system, None);
    assert!(captured[0].tools.is_none());
    assert_eq!(captured[0].messages.len(), 1);
    assert_eq!(captured[0].sampling.top_p, Some(0.95));
}

#[tokio::test]
async fn query_persists_streamed_reasoning_for_follow_up_request() {
    let reasoning_response = ScriptStreamResponse {
        preamble: vec![
            StreamEvent::ReasoningStart { index: 0 },
            StreamEvent::ReasoningDelta {
                index: 0,
                text: "plan".into(),
            },
            StreamEvent::TextStart { index: 1 },
            StreamEvent::TextDelta {
                index: 1,
                text: "final".into(),
            },
        ],
        content: vec![ResponseContent::Text("final".into())],
        metadata: ResponseMetadata {
            extras: vec![ResponseExtra::ReasoningText {
                text: "plan".into(),
            }],
        },
    };
    let provider =
        ScriptStreamProvider::new("reasoning-provider", vec![reasoning_response.clone(); 2]);
    let requests = Arc::clone(&provider.requests);
    let (mut session, registry, runtime) = empty_query_env("hello");
    let seen_events = Arc::new(Mutex::new(Vec::new()));

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider.clone(),
        Arc::clone(&registry),
        &runtime,
        Some(recording_callback(&seen_events)),
        QueryOptions::default(),
    )
    .await
    .expect("first query should succeed");

    assert!(
        seen_events
            .lock()
            .expect("lock events")
            .iter()
            .any(|event| { matches!(event, QueryEvent::ReasoningDelta(text) if text == "plan") })
    );
    assert_eq!(
        session
            .messages
            .iter()
            .find(|message| matches!(message.role, Role::Assistant))
            .expect("assistant message"),
        &Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Reasoning {
                    text: "plan".into(),
                },
                ContentBlock::Text {
                    text: "final".into(),
                },
            ],
        }
    );

    session.push_message(Message::user("follow up"));
    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider,
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("second query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    let replayed_assistant = captured[1]
        .messages
        .iter()
        .find(|message| message.role == "assistant")
        .expect("assistant replay");
    assert_eq!(
        serde_json::to_value(replayed_assistant).expect("serialize assistant replay"),
        json!({
            "role": "assistant",
            "content": [
                { "type": "reasoning", "text": "plan" },
                { "type": "text", "text": "final" }
            ]
        })
    );
}

#[tokio::test]
async fn query_round_trips_provider_reasoning_without_plain_reasoning() {
    let signed_payload = json!({
        "type": "thinking",
        "thinking": "signed plan",
        "signature": "sig_123"
    });
    let provider = ScriptStreamProvider::new(
        "signed-reasoning-provider",
        vec![
            ScriptStreamResponse {
                preamble: vec![],
                content: vec![
                    ResponseContent::ProviderReasoning {
                        provider: "anthropic".into(),
                        payload: signed_payload.clone(),
                    },
                    ResponseContent::Text("first".into()),
                ],
                metadata: ResponseMetadata::default(),
            },
            ScriptStreamResponse {
                preamble: vec![],
                content: vec![ResponseContent::Text("second".into())],
                metadata: ResponseMetadata::default(),
            },
        ],
    );
    let requests = Arc::clone(&provider.requests);
    let (mut session, registry, runtime) = empty_query_env("hello");
    let seen_events = Arc::new(Mutex::new(Vec::new()));

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider.clone(),
        Arc::clone(&registry),
        &runtime,
        Some(recording_callback(&seen_events)),
        QueryOptions::default(),
    )
    .await
    .expect("first query should succeed");

    {
        let events = seen_events.lock().expect("lock events");
        assert!(events.iter().any(|event| {
            matches!(event, QueryEvent::ReasoningDelta(text) if text == "signed plan")
        }));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, QueryEvent::ReasoningCompleted))
        );
        assert_eq!(
            session
                .messages
                .iter()
                .find(|message| matches!(message.role, Role::Assistant))
                .expect("assistant message"),
            &Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ProviderReasoning {
                        provider: "anthropic".into(),
                        payload: signed_payload,
                    },
                    ContentBlock::Text {
                        text: "first".into(),
                    },
                ],
            }
        );
    }

    session.push_message(Message::user("follow up"));
    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider,
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("second query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    let second_request_content = captured[1]
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .collect::<Vec<_>>();
    assert!(second_request_content.iter().any(|content| matches!(
        content,
        RequestContent::ProviderReasoning { provider, payload }
        if provider == "anthropic"
            && payload["thinking"] == json!("signed plan")
            && payload["signature"] == json!("sig_123")
    )));
    assert!(
        second_request_content
            .iter()
            .all(|content| !matches!(content, RequestContent::Reasoning { .. }))
    );
}

#[tokio::test]
async fn query_continues_deepseek_v4_thinking_only_end_turn_once() {
    let thinking_payload = json!({
        "type": "thinking",
        "thinking": "internal plan",
        "signature": "sig_plan"
    });
    let provider = ScriptStreamProvider::new(
        "thinking-only-then-text-provider",
        vec![
            ScriptStreamResponse {
                preamble: vec![],
                content: vec![ResponseContent::ProviderReasoning {
                    provider: "anthropic".into(),
                    payload: thinking_payload.clone(),
                }],
                metadata: ResponseMetadata::default(),
            },
            ScriptStreamResponse {
                preamble: vec![StreamEvent::TextDelta {
                    index: 0,
                    text: "visible answer".into(),
                }],
                content: vec![ResponseContent::Text("visible answer".into())],
                metadata: ResponseMetadata::default(),
            },
        ],
    );
    let requests = Arc::clone(&provider.requests);
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let model = Model {
        slug: "deepseek-v4-pro".into(),
        provider: devo_protocol::ProviderWireApi::AnthropicMessages,
        ..Model::default()
    };
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user("hello"));
    let seen_events = Arc::new(Mutex::new(Vec::new()));

    query(
        &mut session,
        &TurnConfig::new(model, None),
        provider,
        registry,
        &runtime,
        Some(recording_callback(&seen_events)),
        QueryOptions::default(),
    )
    .await
    .expect("query should continue once and finish with text");

    assert_eq!(
        session.messages[session.messages.len() - 4..],
        [
            Message::user("hello"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ProviderReasoning {
                    provider: "anthropic".into(),
                    payload: thinking_payload,
                }],
            },
            Message::user(super::DEEPSEEK_THINKING_ONLY_CONTINUATION_PROMPT),
            Message::assistant_text("visible answer"),
        ]
    );

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    let second_request_messages = &captured[1].messages;
    let second_request_tail = &second_request_messages[second_request_messages.len() - 3..];
    assert_eq!(
        serde_json::to_value(second_request_tail).expect("serialize second request messages"),
        json!([
            {
                "role": "user",
                "content": [{ "type": "text", "text": "hello" }]
            },
            {
                "role": "assistant",
                "content": [{
                    "type": "provider_reasoning",
                    "provider": "anthropic",
                    "payload": {
                        "type": "thinking",
                        "thinking": "internal plan",
                        "signature": "sig_plan"
                    }
                }]
            },
            {
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": super::DEEPSEEK_THINKING_ONLY_CONTINUATION_PROMPT
                }]
            }
        ])
    );

    let events = seen_events.lock().expect("lock events");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, QueryEvent::TurnComplete { .. }))
            .count(),
        1
    );
    assert!(
        events.iter().any(|event| {
            matches!(event, QueryEvent::TextDelta(text) if text == "visible answer")
        })
    );
}

#[tokio::test]
async fn query_preserves_provider_reasoning_and_hosted_tool_order() {
    let before_tool = json!({
        "type": "thinking",
        "thinking": "before tool",
        "signature": "sig_before"
    });
    let after_tool = json!({
        "type": "thinking",
        "thinking": "after tool",
        "signature": "sig_after"
    });
    let hosted_input = json!({"query": "desktop gui 2026"});
    let response_content = vec![
        ResponseContent::ProviderReasoning {
            provider: "anthropic".into(),
            payload: before_tool.clone(),
        },
        ResponseContent::HostedToolUse {
            id: "srvtool_1".into(),
            name: "web_search".into(),
            input: hosted_input.clone(),
            output: None,
            status: None,
        },
        ResponseContent::HostedToolUse {
            id: "srvtool_1".into(),
            name: "web_search".into(),
            input: json!({}),
            output: Some(json!([{"title": "result"}])),
            status: Some("completed".into()),
        },
        ResponseContent::ProviderReasoning {
            provider: "anthropic".into(),
            payload: after_tool.clone(),
        },
        ResponseContent::Text("final".into()),
    ];
    let ordered_content = vec![
        ContentBlock::ProviderReasoning {
            provider: "anthropic".into(),
            payload: before_tool,
        },
        ContentBlock::HostedToolUse {
            id: "srvtool_1".into(),
            name: "web_search".into(),
            input: hosted_input.clone(),
            output: None,
            status: None,
        },
        ContentBlock::HostedToolUse {
            id: "srvtool_1".into(),
            name: "web_search".into(),
            input: hosted_input,
            output: Some(json!([{"title": "result"}])),
            status: Some("completed".into()),
        },
        ContentBlock::ProviderReasoning {
            provider: "anthropic".into(),
            payload: after_tool,
        },
        ContentBlock::Text {
            text: "final".into(),
        },
    ];
    let provider = ScriptStreamProvider::new(
        "ordered-hosted-provider",
        vec![
            ScriptStreamResponse {
                preamble: vec![],
                content: response_content,
                metadata: ResponseMetadata::default(),
            },
            ScriptStreamResponse {
                preamble: vec![],
                content: vec![ResponseContent::Text("second".into())],
                metadata: ResponseMetadata::default(),
            },
        ],
    );
    let requests = Arc::clone(&provider.requests);
    let (mut session, registry, runtime) = empty_query_env("hello");

    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider.clone(),
        Arc::clone(&registry),
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("first query should succeed");

    assert_eq!(
        session
            .messages
            .iter()
            .find(|message| matches!(message.role, Role::Assistant))
            .expect("assistant message")
            .content,
        ordered_content
    );

    session.push_message(Message::user("follow up"));
    query(
        &mut session,
        &TurnConfig::new(Model::default(), None),
        provider,
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("second query should succeed");

    let captured = requests.lock().expect("lock requests");
    let replayed_content = captured[1]
        .messages
        .iter()
        .find(|message| message.role == "assistant")
        .expect("assistant replay")
        .content
        .clone();
    assert_eq!(
        serde_json::to_value(&replayed_content).expect("serialize replayed content"),
        json!([
            {
                "type": "provider_reasoning",
                "provider": "anthropic",
                "payload": {
                    "type": "thinking",
                    "thinking": "before tool",
                    "signature": "sig_before"
                }
            },
            {
                "type": "hosted_tool_use",
                "id": "srvtool_1",
                "name": "web_search",
                "input": { "query": "desktop gui 2026" }
            },
            {
                "type": "hosted_tool_use",
                "id": "srvtool_1",
                "name": "web_search",
                "input": { "query": "desktop gui 2026" },
                "output": [{ "title": "result" }],
                "status": "completed"
            },
            {
                "type": "provider_reasoning",
                "provider": "anthropic",
                "payload": {
                    "type": "thinking",
                    "thinking": "after tool",
                    "signature": "sig_after"
                }
            },
            {
                "type": "text",
                "text": "final"
            }
        ])
    );
}

#[tokio::test]
async fn query_disables_openai_thinking_when_reasoning_context_is_missing() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ModelProviderSDK> =
        Arc::new(ScriptedProvider::capturing("openai", Arc::clone(&requests)));
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let model = Model {
        slug: "deepseek-v4-flash".into(),
        provider: devo_protocol::ProviderWireApi::OpenAIChatCompletions,
        reasoning_capability: ReasoningCapability::Toggle,
        base_instructions: String::new(),
        ..Model::default()
    };
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::assistant_text("legacy assistant reply"));
    session.push_message(Message::user("follow up"));

    query(
        &mut session,
        &TurnConfig::new(model, Some("enabled".into())),
        Arc::clone(&provider),
        registry,
        &runtime,
        None,
        QueryOptions::default(),
    )
    .await
    .expect("query should succeed");

    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].model_slug,
        devo_protocol::ModelProfileKey::CatalogSlug("deepseek-v4-flash".to_string())
    );
    assert_eq!(captured[0].request_thinking.as_deref(), Some("enabled"));
    // Toggle capability does not set reasoning_effort on the request.
    assert_eq!(captured[0].reasoning_effort, None);
}

#[tokio::test]
async fn query_tool_result_summary_is_set() {
    let mut fixtures = ScriptToolFixtures::new(
        Arc::new(mutating_tool()),
        named_spec("mutating_tool", "", ToolExecutionMode::Mutating, false),
        "run the tool",
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let seen_clone = Arc::clone(&seen_clone);
        Box::pin(async move {
            if let QueryEvent::ToolResult { summary, .. } = event {
                seen_clone.lock().unwrap().push(summary);
            }
        })
    });

    fixtures
        .query(
            Arc::new(ScriptedProvider::scripted(
                "test-provider",
                StreamScript::SingleMutating,
            )),
            Some(callback),
            TurnConfig::new(Model::default(), None),
            QueryOptions::default(),
        )
        .await
        .expect("query should complete");

    let summaries = seen.lock().unwrap();
    assert!(
        !summaries.is_empty(),
        "should have at least one ToolResult summary"
    );
    assert!(summaries.iter().all(|summary| !summary.is_empty()));
}

#[tokio::test]
async fn query_tool_events_include_resolved_input() {
    #[derive(Clone, Copy)]
    enum ToolEventKind {
        ResultByName,
        ResultById,
        Start,
    }

    for (script, user_message, kind, expected) in [
        (
            StreamScript::SingleMutating,
            "run the tool",
            ToolEventKind::ResultByName,
            vec![(String::from("mutating_tool"), json!({ "value": 1 }))],
        ),
        (
            StreamScript::InterleavedMutating,
            "run the tools",
            ToolEventKind::ResultById,
            vec![
                (String::from("tool-1"), json!({ "value": 1 })),
                (String::from("tool-2"), json!({ "value": 2 })),
            ],
        ),
        (
            StreamScript::InterleavedMutating,
            "run the tools",
            ToolEventKind::Start,
            vec![
                (String::from("tool-1"), json!({ "value": 1 })),
                (String::from("tool-2"), json!({ "value": 2 })),
            ],
        ),
    ] {
        let mut fixtures = ScriptToolFixtures::new(
            Arc::new(display_content_tool()),
            named_spec("mutating_tool", "", ToolExecutionMode::ReadOnly, false),
            user_message,
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let event_kind = kind;
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let seen_clone = Arc::clone(&seen_clone);
            Box::pin(async move {
                match (event_kind, event) {
                    (
                        ToolEventKind::ResultByName,
                        QueryEvent::ToolResult {
                            tool_name, input, ..
                        },
                    ) => {
                        seen_clone.lock().unwrap().push((tool_name, input));
                    }
                    (
                        ToolEventKind::ResultById,
                        QueryEvent::ToolResult {
                            tool_use_id, input, ..
                        },
                    ) => {
                        seen_clone.lock().unwrap().push((tool_use_id, input));
                    }
                    (ToolEventKind::Start, QueryEvent::ToolUseStart { id, input, .. })
                        if !input.as_object().is_some_and(|object| object.is_empty()) =>
                    {
                        seen_clone.lock().unwrap().push((id, input));
                    }
                    _ => {}
                }
            })
        });

        fixtures
            .query(
                Arc::new(ScriptedProvider::scripted("test-provider", script)),
                Some(callback),
                TurnConfig::new(Model::default(), None),
                QueryOptions::default(),
            )
            .await
            .expect("query should complete");

        assert_eq!(*seen.lock().unwrap(), expected);
    }
}

#[tokio::test]
async fn query_truncates_model_visible_tool_results_but_emits_raw_tool_result_events() {
    let full_content = "abcdefghijklmnopqrstuvwxyz".to_string();
    let display_content = "raw display abcdefghijklmnopqrstuvwxyz".to_string();
    let mut fixtures = ScriptToolFixtures::new(
        Arc::new(LargeToolResultTool::new(
            full_content.clone(),
            Some(display_content.clone()),
        )),
        named_spec("mutating_tool", "", ToolExecutionMode::ReadOnly, false),
        "run the tool",
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let callback = recording_callback(&seen);

    query(
        &mut fixtures.session,
        &TurnConfig::new(
            Model {
                truncation_policy: TruncationPolicyConfig::bytes(20),
                ..Model::default()
            },
            None,
        ),
        Arc::new(ScriptedProvider::capturing_script(
            "capturing-tool-use-provider",
            Arc::clone(&requests),
            StreamScript::CapturingMutating,
        )),
        fixtures.registry.clone(),
        &fixtures.runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await
    .expect("query should complete");

    assert_eq!(
        seen.lock()
            .expect("lock seen events")
            .iter()
            .filter_map(|event| match event {
                QueryEvent::ToolResult {
                    content,
                    display_content,
                    ..
                } => Some((content.clone().into_string(), display_content.clone())),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![(full_content, Some(display_content))]
    );
    let captured = requests.lock().expect("lock requests");
    assert_eq!(captured.len(), 2);
    let model_visible_tool_result = captured[1]
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|content| match content {
            RequestContent::ToolResult { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .expect("continuation request should include tool result");
    assert_eq!(model_visible_tool_result, "abcde\n...[truncated]");
}

#[tokio::test]
#[ignore = "legacy progress mechanism replaced by L3 contracts"]
async fn query_emits_legacy_tool_display_and_progress_events() {
    for (handler, expect_display, expect_progress) in [
        (
            Arc::new(display_content_tool()) as Arc<dyn ToolHandler>,
            true,
            false,
        ),
        (Arc::new(streaming_mutating_tool()), false, true),
    ] {
        let mut fixtures = ScriptToolFixtures::new(
            handler,
            named_spec("mutating_tool", "", ToolExecutionMode::Mutating, false),
            "run the tool",
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let callback = recording_callback(&seen);

        fixtures
            .query(
                Arc::new(ScriptedProvider::scripted(
                    "test-provider",
                    StreamScript::SingleMutating,
                )),
                Some(callback),
                TurnConfig::new(Model::default(), None),
                QueryOptions::default(),
            )
            .await
            .expect("query should complete");

        let events = seen.lock().unwrap();
        if expect_display {
            assert_eq!(events.len(), 1);
            assert!(matches!(
                &events[0],
                QueryEvent::ToolResult {
                    content: crate::tools::ToolContent::Text(text),
                    display_content: Some(display),
                    ..
                } if text == "canonical" && display == "display"
            ));
        }
        if expect_progress {
            let progress_index = events
                .iter()
                .position(|event| {
                    matches!(
                        event,
                        QueryEvent::ToolProgress {
                            tool_use_id,
                            progress: crate::tools::ToolProgress::OutputDelta { delta },
                        } if tool_use_id == "tool-1" && delta == "stream chunk\n"
                    )
                })
                .expect("tool progress event should be emitted");
            let result_index = events.iter().position(|event| {
                matches!(
                    event,
                    QueryEvent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        ..
                    } if tool_use_id == "tool-1"
                        && matches!(content, crate::tools::ToolContent::Text(text) if text == "stream complete")
                        && !is_error
                )
            }).expect("tool result event should be emitted");
            assert!(progress_index < result_index);
        }
    }
}

#[tokio::test]
async fn query_emits_parallel_tool_results_as_each_tool_finishes() {
    let mut fixtures = ScriptToolFixtures::new(
        Arc::new(ParallelDelayTool::new()),
        named_spec("parallel_tool", "", ToolExecutionMode::ReadOnly, true),
        "run the tools",
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let callback: EventCallback = Arc::new(move |event: QueryEvent| {
        let seen_clone = Arc::clone(&seen_clone);
        Box::pin(async move {
            match event {
                QueryEvent::ToolUseStart { id, .. } => {
                    seen_clone
                        .lock()
                        .expect("lock events")
                        .push(format!("start:{id}"));
                }
                QueryEvent::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    seen_clone
                        .lock()
                        .expect("lock events")
                        .push(format!("result:{tool_use_id}:{}", content.into_string()));
                }
                _ => {}
            }
        })
    });

    fixtures
        .query(
            Arc::new(ScriptedProvider::scripted(
                "parallel-tool-provider",
                StreamScript::ParallelDelay,
            )),
            Some(callback),
            TurnConfig::new(Model::default(), None),
            QueryOptions::default(),
        )
        .await
        .expect("query should complete");

    assert_eq!(
        seen.lock().expect("lock events").as_slice(),
        &[
            "start:slow".to_string(),
            "start:fast".to_string(),
            "result:fast:fast complete".to_string(),
            "result:slow:slow complete".to_string(),
        ]
    );

    let tool_result_ids = fixtures
        .session
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_result_ids, vec!["slow", "fast"]);
}

#[path = "durability_tests.rs"]
mod durability;
