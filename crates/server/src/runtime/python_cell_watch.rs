//! Mid-tool Python cell wait-policy (aux LLM), patterned on auto-review.

use std::sync::Arc;

use async_trait::async_trait;
use devo_core::tools::{
    PythonCellCompletionEvent, PythonCellCompletionHook, PythonCellWatch, PythonCellWatchAction,
    PythonCellWatchDecision, PythonCellWatchInput, parse_python_cell_watch_decision,
};
use devo_protocol::native::ids::{SessionId, TurnId};
use devo_protocol::{
    ModelRequest, RequestContent, RequestMessage, ResponseContent, SamplingControls,
};

use crate::json_extract::extract_json_object;
use crate::runtime::ServerRuntime;
use crate::runtime::kernel_host::push_bash_notice;

const WATCH_MAX_TOKENS: usize = 256;
const WATCH_JSON_SHAPE: &str = concat!(
    "{\"action\":\"continue_fg|background|cancel\",",
    "\"wait_seconds\":number,",
    "\"rationale\":\"short reason\"}"
);
const WATCH_SYSTEM_PROMPT: &str = concat!(
    "You decide what to do about a long-running Python REPL cell that exceeded ",
    "its foreground wait budget. Reply with JSON only. Prefer continue_fg when ",
    "output shows active progress and completion looks near; prefer background ",
    "for long independent work; prefer cancel only for clearly stuck or harmful ",
    "loops. wait_seconds is required for continue_fg (seconds to wait next)."
);

/// Server-backed wait-policy decider for `ipython` cells.
pub struct ServerPythonCellWatch {
    runtime: Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
}

impl ServerPythonCellWatch {
    pub fn new(runtime: Arc<ServerRuntime>, session_id: SessionId, turn_id: TurnId) -> Self {
        Self {
            runtime,
            session_id,
            turn_id,
        }
    }
}

#[async_trait]
impl PythonCellWatch for ServerPythonCellWatch {
    async fn decide(&self, input: PythonCellWatchInput) -> PythonCellWatchDecision {
        let from_inline = if let Some(stream) =
            self.runtime.active_stream_state(self.session_id).await
        {
            let stream = stream.lock().await;
            stream.turn_inline.as_ref().map(|inline| {
                let prefix = inline
                    .last_model_request
                    .lock()
                    .ok()
                    .and_then(|guard| guard.clone());
                let fallback_model = inline
                    .summary
                    .model_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| inline.hook_context.runtime_context.default_model.clone());
                (
                    Arc::clone(&inline.hook_context.runtime_context),
                    prefix,
                    fallback_model,
                )
            })
        } else {
            None
        };

        let (runtime_context, prefix, fallback_model) = if let Some(inputs) = from_inline {
            inputs
        } else {
            let Some(reservation) = self
                .runtime
                .session_turn_reservation_snapshot(self.session_id)
                .await
            else {
                return PythonCellWatchDecision {
                    action: PythonCellWatchAction::Background,
                    rationale: Some("no session reservation; defaulting to background".into()),
                };
            };
            let fallback_model = reservation
                .summary
                .model_name()
                .map(str::to_string)
                .unwrap_or_else(|| reservation.runtime_context.default_model.clone());
            (reservation.runtime_context, None, fallback_model)
        };

        let model_request = match prefix {
            Some(prefix) => extend_watch_request(prefix, &input),
            None => build_watch_request(fallback_model, &input),
        };

        let provider = self.runtime.usage_ledger.instrumented_provider(
            Arc::clone(&runtime_context.provider),
            self.session_id,
            Some(self.turn_id),
            devo_protocol::native::usage::UsagePurpose::PythonCellWatch,
        );

        let response = match provider.completion(model_request.clone()).await {
            Ok(response) => response,
            Err(first_error) => {
                tracing::warn!(
                    session_id = %self.session_id,
                    cell_id = %input.cell_id,
                    error = %first_error,
                    "python cell watch failed; retrying once"
                );
                match provider.completion(model_request).await {
                    Ok(response) => response,
                    Err(error) => {
                        tracing::warn!(
                            session_id = %self.session_id,
                            cell_id = %input.cell_id,
                            error = %error,
                            "python cell watch failed after retry; defaulting to background"
                        );
                        return PythonCellWatchDecision {
                            action: PythonCellWatchAction::Background,
                            rationale: Some("watch LLM failed; defaulting to background".into()),
                        };
                    }
                }
            }
        };

