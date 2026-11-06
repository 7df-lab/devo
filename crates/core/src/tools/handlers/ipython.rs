//! Model-facing `ipython` tool — executes code in the session RLM kernel.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgress, ToolProgressSender, ToolResult, ToolResultContent,
    ToolResultImage,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolSpec};
use devo_kernel::CellWaitOutcome;
use devo_tools::python_cell_watch::{
    PYTHON_CELL_CODE_PREVIEW_CHARS, PYTHON_CELL_MAX_CONTINUE_RENEWALS,
    PYTHON_CELL_OUTPUT_TAIL_CHARS, PythonCellCompletionEvent, PythonCellWatchAction,
    PythonCellWatchDecision, PythonCellWatchInput, effective_first_wait_ms, output_tail,
};

mod startup;
use startup::persist_kernel_namespace_snapshot;
pub use startup::{
    KernelNamespaceRestoreState, ensure_kernel, ensure_kernel_with_restore_state,
    kernel_namespace_manifest_path, kernel_namespace_snapshot_path,
};

pub struct IpythonHandler {
    spec: ToolSpec,
}

impl Default for IpythonHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl IpythonHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "ipython".into(),
                description: "Execute Python code in the session RLM kernel. Namespace persists across cells and turns.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([(
                        "code".to_string(),
                        JsonSchema::string(Some("Python source to execute")),
                    )]),
                    Some(vec!["code".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::StructuredJson,
                execution_mode: ToolExecutionMode::Mutating,
                capability_tags: vec![],
                supports_parallel: false,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
                display_name: Some("ipython".into()),
                supports_cancellation: Some(true),
                supports_streaming: Some(true),
            },
        }
    }
}

#[async_trait]
impl ToolHandler for IpythonHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let code = input
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'code' field".into()))?;

        let kernel = ctx.kernel.as_ref().ok_or_else(|| {
            ToolCallError::InternalError(
                "ipython tool requires a session kernel (execution_surface=Rlm)".into(),
            )
        })?;

        let started = std::time::Instant::now();
        let first_wait = std::time::Duration::from_millis(effective_first_wait_ms(
            ctx.python_cell_first_wait_ms,
        ));

        let inflight = tokio::select! {
            biased;
            () = ctx.cancel_token.cancelled() => {
                let _ = kernel.interrupt(None).await;
                return Err(ToolCallError::Cancelled);
            }
            result = kernel.begin_execute(code.to_string()) => {
                result.map_err(|e| ToolCallError::InternalError(e.to_string()))?
            }
        };

        let mut renewals_remaining = PYTHON_CELL_MAX_CONTINUE_RENEWALS;
        let mut wait_budget = first_wait;
        let output = loop {
            let wait_outcome = tokio::select! {
                biased;
                () = ctx.cancel_token.cancelled() => {
                    let _ = inflight.interrupt().await;
                    let _ = inflight.wait_until_done().await;
                    return Err(ToolCallError::Cancelled);
                }
                outcome = inflight.wait_for(wait_budget) => {
                    outcome.map_err(|e| ToolCallError::InternalError(e.to_string()))?
                }
            };

            match wait_outcome {
                CellWaitOutcome::Done(output) => break output,
                CellWaitOutcome::TimedOut { snapshot } => {
                    if let Some(progress) = &progress {
                        let _ = progress.send(ToolProgress::StatusUpdate {
                            message: "Python cell still running; consulting wait policy…".into(),
                            percent: None,
                        });
                    }

                    let decision = if renewals_remaining == 0 {
                        PythonCellWatchDecision {
                            action: PythonCellWatchAction::Background,
                            rationale: Some(
                                "max continue_fg renewals reached; forcing background".into(),
                            ),
                        }
                    } else if let Some(watch) = &ctx.python_cell_watch {
                        watch
                            .decide(PythonCellWatchInput {
                                cell_id: inflight.cell_id().to_string(),
                                code_preview: output_tail(code, PYTHON_CELL_CODE_PREVIEW_CHARS),
                                elapsed_ms: started.elapsed().as_millis() as u64,
                                stdout_tail: output_tail(
                                    &snapshot.stdout,
                                    PYTHON_CELL_OUTPUT_TAIL_CHARS,
                                ),
                                stderr_tail: output_tail(
                                    &snapshot.stderr,
                                    PYTHON_CELL_OUTPUT_TAIL_CHARS,
                                ),
                                renewals_remaining,
                            })
                            .await
                    } else {
                        // No decider (unit tests): keep extending first-wait without parking.
                        wait_budget = first_wait;
                        continue;
                    };

                    match decision.action {
                        PythonCellWatchAction::ContinueFg { wait_seconds } => {
                            if renewals_remaining == 0 {
                                // Should have been forced to background above.
                                renewals_remaining = 0;
                            } else {
                                renewals_remaining = renewals_remaining.saturating_sub(1);
                            }
                            wait_budget = std::time::Duration::from_secs(wait_seconds);
                            if let Some(progress) = &progress {
                                let _ = progress.send(ToolProgress::StatusUpdate {
                                    message: format!(
                                        "Continuing foreground wait for {wait_seconds}s…"
                                    ),
                                    percent: None,
                                });
                            }
                        }
                        PythonCellWatchAction::Cancel => {
                            let _ = inflight.interrupt().await;
                            let duration_ms = started.elapsed().as_millis() as u64;
                            // Interrupt is best-effort; do not block the tool on Done.
                            // Park the collector so the execute gate still releases.
                            inflight.park(|_id, _result| {});
                            let mut cancelled = snapshot;
                            cancelled.status = "cancelled".into();
                            return Ok(build_ipython_result(
                                cancelled,
                                duration_ms,
                                Some("cancelled by wait policy"),
                            ));
                        }
                        PythonCellWatchAction::Background => {
                            let cell_id = inflight.cell_id().to_string();
                            let duration_ms = started.elapsed().as_millis() as u64;
                            let completion = ctx.python_cell_completion.clone();
                            let session_id = ctx.session_id.to_string();
                            let session_dir = ctx.session_dir.clone();
                            let kernel_for_snap = Arc::clone(kernel);
                            inflight.park(move |completed_id, result| {
                                let status_ok = matches!(&result, Ok(out) if out.status == "ok");
                                if let Some(hook) = completion {
                                    let (status, stdout, stderr, error) = match &result {
                                        Ok(out) => (
                                            out.status.clone(),
                                            output_tail(&out.stdout, PYTHON_CELL_OUTPUT_TAIL_CHARS),
                                            output_tail(&out.stderr, PYTHON_CELL_OUTPUT_TAIL_CHARS),
                                            out.error_value.clone(),
                                        ),
                                        Err(err) => (
                                            "error".into(),
                                            String::new(),
                                            String::new(),
                                            Some(err.to_string()),
                                        ),
                                    };
                                    hook.completed(PythonCellCompletionEvent {
                                        session_id,
                                        cell_id: completed_id,
                                        status,
                                        stdout_tail: stdout,
                                        stderr_tail: stderr,
                                        error,
                                    });
                                }
                                if status_ok {
                                    let dir = session_dir;
                                    let kernel = kernel_for_snap;
                                    tokio::spawn(async move {
                                        persist_kernel_namespace_snapshot(
                                            kernel.as_ref(),
                                            dir.as_deref(),
                                        )
                                        .await;
                                    });
                                }
                            });
                            let provisional = json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!(
                                        "Python cell {cell_id} parked in background after wait budget. \
                                         A completion follow-up will arrive when it finishes."
                                    )
                                }],
                                "details": {
                                    "status": "background",
                                    "cellId": cell_id,
                                    "durationMs": duration_ms,
                                    "stdout": snapshot.stdout,
                                    "stderr": snapshot.stderr,
                                    "result": snapshot.result,
                                    "rationale": decision.rationale,
                                }
                            });
                            return Ok(ToolResult::success(
                                ToolResultContent::Json(provisional),
                                "ipython backgrounded",
                            ));
                        }
                    }
                }
            }
        };

        if ctx.cancel_token.is_cancelled() {
            let _ = kernel.interrupt(None).await;
            return Err(ToolCallError::Cancelled);
        }
        let duration_ms = started.elapsed().as_millis() as u64;
        let status_ok = output.status == "ok";
        let result = build_ipython_result(output, duration_ms, None);
        if status_ok {
            persist_kernel_namespace_snapshot(kernel.as_ref(), ctx.session_dir.as_deref()).await;
        }
        Ok(result)
    }
}

