use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use devo_core::AgentError;
use devo_core::ContentBlock;
use devo_core::EventCallback;
use devo_core::Message;
use devo_core::Model;
use devo_core::ModelRequest;
use devo_core::ModelResponse;
use devo_core::QueryEvent;
use devo_core::QueryOptions;
use devo_core::ResponseContent;
use devo_core::Role;
use devo_core::SessionConfig;
use devo_core::SessionState;
use devo_core::StopReason;
use devo_core::StreamEvent;
use devo_core::TurnConfig;
use devo_core::Usage;
use devo_core::query;
use devo_core::tools::ToolRegistry;
use devo_core::tools::ToolRuntime;
use devo_provider::ModelProviderSDK;
use devo_provider::error::ProviderError;
use futures::Stream;
use pretty_assertions::assert_eq;

const MAX_REQUEST_BYTES: usize = 32_768;
const OLD_HISTORY: &str = "old-history-marker";
const TOOL_RESULT: &str = "tool-result-marker";
const LATEST_INPUT: &str = "latest user request";
const LATEST_INPUT_DETAIL: &str = "detail from the same user input";
const CONTEXT_LIMIT_MESSAGE: &str = "This model's maximum context length is 256000 tokens. However, you requested 32768 output tokens and your prompt contains at least 223233 input tokens, for a total of at least 256001 tokens. Please reduce the length of the input prompt or the number of requested output tokens. (parameter=input_tokens, value=223233)";

#[derive(Default)]
struct SizeLimitedProvider {
    summary_limit: Option<usize>,
    fail_on_last_fragment: bool,
    queries: Mutex<Vec<ModelRequest>>,
    summaries: Mutex<Vec<ModelRequest>>,
}