        parse_decision_from_response(&response.content)
    }
}

/// Pushes `python.completed` notices and wakes idle sessions.
pub struct ServerPythonCellCompletionHook {
    runtime: Arc<ServerRuntime>,
}

impl ServerPythonCellCompletionHook {
    pub fn new(runtime: Arc<ServerRuntime>) -> Self {
        Self { runtime }
    }
}

impl PythonCellCompletionHook for ServerPythonCellCompletionHook {
    fn completed(&self, event: PythonCellCompletionEvent) {
        let session_id = event.session_id.clone();
        push_bash_notice(
            &session_id,
            serde_json::json!({
                "kind": "async_python_completion",
                "params": {
                    "cellId": event.cell_id,
                    "status": event.status,
                    "stdoutTail": event.stdout_tail,
                    "stderrTail": event.stderr_tail,
                    "error": event.error,
                }
            }),
        );
        let runtime = Arc::clone(&self.runtime);
        let session_id: SessionId = session_id.parse().expect("session id");
        tokio::spawn(async move {
            runtime
                .drain_async_tool_completion_notices(session_id)
                .await;
        });
    }
}

fn build_watch_request(model: String, input: &PythonCellWatchInput) -> ModelRequest {
    ModelRequest {
        model_slug: devo_protocol::ModelProfileKey::Generic,
        model,
        system: Some(format!(
            "{WATCH_SYSTEM_PROMPT} JSON shape: {WATCH_JSON_SHAPE}."
        )),
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: vec![RequestContent::Text {
                text: watch_prompt(input),
            }],
        }],
        max_tokens: WATCH_MAX_TOKENS,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls {
            temperature: Some(0.0),
            ..SamplingControls::default()
        },
        request_thinking: None,
        reasoning_effort: None,
        extra_body: None,
    }
}

fn extend_watch_request(mut prefix: ModelRequest, input: &PythonCellWatchInput) -> ModelRequest {
    prefix.messages.push(RequestMessage {
        role: "user".to_string(),
        content: vec![RequestContent::Text {
            text: watch_prompt(input),
        }],
    });
    prefix.max_tokens = WATCH_MAX_TOKENS;
    prefix.request_thinking = Some("disabled".to_string());
    prefix.reasoning_effort = None;
    prefix
}

fn watch_prompt(input: &PythonCellWatchInput) -> String {
    format!(
        "Python cell `{cell}` has been running for {elapsed}ms and exceeded its wait budget.\n\
         Renewals remaining after this decision: {renewals}.\n\
         Allowed actions: continue_fg (with wait_seconds in [30, 900]), background, cancel.\n\
         Code preview:\n```\n{code}\n```\n\
         Stdout tail:\n```\n{stdout}\n```\n\
         Stderr tail:\n```\n{stderr}\n```\n\
         Reply with JSON only.",
        cell = input.cell_id,
        elapsed = input.elapsed_ms,
        renewals = input.renewals_remaining,
        code = input.code_preview,
        stdout = input.stdout_tail,
        stderr = input.stderr_tail,
    )
}

fn parse_decision_from_response(content: &[ResponseContent]) -> PythonCellWatchDecision {
    let mut combined = String::new();
    for block in content {
        if let ResponseContent::Text(text) = block {
            combined.push_str(text);
            combined.push('\n');
        }
    }
    let raw = extract_json_object(&combined)
        .map(str::to_string)
        .unwrap_or(combined);
    parse_python_cell_watch_decision(&raw)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use devo_core::tools::{PYTHON_CELL_WAIT_SECONDS_MIN, PythonCellWatchAction};

    #[test]
    fn parse_from_response_text() {
        let d = parse_decision_from_response(&[ResponseContent::Text(
            r#"Here: {"action":"continue_fg","wait_seconds":10}"#.into(),
        )]);
        assert_eq!(
            d.action,
            PythonCellWatchAction::ContinueFg {
                wait_seconds: PYTHON_CELL_WAIT_SECONDS_MIN
            }
        );
    }
}
