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
mod interactive;
pub use crate::shell_exec::InteractiveIo;
pub use interactive::{BashMode, control_async};
mod tail;
#[cfg(all(test, unix))]
mod tail_tests;
pub use activity::{HostBashActivity, activity_snapshot, session_activities};
pub use tail::OutputTail;

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
        .execute(ctx.clone(), input.clone(), progress.clone(), mode.clone())
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
    mode: BashMode,
    io: Option<InteractiveIo>,
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
    tail: OutputTail,
    output_truncated: bool,
    exit_code: Option<i64>,
    error: Option<String>,
}
impl Entry {
    fn snapshot(&self, handle_id: &str) -> Value {
        json!({"status":"ok", "result": {
            "handle_id":handle_id, "running":self.finished.is_none(),
            "mode":self.mode.as_str(), "command":self.command,
            "ready":self.io.as_ref().is_none_or(InteractiveIo::is_ready),
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
    ctx: ToolContext,
    input: Value,
    retry: SandboxRetry,
    kernel_lifetime: Option<CancellationToken>,
) -> Value {
    start_with_session_lifetime(ctx, input, retry, kernel_lifetime, None)
}

/// Interactive callers supply a session token independent of turn/kernel
/// lifetime. Linking it before registry insertion closes delete/admit races.
pub fn start_with_session_lifetime(
    mut ctx: ToolContext,
    mut input: Value,
    retry: SandboxRetry,
    mut kernel_lifetime: Option<CancellationToken>,
    session_lifetime: Option<CancellationToken>,
) -> Value {
    if input.get("env").is_some() || input.get("persistent").and_then(Value::as_bool) == Some(true)
    {
        return error("host bash does not support env or persistent execution");
    }
    let mode = match BashMode::parse(&input) {
        Ok(mode) => mode,
        Err(message) => return error(&message),
    };
    input["tty"] = json!(mode == BashMode::Pty);
    let idle_timeout = match input.get("idle_timeout_ms") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_u64() {
            Some(ms) if ms > 0 && mode != BashMode::Pipe => Some(Duration::from_millis(ms)),
            _ => {
                return error(
                    "idle_timeout_ms requires an interactive mode and a positive integer",
                );
            }
        },
    };
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
    ctx.cancel_token = if mode == BashMode::Pipe {
        ctx.cancel_token.child_token()
    } else {
        // Admission was checked above. Interactive tasks are session-owned, not
        // tied to a transient turn, Python kernel, or client connection.
        kernel_lifetime = None;
        session_lifetime
            .unwrap_or_else(|| ctx.cancel_token.clone())
            .child_token()
    };
    if ctx.cancel_token.is_cancelled() {
        return error("originating session stopped");
    }
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
        mode,
        io: (mode != BashMode::Pipe).then(|| InteractiveIo::new(mode == BashMode::Pty)),
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
        tail: OutputTail::default(),
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
        let (tail, io) = {
            let state = task_entry.lock().unwrap();
            (state.tail.clone(), state.io.clone())
        };
        let execution_mode = match &io {
            Some(io) => ShellExecutionMode::HostInteractive {
                tail,
                io: io.clone(),
            },
            None => ShellExecutionMode::HostBackground { tail },
        };
        let execution = async {
            let execution = execute(
                ctx,
                input,
                if mode == BashMode::Pipe {
                    retry
                } else {
                    SandboxRetry::Never
                },
                Some(sender),
                execution_mode,
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
        let idle_expired = std::sync::atomic::AtomicBool::new(false);
        let idle_watch = async {
            match (&io, idle_timeout) {
                (Some(io), Some(timeout)) => loop {
                    if io.is_ready() && io.idle_for() >= timeout {
                        idle_expired.store(true, std::sync::atomic::Ordering::SeqCst);
                        cancel.cancel();
                        break;
                    }
                    tokio::time::sleep(timeout.min(Duration::from_millis(100))).await;
                },
                _ => std::future::pending::<()>().await,
            }
        };
        let execution = async {
            tokio::pin!(execution);
            tokio::select! {
                result = &mut execution => result,
                () = idle_watch => execution.await,
            }
        };
        let (outcome, ()) = tokio::join!(execution, collect);
        if let Some(io) = &io {
            io.finish();
        }
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
            if idle_expired.load(std::sync::atomic::Ordering::SeqCst) {
                state.error = Some("interactive bash idle timeout".into());
                state.append("\ninteractive bash idle timeout");
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
    if action == "bash.list" {
        let handles = HANDLES.lock().unwrap();
        let mut snapshots: Vec<Value> = handles
            .iter()
            .filter_map(|(id, entry)| {
                let state = entry.lock().unwrap();
                (state.session_id == session_id && state.mode != BashMode::Pipe)
                    .then(|| state.snapshot(id)["result"].clone())
            })
            .collect();
        snapshots.sort_by_key(|value| value["handle_id"].as_str().unwrap_or_default().to_owned());
        return json!({"status":"ok", "result":{"handles":snapshots}});
    }
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
        "bash.poll" | "bash.attach" => {
            if action == "bash.attach" && state.mode == BashMode::Pipe {
                return error("only interactive bash handles can be attached");
            }
            // Recent bytes are a polling view, not duplicate transcript content
            // in the completion notification sent to the owning agent.
            let mut snapshot = state.snapshot(id);
            snapshot["result"]["tail"] = json!(state.tail.snapshot(MAX_OUTPUT_BYTES));
            snapshot
        }
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
mod tests;

#[cfg(all(test, unix))]
mod interactive_tests;

#[cfg(all(test, unix))]
#[path = "host_bash_background_tests.rs"]
mod background_tests;