#[async_trait]
impl ModelProviderSDK for SizeLimitedProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        let input = serde_json::to_string(&request.messages)?;
        self.summaries
            .lock()
            .expect("record summary request")
            .push(request);
        let mut summary = String::from("Earlier work summary");
        if let Some(limit) = self.summary_limit {
            if input.len() > limit {
                return Err(anyhow::anyhow!("Prompt is too long for this deployment")
                    .context("self-hosted summarizer returned HTTP 400"));
            }
            // Carry observations forward only if they reached this invocation.
            for marker in ["FIRST_OBSERVATION", "LAST_OBSERVATION"] {
                if input.contains(marker) {
                    summary.push_str(marker);
                }
            }
            if self.fail_on_last_fragment && input.contains("LAST_OBSERVATION") {
                summary.clear();
            }
        }
        Ok(ModelResponse {
            id: "compaction-response".to_string(),
            content: vec![ResponseContent::Text(summary)],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: Default::default(),
        })
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let input_bytes = serde_json::to_vec(&request.messages)?.len();
        self.queries
            .lock()
            .expect("record query request")
            .push(request);
        if input_bytes > MAX_REQUEST_BYTES {
            if self.summary_limit.is_some() {
                return Err(anyhow::Error::new(ProviderError::InvalidRequestError {
                    message: "Prompt is too long for this deployment".into(),
                    details: None,
                }));
            }
            return Err(anyhow::Error::new(ProviderError::ContextLimitError {
                message: CONTEXT_LIMIT_MESSAGE.to_string(),
                current_tokens: None,
                limit: None,
            }));
        }

        Ok(Box::pin(futures::stream::iter([Ok(
            StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "final-response".to_string(),
                    content: vec![ResponseContent::Text("done".to_string())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: Default::default(),
                },
            },
        )])))
    }

    fn name(&self) -> &str {
        "size-limited-provider"
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CompactionEvent {
    Started,
    Completed,
    Failed,
}

async fn run_query(
    provider: Arc<SizeLimitedProvider>,
    messages: Vec<Message>,
) -> (Result<(), AgentError>, SessionState, Vec<CompactionEvent>) {
    let registry = Arc::new(ToolRegistry::new());
    let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut session = SessionState::new(SessionConfig::default(), tempdir.path().to_path_buf());
    for message in messages {
        session.push_message(message);
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured_events = Arc::clone(&events);
    let callback: EventCallback = Arc::new(move |event| {
        let captured_events = Arc::clone(&captured_events);
        Box::pin(async move {
            captured_events.lock().expect("lock events").push(event);
        })
    });

    let model = Model {
        max_tokens: Some(32_768),
        ..Model::default()
    };
    let outcome = query(
        &mut session,
        &TurnConfig::new(model, None),
        provider,
        registry,
        &runtime,
        Some(callback),
        QueryOptions::default(),
    )
    .await;

    let compaction_events = events
        .lock()
        .expect("lock events")
        .iter()
        .filter_map(|event| match event {
            QueryEvent::ContextCompactionStarted => Some(CompactionEvent::Started),
            QueryEvent::ContextCompactionCompleted { .. } => Some(CompactionEvent::Completed),
            QueryEvent::ContextCompactionFailed { .. } => Some(CompactionEvent::Failed),
            QueryEvent::ProviderRetryStatus(_)
            | QueryEvent::ProviderQueryFailed { .. }
            | QueryEvent::TextDelta(_)
            | QueryEvent::ModelResponseCompleted
            | QueryEvent::ReasoningDelta(_)
            | QueryEvent::ReasoningCompleted
            | QueryEvent::ContextEstimate { .. }
            | QueryEvent::UsageDelta { .. }
            | QueryEvent::ToolUseStart { .. }
            | QueryEvent::ToolUseInputDelta { .. }
            | QueryEvent::ToolExecutionStart { .. }
            | QueryEvent::ToolProgress { .. }
            | QueryEvent::ToolResult { .. }
            | QueryEvent::TurnComplete { .. }
            | QueryEvent::Usage { .. } => None,
        })
        .collect::<Vec<_>>();

    (outcome, session, compaction_events)
}

#[tokio::test]
async fn context_limit_error_compacts_and_retries_query() {
    let provider = Arc::new(SizeLimitedProvider::default());
    let (outcome, session, events) = run_query(
        Arc::clone(&provider),
        vec![
            Message::user(OLD_HISTORY.repeat(3_000)),
            Message::assistant_text("earlier assistant response"),
            Message::user(LATEST_INPUT),
        ],
    )
    .await;
    outcome.expect("query should compact and retry");
    assert_eq!(
        events,
        vec![CompactionEvent::Started, CompactionEvent::Completed]
    );
    assert_eq!(provider.queries.lock().expect("query requests").len(), 2);
    assert_eq!(
        provider.summaries.lock().expect("summary requests").len(),
        1
    );
    assert_eq!(
        session.prompt_source_messages().last(),
        Some(&Message::assistant_text("done"))
    );
}

#[derive(Debug, PartialEq, Eq)]
struct QueryHistory {
    old_history: bool,
    summary: bool,
    latest_input: bool,
    completed_tool_result: bool,
}

#[tokio::test]
async fn repeated_context_limit_compacts_completed_work_from_current_turn() {
    let provider = Arc::new(SizeLimitedProvider::default());
    let call_id = "completed-python-call";
    let (outcome, _, events) = run_query(
        Arc::clone(&provider),
        vec![
            Message::user(OLD_HISTORY.repeat(3_000)),
            Message::assistant_text("earlier assistant response"),
            Message::user(LATEST_INPUT),
            Message::user(LATEST_INPUT_DETAIL),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: call_id.into(),
                    name: "ipython".into(),
                    input: serde_json::json!({"code": "print(result)"}),
                }],
            },
            Message {
                role: Role::User,
                content: vec![
                    ContentBlock::ToolResult {
                        tool_use_id: call_id.into(),
                        content: TOOL_RESULT.repeat(3_000),
                        is_error: false,
                    },
                    ContentBlock::Image {
                        mime_type: "image/png".into(),
                        data_base64: "cG5n".into(),
                    },
                ],
            },
        ],
    )
    .await;
    outcome.expect("the stronger compaction should recover the turn");

    let queries = provider.queries.lock().expect("query requests");
    let sent_history = queries
        .iter()
        .map(|request| {
            let messages = serde_json::to_string(&request.messages).expect("serialize model input");
            QueryHistory {
                old_history: messages.contains(OLD_HISTORY),
                summary: messages.contains("<compaction_summary>"),
                latest_input: messages.contains(LATEST_INPUT)
                    && messages.contains(LATEST_INPUT_DETAIL),
                completed_tool_result: messages.contains(TOOL_RESULT),
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        sent_history,
        vec![
            QueryHistory {
                old_history: true,
                summary: false,
                latest_input: true,
                completed_tool_result: true
            },
            QueryHistory {
                old_history: false,
                summary: true,
                latest_input: true,
                completed_tool_result: true
            },
            QueryHistory {
                old_history: false,
                summary: true,
                latest_input: true,
                completed_tool_result: false
            },
        ]
    );
    let summaries = provider.summaries.lock().expect("summary requests");
    let summary_budgets = summaries
        .iter()
        .map(|request| request.max_tokens)
        .collect::<Vec<_>>();
    assert_eq!(summary_budgets, vec![4096, 4096]);
    let second_summary =
        serde_json::to_string(&summaries[1].messages).expect("serialize summary input");
    assert!(second_summary.contains(TOOL_RESULT));
    assert!(second_summary.contains(call_id));
    assert_eq!(
        events,
        vec![
            CompactionEvent::Started,
            CompactionEvent::Completed,
            CompactionEvent::Started,
            CompactionEvent::Completed,
        ]
    );
}

#[tokio::test]
async fn context_limit_does_not_resend_an_unchanged_prompt() {
    let provider = Arc::new(SizeLimitedProvider::default());
    let messages = vec![
        Message::user("short old input"),
        Message::assistant_text("short old response"),
        Message::user(LATEST_INPUT.repeat(3_000)),
    ];
    let (outcome, session, events) = run_query(Arc::clone(&provider), messages.clone()).await;
    assert!(matches!(outcome, Err(AgentError::Provider(_))));
    assert_eq!(provider.queries.lock().expect("query requests").len(), 1);
    assert_eq!(
        provider.summaries.lock().expect("summary requests").len(),
        2
    );
    let visible_history = session
        .prompt_source_messages()
        .iter()
        .filter(|message| {
            !matches!(
                message.content.as_slice(),
                [ContentBlock::Text { text }] if text.starts_with("<context_changes>")
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(visible_history, messages);
    assert_eq!(
        events,
        vec![CompactionEvent::Started, CompactionEvent::Failed]
    );
}

#[tokio::test]
async fn single_oversized_tool_result_folds_all_fragments_before_replacing_history() {
    for fail_on_last_fragment in [false, true] {
        let provider = Arc::new(SizeLimitedProvider {
            summary_limit: Some(7_000),
            fail_on_last_fragment,
            ..Default::default()
        });
        let output = format!("FIRST_OBSERVATION{}LAST_OBSERVATION", "记录".repeat(7_000));
        let messages = vec![
            Message::user(LATEST_INPUT),
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ProviderReasoning {
                        provider: "test".into(),
                        payload: serde_json::json!({"signature":"opaque_reasoning_signature"}),
                    },
                    ContentBlock::ToolUse {
                        id: "scan".into(),
                        name: "read".into(),
                        input: serde_json::json!({}),
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "scan".into(),
                    content: output.clone(),
                    is_error: false,
                }],
            },
        ];
        let (outcome, session, events) = run_query(Arc::clone(&provider), messages).await;
        let history = serde_json::to_string(session.prompt_source_messages()).unwrap();
        let queries = provider.queries.lock().unwrap();
        let summaries = provider.summaries.lock().unwrap();
        assert!(summaries.len() > 1);
        assert!(summaries.iter().skip(1).all(|request| {
            !serde_json::to_string(&request.messages)
                .unwrap()
                .contains("opaque_reasoning_signature")
        }));
        assert!(history.contains(LATEST_INPUT));
        if fail_on_last_fragment {
            assert!(outcome.is_err());
            assert_eq!(queries.len(), 1);
            assert!(history.contains(&output));
            assert!(!history.contains("<compaction_summary>"));
            assert_eq!(
                events,
                vec![CompactionEvent::Started, CompactionEvent::Failed]
            );
        } else {
            outcome.expect("single large result should compact");
            assert_eq!(queries.len(), 2);
            assert!(history.contains("Earlier work summaryFIRST_OBSERVATIONLAST_OBSERVATION"));
            assert!(!history.contains(&output));
            assert_eq!(
                events,
                vec![CompactionEvent::Started, CompactionEvent::Completed]
            );
        }
    }
}
