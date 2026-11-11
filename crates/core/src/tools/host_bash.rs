//! Host-owned shell handles. Callers must authorize before calling `start`.
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::handlers::{ShellCommandHandler, ShellExecutionMode};
use super::{
    ToolCallError, ToolContext, ToolProgress, ToolProgressSender, ToolResult, ToolResultContent,
};

mod activity;
pub use activity::{HostBashActivity, activity_snapshot, session_activities};

const MAX_HANDLES: usize = 256;
const MAX_SESSION_HANDLES: usize = 32;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const RETENTION: Duration = Duration::from_secs(30 * 60);

/// Whether the authorization permits one retry after a structured sandbox denial.
#[derive(Clone, Copy)]
pub enum SandboxRetry {
    Never,
    ApprovedDenial,
}

/// Execute an already-authorized command without changing its frozen sandbox decision.
pub async fn execute(
    ctx: ToolContext,
    input: Value,
    retry: SandboxRetry,
    progress: Option<ToolProgressSender>,
    mode: ShellExecutionMode,
) -> Result<ToolResult, ToolCallError> {
    if ctx.cancel_token.is_cancelled() {
        return Err(ToolCallError::ExecutionFailed(
            "originating turn cancelled".into(),
        ));
    }
    let outcome = ShellCommandHandler::new()
        .execute(ctx.clone(), input.clone(), progress.clone(), mode)
        .await;
    let denied = outcome.as_ref().is_ok_and(|output| {
        let text = match &output.content {
            ToolResultContent::Text(text) => text.as_str(),
            ToolResultContent::Mixed {
                text: Some(text), ..
            } => text.as_str(),
            ToolResultContent::Json(_) | ToolResultContent::Mixed { text: None, .. } => "",
        };
        text.starts_with("SANDBOX_DENIED:") || output.result_summary.starts_with("SANDBOX_DENIED:")
    });
    if denied && matches!(retry, SandboxRetry::ApprovedDenial) && !ctx.cancel_token.is_cancelled() {
        let mut ctx = ctx;
        ctx.sandbox_profile = Some("off".into());
        ShellCommandHandler::new()
            .execute(ctx, input, progress, mode)
            .await
    } else {
        outcome
    }
}

struct Entry {
    session_id: String,
    command: String,
    turn_id: Option<devo_protocol::TurnId>,
    cancelled: bool,
    cwd: std::path::PathBuf,
    created_at: chrono::DateTime<chrono::Utc>,
    cancel: CancellationToken,
    expiry_cancel: CancellationToken,
    started: Instant,
    finished: Option<Instant>,
    output: String,
    output_truncated: bool,
    exit_code: Option<i64>,
    error: Option<String>,
}
impl Entry {
    fn snapshot(&self, handle_id: &str) -> Value {
        json!({"status":"ok", "result": {
            "handle_id":handle_id, "running":self.finished.is_none(),
            "exit_code":self.exit_code, "output":self.output, "output_truncated":self.output_truncated,
            "duration":self.finished.unwrap_or_else(Instant::now).duration_since(self.started).as_secs_f64(),
            "error":self.error,
        }})
    }
    fn append(&mut self, text: &str) {
        if self.output_truncated {
            return;
        }
        let mut keep = text
            .len()
            .min(MAX_OUTPUT_BYTES.saturating_sub(self.output.len()));
        while !text.is_char_boundary(keep) {
            keep -= 1;
        }
        self.output.push_str(&text[..keep]);
        if keep < text.len() {
            const NOTICE: &str = "\n... [host bash output truncated]";
            let mut end = self.output.len().min(MAX_OUTPUT_BYTES - NOTICE.len());
            while !self.output.is_char_boundary(end) {
                end -= 1;
            }
            self.output.truncate(end);
            self.output.push_str(NOTICE);
            self.output_truncated = true;
        }
    }
}
type Registry = HashMap<String, Arc<Mutex<Entry>>>;
static HANDLES: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn error(message: &str) -> Value {
    json!({"status":"error", "error":message})
}

