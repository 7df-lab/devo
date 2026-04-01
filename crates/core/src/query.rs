use std::sync::Arc;

use futures::StreamExt;
use tracing::{debug, info, warn};

use claw_provider::{ModelProvider, ModelRequest, ResponseContent, StopReason, StreamEvent};
use claw_tools::{ToolCall, ToolContext, ToolOrchestrator, ToolRegistry};

use crate::{AgentError, ContentBlock, Message, Role, SessionState};

/// Events emitted during a query for the caller (CLI/UI) to observe.
#[derive(Debug, Clone)]
pub enum QueryEvent {
    /// Incremental text from the assistant.
    TextDelta(String),
    /// The assistant started a tool call.
    ToolUseStart { id: String, name: String },
    /// A tool call completed.
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// A turn is complete (model stopped generating).
    TurnComplete { stop_reason: StopReason },
    /// Token usage update.
    Usage {
        input_tokens: usize,
        output_tokens: usize,
    },
}

/// Callback for streaming query events to the UI layer.
pub type EventCallback = Arc<dyn Fn(QueryEvent) + Send + Sync>;

/// The recursive agent loop — the beating heart of the runtime.
///
/// This is the Rust equivalent of Claude Code's `query.ts`. It drives
/// multi-turn conversations by:
///
/// 1. Building the model request from session state
/// 2. Streaming the model response
/// 3. Collecting assistant text and tool_use blocks
/// 4. Executing tool calls via the orchestrator
/// 5. Appending tool_result messages
/// 6. Recursing if the model wants to continue
///
/// The loop terminates when:
/// - The model emits `end_turn` with no tool calls
/// - Max turns are exceeded
/// - An unrecoverable error occurs
pub async fn query(
    session: &mut SessionState,
    provider: &dyn ModelProvider,
    registry: Arc<ToolRegistry>,
    orchestrator: &ToolOrchestrator,
    on_event: Option<EventCallback>,
) -> Result<(), AgentError> {
    let emit = |event: QueryEvent| {
        if let Some(ref cb) = on_event {
            cb(event);
        }
    };

    loop {
        if session.turn_count >= session.config.max_turns {
            return Err(AgentError::MaxTurnsExceeded(session.config.max_turns));
        }

        session.turn_count += 1;
        info!(turn = session.turn_count, "starting turn");

        // Build model request
        let request = ModelRequest {
            model: session.config.model.clone(),
            system: if session.config.system_prompt.is_empty() {
                None
            } else {
                Some(session.config.system_prompt.clone())
            },
            messages: session.to_request_messages(),
            max_tokens: session.config.token_budget.max_output_tokens,
            tools: Some(registry.tool_definitions()),
            temperature: None,
        };

        // Stream model response
        let mut stream = provider
            .stream(request)
            .await
            .map_err(AgentError::Provider)?;

        let mut assistant_text = String::new();
        let mut tool_uses: Vec<(String, String, String)> = Vec::new(); // (id, name, json_accum)
        let mut stop_reason = None;

        while let Some(event) = stream.next().await {
            match event {
                Ok(StreamEvent::TextDelta { text, .. }) => {
                    assistant_text.push_str(&text);
                    emit(QueryEvent::TextDelta(text));
                }
                Ok(StreamEvent::ContentBlockStart {
                    content: ResponseContent::ToolUse { id, name, .. },
                    ..
                }) => {
                    emit(QueryEvent::ToolUseStart {
                        id: id.clone(),
                        name: name.clone(),
                    });
                    tool_uses.push((id, name, String::new()));
                }
                Ok(StreamEvent::InputJsonDelta { partial_json, .. }) => {
                    if let Some(last) = tool_uses.last_mut() {
                        last.2.push_str(&partial_json);
                    }
                }
                Ok(StreamEvent::MessageDone { response }) => {
                    stop_reason = response.stop_reason.clone();
                    session.total_input_tokens += response.usage.input_tokens;
                    session.total_output_tokens += response.usage.output_tokens;
                    emit(QueryEvent::Usage {
                        input_tokens: response.usage.input_tokens,
                        output_tokens: response.usage.output_tokens,
                    });
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(error = %e, "stream error");
                    return Err(AgentError::Provider(e));
                }
            }
        }

        // Build assistant message
        let mut assistant_content: Vec<ContentBlock> = Vec::new();

        if !assistant_text.is_empty() {
            assistant_content.push(ContentBlock::Text {
                text: assistant_text,
            });
        }

        let tool_calls: Vec<ToolCall> = tool_uses
            .into_iter()
            .map(|(id, name, json_str)| {
                let input = serde_json::from_str(&json_str)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                assistant_content.push(ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                });
                ToolCall { id, name, input }
            })
            .collect();

        session.push_message(Message {
            role: Role::Assistant,
            content: assistant_content,
        });

        // If no tool calls, we're done
        if tool_calls.is_empty() {
            if let Some(sr) = stop_reason {
                emit(QueryEvent::TurnComplete { stop_reason: sr });
            }
            debug!("no tool calls, ending query loop");
            return Ok(());
        }

        // Execute tool calls
        let tool_ctx = ToolContext {
            cwd: session.cwd.clone(),
            permissions: Arc::new(claw_permissions::RuleBasedPolicy::new(
                session.config.permission_mode,
            )),
            session_id: session.id.clone(),
        };

        let results = orchestrator.execute_batch(&tool_calls, &tool_ctx).await;

        // Build tool result message (user role, per Anthropic API convention)
        let result_content: Vec<ContentBlock> = results
            .into_iter()
            .map(|r| {
                emit(QueryEvent::ToolResult {
                    tool_use_id: r.tool_use_id.clone(),
                    content: r.output.content.clone(),
                    is_error: r.output.is_error,
                });
                ContentBlock::ToolResult {
                    tool_use_id: r.tool_use_id,
                    content: r.output.content,
                    is_error: r.output.is_error,
                }
            })
            .collect();

        session.push_message(Message {
            role: Role::User,
            content: result_content,
        });

        // Tool results appended — loop back to get the model's follow-up response.
        // Never exit here: the model must see the tool results before we can stop.
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use anyhow::Result;
    use async_trait::async_trait;
    use futures::Stream;
    use serde_json::json;

use claw_permissions::PermissionMode;
use claw_provider::{
        ModelRequest, ModelResponse, ResponseContent, StopReason, StreamEvent, Usage,
    };
use claw_tools::{Tool, ToolOrchestrator, ToolOutput, ToolRegistry};

    use super::query;
    use crate::{ContentBlock, Message, SessionConfig, SessionState};

    struct SingleToolUseProvider {
        requests: AtomicUsize,
    }

    #[async_trait]
impl claw_provider::ModelProvider for SingleToolUseProvider {
        async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let request_number = self.requests.fetch_add(1, Ordering::SeqCst);

            let events = if request_number == 0 {
                vec![
                    Ok(StreamEvent::ContentBlockStart {
                        index: 0,
                        content: ResponseContent::ToolUse {
                            id: "tool-1".into(),
                            name: "mutating_tool".into(),
                            input: json!({ "value": 1 }),
                        },
                    }),
                    Ok(StreamEvent::InputJsonDelta {
                        index: 0,
                        partial_json: r#"{"value":1}"#.into(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-1".into(),
                            content: vec![ResponseContent::ToolUse {
                                id: "tool-1".into(),
                                name: "mutating_tool".into(),
                                input: json!({ "value": 1 }),
                            }],
                            stop_reason: Some(StopReason::ToolUse),
                            usage: Usage::default(),
                        },
                    }),
                ]
            } else {
                vec![
                    Ok(StreamEvent::TextDelta {
                        index: 0,
                        text: "done".into(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-2".into(),
                            content: vec![ResponseContent::Text("done".into())],
                            stop_reason: Some(StopReason::EndTurn),
                            usage: Usage::default(),
                        },
                    }),
                ]
            };

            Ok(Box::pin(futures::stream::iter(events)))
        }

        fn name(&self) -> &str {
            "test-provider"
        }
    }

    struct MutatingTool;

    #[async_trait]
    impl Tool for MutatingTool {
        fn name(&self) -> &str {
            "mutating_tool"
        }

        fn description(&self) -> &str {
            "A test-only mutating tool."
        }

        fn input_schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "value": { "type": "integer" }
                },
                "required": ["value"]
            })
        }

        async fn execute(
            &self,
        _ctx: &claw_tools::ToolContext,
            _input: serde_json::Value,
        ) -> Result<ToolOutput> {
            Ok(ToolOutput::success("ok"))
        }
    }

    #[tokio::test]
    async fn query_uses_session_permission_mode_for_mutating_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(MutatingTool));
        let registry = Arc::new(registry);
        let orchestrator = ToolOrchestrator::new(Arc::clone(&registry));

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
            &SingleToolUseProvider {
                requests: AtomicUsize::new(0),
            },
            registry,
            &orchestrator,
            None,
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
}