fn build_ipython_result(
    output: devo_kernel::CellOutput,
    duration_ms: u64,
    status_override: Option<&str>,
) -> ToolResult {
    let status = status_override.unwrap_or(output.status.as_str());
    // The TUI IPythonCellComponent expects ToolExecution result
    // shape: `{ content: TextBlock[], details: { status, durationMs, … } }`.
    let mut details = json!({
        "stdout": output.stdout,
        "stderr": output.stderr,
        "result": output.result,
        "status": status,
        "durationMs": duration_ms,
    });
    if !output.diffs.is_empty() {
        details["diffs"] = json!(
            output
                .diffs
                .iter()
                .map(|d| {
                    json!({
                        "path": d.path,
                        "oldStr": d.old_str,
                        "newStr": d.new_str,
                        "startLine": d.start_line,
                    })
                })
                .collect::<Vec<_>>()
        );
    }
    if !output.attachments.is_empty() {
        details["attachments"] = json!(
            output
                .attachments
                .iter()
                .map(|a| {
                    json!({
                        "mimeType": a.mime_type,
                        "path": a.path,
                    })
                })
                .collect::<Vec<_>>()
        );
    }
    if let Some(ename) = output.error_name.as_ref() {
        details["errorEname"] = json!(ename);
        details["error"] = json!({
            "ename": ename,
            "evalue": output.error_value.clone().unwrap_or_default(),
            "traceback": output.traceback.clone(),
        });
    }

    let mut text = String::new();
    if !output.stdout.is_empty() {
        text.push_str(&output.stdout);
    }
    if let Some(result) = &output.result {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(result);
    }
    if !output.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&output.stderr);
    }
    if status != "ok" && !output.traceback.is_empty() {
        let tb = output.traceback.join("\n");
        if !tb.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&tb);
        }
    }
    if text.is_empty() {
        text = format!("(status: {status})");
    }

    let images: Vec<ToolResultImage> = output
        .attachments
        .into_iter()
        .map(|a| ToolResultImage {
            mime_type: a.mime_type,
            data_base64: a.data,
        })
        .collect();

    let content = ToolResultContent::Json(json!({
        "content": [{ "type": "text", "text": text }],
        "details": details,
    }));

    if status != "ok" {
        let mut result = ToolResult::error(
            content,
            if status_override.is_some() {
                "ipython cell cancelled"
            } else {
                "ipython cell failed"
            },
            ToolCallError::InternalError(
                output
                    .error_value
                    .clone()
                    .unwrap_or_else(|| status.to_string()),
            ),
        );
        result.images = images;
        return result;
    }

    let mut result = ToolResult::success(content, "ipython ok");
    result.images = images;
    result
}

#[cfg(test)]
#[path = "kernel_startup_tests.rs"]
mod kernel_startup_tests;