/// Schedule an authorized shell command. The child token retains originating turn cancellation.
pub fn start(
    mut ctx: ToolContext,
    input: Value,
    retry: SandboxRetry,
    kernel_lifetime: Option<CancellationToken>,
) -> Value {
    if input.get("env").is_some() || input.get("persistent").and_then(Value::as_bool) == Some(true)
    {
        return error("host bash does not support env or persistent execution");
    }
    if input.get("tty").and_then(Value::as_bool) == Some(true) {
        return error("host bash handles do not support tty execution");
    }
    if ctx.cancel_token.is_cancelled() {
        return error("originating turn cancelled");
    }
    if kernel_lifetime
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled)
    {
        return error("originating kernel stopped");
    }
    // The lifetime token, not an Arc back to the kernel, owns this execution.
    ctx.kernel = None;
    ctx.cancel_token = ctx.cancel_token.child_token();
    let session_id = ctx.session_id.to_string();
    let handle_id = Uuid::new_v4().to_string();
    let cwd = match crate::shell_exec::requested_workdir(&input)
        .and_then(|requested| crate::shell_exec::resolve_shell_dirs(&ctx.workspace_root, requested))
    {
        Ok(dirs) => dirs.cwd,
        Err(message) => return error(&message),
    };
    let entry = Arc::new(Mutex::new(Entry {
        session_id: session_id.clone(),
        turn_id: ctx.turn_id,
        cancelled: false,
        command: input
            .get("command")
            .or_else(|| input.get("cmd"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        cwd,
        created_at: chrono::Utc::now(),
        cancel: ctx.cancel_token.clone(),
        expiry_cancel: CancellationToken::new(),
        started: Instant::now(),
        finished: None,
        output: String::new(),
        output_truncated: false,
        exit_code: None,
        error: None,
    }));
    {
        let mut handles = HANDLES.lock().unwrap();
        handles.retain(|_, entry| {
            entry
                .lock()
                .unwrap()
                .finished
                .is_none_or(|done| done.elapsed() < RETENTION)
        });
        loop {
            let session_full = handles
                .values()
                .filter(|entry| entry.lock().unwrap().session_id == session_id)
                .count()
                >= MAX_SESSION_HANDLES;
            if handles.len() < MAX_HANDLES && !session_full {
                break;
            }
            let oldest = handles
                .iter()
                .filter_map(|(id, entry)| {
                    let state = entry.lock().unwrap();
                    if session_full && state.session_id != session_id {
                        return None;
                    }
                    state.finished.map(|finished| (id.clone(), finished))
                })
                .min_by_key(|(_, finished)| *finished);
            let Some((id, _)) = oldest else {
                return error("host bash running handle limit reached");
            };
            if let Some(entry) = handles.remove(&id) {
                entry.lock().unwrap().expiry_cancel.cancel();
            }
        }
        handles.insert(handle_id.clone(), Arc::clone(&entry));
    }
    let coordinator = ctx.agent_coordinator.clone();
    let owner_session_id = ctx.session_id;
    let work_id = format!("bash:{handle_id}");
    if let Some(coordinator) = &coordinator {
        coordinator.background_work_started(owner_session_id, work_id.clone());
    }
    let initial = entry.lock().unwrap().snapshot(&handle_id);
    let task_entry = Arc::clone(&entry);
    tokio::spawn(async move {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let collect = async {
            while let Some(event) = receiver.recv().await {
                match event {
                    ToolProgress::OutputDelta { delta } => {
                        task_entry.lock().unwrap().append(&delta)
                    }
                    ToolProgress::StatusUpdate { .. } | ToolProgress::Completion { .. } => {}
                }
            }
        };
        let cancel = ctx.cancel_token.clone();
        let execution = async {
            let execution = execute(
                ctx,
                input,
                retry,
                Some(sender),
                ShellExecutionMode::HostBackground,
            );
            tokio::pin!(execution);
            if let Some(lifetime) = kernel_lifetime {
                tokio::select! {
                    biased;
                    _ = lifetime.cancelled() => {
                        cancel.cancel();
                        execution.await
                    }
                    result = &mut execution => result,
                }
            } else {
                execution.await
            }
        };
        let (outcome, ()) = tokio::join!(execution, collect);
        {
            let mut state = task_entry.lock().unwrap();
            match outcome {
                Ok(result) => {
                    let (text, meta) = match result.content {
                        ToolResultContent::Text(text) => (Some(text), None),
                        ToolResultContent::Json(meta) => (None, Some(meta)),
                        ToolResultContent::Mixed { text, json } => (text, json),
                    };
                    state.exit_code = meta
                        .as_ref()
                        .and_then(|meta| meta.get("exit"))
                        .and_then(Value::as_i64);
                    // Final executor formatting includes cancellation / timeout / sandbox errors.
                    if let Some(text) = text {
                        state.output.clear();
                        state.output_truncated = false;
                        state.append(&text);
                    }
                    if !matches!(
                        result.structured_status,
                        super::ToolTerminalStatus::Completed
                    ) {
                        state.error = Some(if state.output.is_empty() {
                            result.result_summary
                        } else {
                            state.output.clone()
                        });
                    }
                }
                Err(err) => state.error = Some(err.to_string()),
            }
            state.cancelled = state.cancel.is_cancelled();
            state.finished = Some(Instant::now());
        }
        if let Some(coordinator) = coordinator {
            let result = task_entry.lock().unwrap().snapshot(&handle_id).to_string();
            coordinator.background_work_completed(owner_session_id, work_id, result);
        }
        // Completed records expire even if the session never makes another request.
        let expiry_cancel = task_entry.lock().unwrap().expiry_cancel.clone();
        tokio::select! {
            _ = tokio::time::sleep(RETENTION) => {},
            _ = expiry_cancel.cancelled() => {},
        }
        HANDLES.lock().unwrap().remove(&handle_id);
    });
    initial
}

/// Inspect or cancel only handles owned by this session; never accepts an OS PID.
pub fn control(session_id: &str, action: &str, params: &Value) -> Value {
    let Some(id) = params.get("handle_id").and_then(Value::as_str) else {
        return error("missing bash handle_id");
    };
    let mut handles = HANDLES.lock().unwrap();
    let Some(entry) = handles.get(id).cloned() else {
        return error("unknown bash handle");
    };
    let state = entry.lock().unwrap();
    if state.session_id != session_id {
        return error("unknown bash handle");
    }
    match action {
        "bash.poll" => state.snapshot(id),
        "bash.kill" => {
            state.cancel.cancel();
            state.snapshot(id)
        }
        "bash.forget" => {
            if state.finished.is_none() {
                return error("cannot forget a running bash handle");
            }
            activity::retain_completed(id, &state);
            state.expiry_cancel.cancel();
            handles.remove(id);
            json!({"status":"ok", "result":{"forgotten":true}})
        }
        _ => error("unknown bash control action"),
    }
}

/// Cancel owned processes and release records when a kernel/session is torn down.
pub fn cancel_session(session_id: &str) {
    let mut handles = HANDLES.lock().unwrap();
    handles.retain(|_, entry| {
        let state = entry.lock().unwrap();
        if state.session_id == session_id {
            state.cancel.cancel();
            state.expiry_cancel.cancel();
            false
        } else {
            true
        }
    });
    activity::clear_session(session_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn output_is_bounded_on_utf8_boundary() {
        let mut entry = Entry {
            session_id: "bounds".into(),
            command: String::new(),
            turn_id: None,
            cancelled: false,
            cwd: std::path::PathBuf::new(),
            created_at: chrono::Utc::now(),
            cancel: CancellationToken::new(),
            expiry_cancel: CancellationToken::new(),
            started: Instant::now(),
            finished: None,
            output: "x".repeat(MAX_OUTPUT_BYTES - 1),
            output_truncated: false,
            exit_code: None,
            error: None,
        };
        entry.append("é");
        assert_eq!(entry.output.len(), MAX_OUTPUT_BYTES);
        assert!(entry.output.ends_with("[host bash output truncated]"));
        assert!(entry.output_truncated);
        let output = entry.output.clone();
        entry.append("abc");
        assert_eq!(entry.output, output);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn origin_cancellation_finishes_started_handle() {
        use crate::contracts::ToolBudgets;
        use crate::invocation::ToolCallId;
        use devo_protocol::{CollaborationMode, SessionId};
        use devo_tools::ToolAgentScope;

        let cancel = CancellationToken::new();
        let session = Uuid::new_v4().to_string();
        let ctx = ToolContext {
            output_store: None,
            tool_call_id: ToolCallId("host-bash-cancel".into()),
            session_id: SessionId::from(session.clone()),
            turn_id: None,
            workspace_root: std::env::temp_dir(),
            budgets: ToolBudgets {
                output_limit_bytes: 32 * 1024,
                wall_time_limit_ms: Some(10_000),
            },
            cancel_token: cancel.clone(),
            agent_scope: ToolAgentScope::Parent,
            collaboration_mode: CollaborationMode::Build,
            agent_coordinator: None,
            client_filesystem: None,
            file_read_ledger: None,
            network_proxy: None,
            network_no_proxy: None,
            sandbox_profile: Some("off".into()),
            sandbox_permission_overlay: None,
            kernel: None,
            python_cell_first_wait_ms: None,
            python_cell_watch: None,
            python_cell_completion: None,
            session_dir: None,
        };
        let started = start(
            ctx,
            json!({
                "command": "printf streaming; sleep 30",
                "workdir": std::fs::canonicalize(std::env::temp_dir())
                    .expect("canonical temp dir")
                    .to_string_lossy()
            }),
            SandboxRetry::Never,
            None,
        );
        let id = started["result"]["handle_id"]
            .as_str()
            .expect("handle id")
            .to_string();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = control(&session, "bash.poll", &json!({"handle_id": id}));
                if snapshot["result"]["output"]
                    .as_str()
                    .is_some_and(|output| output.contains("streaming"))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("streaming output");
        cancel.cancel();
        let done = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = control(&session, "bash.poll", &json!({"handle_id": id}));
                if snapshot["result"]["running"] == false {
                    break snapshot;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancel completes");
        assert!(
            done["result"]["output"]
                .as_str()
                .is_some_and(|output| output.contains("cancelled")),
            "cancelled result: {done}"
        );
        let activity = session_activities(&session);
        assert_eq!(activity.len(), 1);
        assert_eq!((activity[0].running, activity[0].cancelled), (false, true));
        cancel_session(&session);
    }

    #[test]
    fn cancel_session_cancels_owned_token_and_removes_handle() {
        let session = Uuid::new_v4().to_string();
        let id = Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        HANDLES.lock().unwrap().insert(
            id.clone(),
            Arc::new(Mutex::new(Entry {
                session_id: session.clone(),
                command: String::new(),
                turn_id: None,
                cancelled: false,
                cwd: std::path::PathBuf::new(),
                created_at: chrono::Utc::now(),
                cancel: cancel.clone(),
                expiry_cancel: CancellationToken::new(),
                started: Instant::now(),
                finished: None,
                output: String::new(),
                output_truncated: false,
                exit_code: None,
                error: None,
            })),
        );
        cancel_session(&session);
        assert_eq!(
            (
                cancel.is_cancelled(),
                control(&session, "bash.poll", &json!({"handle_id":id}))
            ),
            (
                true,
                json!({"status":"error", "error":"unknown bash handle"})
            )
        );
    }
}

#[cfg(all(test, unix))]
#[path = "host_bash_background_tests.rs"]
mod background_tests;
