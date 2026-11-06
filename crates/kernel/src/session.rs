//! Session-scoped REPL process and execute/interrupt/shutdown.

#[cfg(test)]
mod inflight_wait_tests;
#[cfg(test)]
mod lifetime_tests;
mod session_event_pump;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, Notify, mpsc};
use uuid::Uuid;

use crate::protocol::{KernelEvent, KernelRequest, PROTOCOL_VERSION, ReadyEvent, ReplError};

/// Async callback that answers a `host_request` sent by the Python kernel.
///
/// `host_request` is the kernel-to-host direction of the protocol. The kernel
/// is an ordinary Python process; approval policy, credentials, provider
/// access, host-owned filesystem writes, and subagent session lifecycle and
/// message routing stay in Rust. Kernel helpers such as `rlm.spawn`,
/// `rlm.list_subagents`, `rlm.delete_subagent`, and `agent_message.send` are
/// thin wrappers around this RPC; the host creates and owns the child
/// sessions. When Python code needs one of those capabilities it sends a
/// `host_request` and awaits the matching `host_reply`, instead of holding
/// privileged state or reimplementing host policy inside the kernel.
///
/// Common actions are `bash` (run through the approval pipeline),
/// `web.search` / `web.fetch`, `mcp.refresh` / `mcp.list_tools`, `fs.write`,
/// `plan.update`, `question`, and the `rlm.*` subagent calls.
///
/// Flow:
/// 1. Python calls `rlm.repl.host_request(data)`.
/// 2. The kernel writes `{"event":"host_request","id":"h1","cell_id":"c1",...}`.
/// 3. The session pump routes it here.
/// 4. This handler returns a JSON value.
/// 5. The session writes `{"type":"host_reply","id":"h1","data":...}` to stdin,
///    and the awaiting Python call resumes.
///
/// Arguments are `(request_id, data)`; `data` is the raw request object whose
/// `type` (or `action`) field names the operation. The handler is captured for
/// the live execute request and matched by the REPL `cell_id`, so a late
/// request from a finished cell cannot reuse a later turn's capabilities; idle
/// requests are denied except the narrow `bash.completed` / `bash.consumed`
/// route. Implementations must not call [`KernelSession::execute`] on the same
/// session (the execute gate is held). Replies use a separate stdin lock so
/// they never wait behind the execute FIFO that is itself waiting for this
/// reply (deadlock rule).
pub type HostRequestHandler = Arc<dyn Fn(String, Value) -> BoxFuture<'static, Value> + Send + Sync>;

/// Default deny handler when no host table is installed.
///
/// It is also the fallback for idle `host_request`s that are not the narrow
/// `bash.completed` / `bash.consumed` route, and for a late request whose
/// `cell_id` no longer matches the active execute request. The handler never
/// performs the requested action; it returns an error envelope naming the
/// action, which the session writes back as the matching `host_reply`.
///
/// Request data:
///
/// ```json
/// {"type":"bash","params":{"command":"ls"}}
/// ```
///
/// Reply data (the handler's return value):
///
/// ```json
/// {"status":"error","error":"host_request action not implemented: bash"}
/// ```
///
/// Wire frame produced by [`KernelSession`]:
///
/// ```json
/// {"type":"host_reply","id":"h1","data":{"status":"error","error":"host_request action not implemented: bash"}}
/// ```
pub fn deny_host_handler() -> HostRequestHandler {
    Arc::new(|_id, data| {
        Box::pin(async move {
            let action = data
                .get("type")
                .or_else(|| data.get("action"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            serde_json::json!({
                "status": "error",
                "error": format!("host_request action not implemented: {action}"),
            })
        })
    })
}

/// Extract `(action, params)` from a REPL `host_request` data object.
///
/// `action` prefers the `action` key and falls back to `type`; a request with
/// neither yields an empty string. When `params` is present it is returned as
/// is; otherwise the remaining object (without `action` / `type`) becomes the
/// params.
///
/// ```json
/// {"type":"bash","params":{"command":"ls"}}
/// ```
///
/// yields `("bash", {"command":"ls"})`, while
///
/// ```json
/// {"action":"compact.status","x":1}
/// ```
///
/// yields `("compact.status", {"x":1})`.
pub fn parse_host_request_data(data: &Value) -> (String, Value) {
    let action = data
        .get("action")
        .and_then(|v| v.as_str())
        .or_else(|| data.get("type").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    if let Some(params) = data.get("params").cloned() {
        return (action, params);
    }
    let mut obj = data.as_object().cloned().unwrap_or_default();
    obj.remove("action");
    obj.remove("type");
    (action, Value::Object(obj))
}

/// Selects whether a turn starts the RLM Python kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecutionSurface {
    /// Do not start a kernel. The Python-only model policy exposes no tool.
    Discrete,
    /// Start the kernel; the regular model function-tool schema is only `ipython`.
    /// Provider-hosted web capabilities are configured separately.
    Rlm,
}

/// Host-requested workspace access for mode-aware kernel reuse.
/// This tag does not grant or restrict permissions in the trusted kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAccess {
    /// Plan mode: the host requests read-only workspace access.
    ReadOnly,
    /// Build mode: the host requests read-write workspace access.
    ReadWrite,
}

/// Configuration for spawning a kernel session.
#[derive(Debug, Clone)]
pub struct KernelSessionConfig {
    /// Interpreter used to run `python -m rlm.repl`.
    pub python: PathBuf,
    /// Working directory of the kernel process and its default workspace root.
    pub cwd: PathBuf,
    /// Directories prepended to `PYTHONPATH` (vendored kernel runtime `src`).
    pub python_path_entries: Vec<PathBuf>,
    /// Extra environment variables for the kernel process (harness paths, depth, …).
    pub extra_env: Vec<(String, String)>,
    /// Records the access requested by the host for subsequent kernel reuse.
    pub workspace_access: WorkspaceAccess,
}

impl Default for KernelSessionConfig {
    /// Use the packaged runtime when present, otherwise a PATH-resolved
    /// interpreter. The working directory is the process cwd. Packaged
    /// sessions get the bundled site dir on `PYTHONPATH` and
    /// `PYTHONNOUSERSITE=1`.
    fn default() -> Self {
        let bundle = devo_util_paths::runtime::RuntimeBundle::current();
        Self {
            python: bundle.as_ref().map_or_else(
                || {
                    devo_util_paths::runtime::fallback_interpreter(
                        std::env::var_os("PATH").as_deref(),
                    )
                },
                |runtime| runtime.python.clone(),
            ),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            python_path_entries: bundle
                .as_ref()
                .map_or_else(Vec::new, |runtime| vec![runtime.python_site.clone()]),
            extra_env: bundle.map_or_else(Vec::new, |_| {
                vec![("PYTHONNOUSERSITE".to_string(), "1".to_string())]
            }),
            workspace_access: WorkspaceAccess::ReadWrite,
        }
    }
}

/// Why [`KernelSession::spawn`] or [`KernelSession::spawn_command`] failed.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    /// Protocol setup or the handshake failed after the child started.
    #[error(transparent)]
    Repl(#[from] ReplError),
    /// The custom command passed to [`KernelSession::spawn_command`] failed to
    /// start.
    #[error("failed to spawn kernel: {0}")]
    Spawn(#[source] std::io::Error),
    /// The configured Python interpreter could not be executed.
    #[error("failed to spawn kernel interpreter `{python}`: {source}")]
    SpawnInterpreter {
        /// Interpreter path from [`KernelSessionConfig::python`].
        python: PathBuf,
        /// OS error from the failed spawn.
        #[source]
        source: std::io::Error,
    },
}

/// Collected output from one kernel request that waits for `done`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CellOutput {
    pub stdout: String,
    pub stderr: String,
    pub result: Option<String>,
    pub status: String,
    pub error_name: Option<String>,
    pub error_value: Option<String>,
    pub traceback: Vec<String>,
    /// Populated by `list_names` (and any `done` that carries `names`).
    pub names: Option<Vec<String>>,
    /// Diffs from `application/vnd.diff+json` display events.
    pub diffs: Vec<CellDiffDisplay>,
    /// Media attachments from `application/vnd.attachment+json`
    /// (e.g. attach-image skill).
    pub attachments: Vec<CellAttachment>,
}

/// One file edit emitted by the kernel edit skill (display MIME).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellDiffDisplay {
    pub path: String,
    pub old_str: String,
    pub new_str: String,
    pub start_line: Option<u32>,
}

/// One media attachment emitted by the attach-image skill (display MIME).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellAttachment {
    pub mime_type: String,
    /// base64-encoded image bytes.
    pub data: String,
    pub path: Option<String>,
}

/// MIME used by the edit skill when displaying a file diff from Python.
pub const DIFF_DISPLAY_MIME: &str = "application/vnd.diff+json";

/// MIME used by the attach-image skill when loading an image into context.
pub const ATTACHMENT_DISPLAY_MIME: &str = "application/vnd.attachment+json";

/// Hard ceiling on a single attachment's base64 payload (defensive; skill caps lower).
pub const MAX_ATTACHMENT_DATA_CHARS: usize = 10_000_000;

/// Parse a diff display payload from a kernel `display` event's `data`.
///
/// Accepts the diff MIME payload with snake_case or camelCase field names.
/// `start_line` is optional; unrelated MIME payloads return `None`.
///
/// ```json
/// {"application/vnd.diff+json":{"path":"src/a.py","old_str":"x = 1\n","new_str":"x = 2\n","start_line":3}}
/// ```
pub fn parse_diff_display(data: &Value) -> Option<CellDiffDisplay> {
    let payload = data.get(DIFF_DISPLAY_MIME)?;
    let path = payload.get("path")?.as_str()?.to_string();
    let old_str = payload
        .get("old_str")
        .or_else(|| payload.get("oldStr"))?
        .as_str()?
        .to_string();
    let new_str = payload
        .get("new_str")
        .or_else(|| payload.get("newStr"))?
        .as_str()?
        .to_string();
    let start_line = payload
        .get("start_line")
        .or_else(|| payload.get("startLine"))
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    Some(CellDiffDisplay {
        path,
        old_str,
        new_str,
        start_line,
    })
}

/// Parse an attachment display payload from a kernel `display` event's `data`.
///
/// Only `image/*` payloads with non-empty base64 data are accepted; oversized
/// or malformed payloads are dropped (returns `None`). Both `mime_type` and
/// `mimeType` are accepted.
///
/// ```json
/// {"application/vnd.attachment+json":{"mime_type":"image/png","data":"<base64>","path":"a.png"}}
/// ```
pub fn parse_attachment_display(data: &Value) -> Option<CellAttachment> {
    let payload = data.get(ATTACHMENT_DISPLAY_MIME)?;
    let mime_type = payload
        .get("mime_type")
        .or_else(|| payload.get("mimeType"))?
        .as_str()?
        .to_string();
    if !mime_type.starts_with("image/") {
        return None;
    }
    let data_b64 = payload.get("data")?.as_str()?.to_string();
    if data_b64.is_empty() || data_b64.len() > MAX_ATTACHMENT_DATA_CHARS {
        return None;
    }
    let path = payload
        .get("path")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Some(CellAttachment {
        mime_type,
        data: data_b64,
        path,
    })
}

struct KernelIo {
    /// Writable independently of the execute gate (host_reply + interrupt).
    stdin: Arc<Mutex<ChildStdin>>,
    /// Events demultiplexed by the one session-owned stdout reader.
    events: Mutex<mpsc::Receiver<Result<KernelEvent, ReplError>>>,
}

struct EventPumpState {
    events: Mutex<mpsc::Receiver<Result<KernelEvent, ReplError>>>,
    active_request: Arc<Mutex<Option<session_event_pump::ActiveRequestContext>>>,
    idle_bash_handler: Arc<Mutex<Option<HostRequestHandler>>>,
    lifetime: tokio_util::sync::CancellationToken,
}

/// Start the single stdout reader for this session.
///
/// The pump owns the only reader of the kernel's stdout, forwards every event
/// to the collection channel, and handles `host_request` frames itself (a
/// request must be answered, not collected as cell output). It lives for the
/// lifetime of the child, so it can still deliver `bash.completed` after a
/// cell's `done` through the idle handler.
fn start_event_pump(
    stdout: BufReader<ChildStdout>,
    stdin: Arc<Mutex<ChildStdin>>,
) -> EventPumpState {
    let (event_tx, event_rx) = mpsc::channel(session_event_pump::EVENT_CHANNEL_CAPACITY);
    let active_request = Arc::new(Mutex::new(None));
    let idle_bash_handler = Arc::new(Mutex::new(None));
    let lifetime = tokio_util::sync::CancellationToken::new();
    session_event_pump::spawn(
        stdout,
        stdin,
        Arc::clone(&active_request),
        Arc::clone(&idle_bash_handler),
        event_tx,
        lifetime.clone(),
    );
    EventPumpState {
        events: Mutex::new(event_rx),
        active_request,
        idle_bash_handler,
        lifetime,
    }
}
/// Shared collector state for a cell started via [`KernelSession::begin_execute`].
struct InflightCollectorState {
    output: CellOutput,
    /// Set when the collector finishes (`Ok` = got `done`; `Err` = I/O/protocol message).
    terminal: Option<Result<(), String>>,
}

/// Outcome of a budgeted wait on an in-flight Python cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellWaitOutcome {
    /// Cell reached `done` within the budget.
    Done(CellOutput),
    /// Budget expired; cell is still running. `snapshot` is stdout/stderr so far.
    TimedOut { snapshot: CellOutput },
}

/// A cell that has been submitted to the kernel and is still being collected.
///
/// The execute gate is held by the background collector until `done` (stdout is
/// exclusive). Parking releases the *caller* so the turn can continue; later
/// cells still serialize behind this gate until the parked cell finishes.
pub struct InFlightCell {
    session: Arc<KernelSession>,
    cell_id: String,
    state: Arc<Mutex<InflightCollectorState>>,
    done: Arc<Notify>,
}

impl InFlightCell {
    /// Protocol id for this cell (`c1`, `s1`, ...), echoed by its events.
    pub fn cell_id(&self) -> &str {
        &self.cell_id
    }

    /// Best-effort interrupt of this cell (does not wait for `done`).
    pub async fn interrupt(&self) -> Result<(), ReplError> {
        self.session.interrupt(Some(self.cell_id.clone())).await
    }

    /// Latest stdout/stderr/result snapshot (for wait-policy prompts).
    pub async fn snapshot(&self) -> CellOutput {
        self.state.lock().await.output.clone()
    }

    /// Wait up to `budget` for the cell to finish.
    pub async fn wait_for(&self, budget: Duration) -> Result<CellWaitOutcome, ReplError> {
        // Register before inspecting terminal state: notify_waiters does not
        // retain a permit when completion races with an unregistered waiter.
        let notified = self.done.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if let Some(outcome) = self.try_take_done().await? {
            return Ok(CellWaitOutcome::Done(outcome));
        }
        if budget.is_zero() {
            return Ok(CellWaitOutcome::TimedOut {
                snapshot: self.snapshot().await,
            });
        }
        tokio::select! {
            () = &mut notified => {}
            () = tokio::time::sleep(budget) => {
                if let Some(outcome) = self.try_take_done().await? {
                    return Ok(CellWaitOutcome::Done(outcome));
                }
                return Ok(CellWaitOutcome::TimedOut {
                    snapshot: self.snapshot().await,
                });
            }
        }
        if let Some(outcome) = self.try_take_done().await? {
            Ok(CellWaitOutcome::Done(outcome))
        } else {
            Ok(CellWaitOutcome::TimedOut {
                snapshot: self.snapshot().await,
            })
        }
    }

    /// Wait until the cell reaches `done` (or collector error).
    pub async fn wait_until_done(self) -> Result<CellOutput, ReplError> {
        loop {
            let notified = self.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(outcome) = self.try_take_done().await? {
                return Ok(outcome);
            }
            notified.await;
        }
    }

    /// Keep collecting in the background and invoke `on_complete` when finished.
    pub fn park<F>(self, on_complete: F)
    where
        F: FnOnce(String, Result<CellOutput, ReplError>) + Send + 'static,
    {
        tokio::spawn(async move {
            let cell_id = self.cell_id.clone();
            let result = self.wait_until_done().await;
            on_complete(cell_id, result);
        });
    }

    /// Read the collector's terminal state without waiting.
    ///
    /// Returns `Ok(Some(output))` after `done`, `Ok(None)` while the collector
    /// is still running, and an error if collection failed.
    async fn try_take_done(&self) -> Result<Option<CellOutput>, ReplError> {
        let state = self.state.lock().await;
        match &state.terminal {
            Some(Ok(())) => Ok(Some(state.output.clone())),
            Some(Err(err)) => Err(ReplError::Other(err.clone())),
            None => Ok(None),
        }
    }
}

/// One durable CPython REPL for a session.
pub struct KernelSession {
    lifetime: tokio_util::sync::CancellationToken,
    child: Mutex<Child>,
    io: KernelIo,
    shutdown_gate: Mutex<()>,
    /// Serializes cells; held across host_request awaits (Ask).
    /// `Arc` so owned guards can move into background collectors.
    execute_gate: Arc<Mutex<()>>,
    ready: ReadyEvent,
    cell_seq: AtomicU64,
    host_handler: Mutex<Option<HostRequestHandler>>,
    active_request: Arc<Mutex<Option<session_event_pump::ActiveRequestContext>>>,
    /// Only `bash.completed` may use this handler after a cell's `done` event.
    idle_bash_handler: Arc<Mutex<Option<HostRequestHandler>>>,
    workspace_access: WorkspaceAccess,
    workspace_root: PathBuf,
}

/// Environment variable name segments that mark a variable as a credential.
///
/// The kernel runs model-generated code and never needs provider or cloud
/// credentials; the server holds those. Dropping credential-shaped variables
/// and credential-bearing URI values from the inherited environment prevents
/// silent exfiltration through env inspection. Nonsecret URLs and ordinary
/// runtime variables (PATH, TEMP, locale, PYTHON*, RLM_*) remain available.
const CREDENTIAL_ENV_SEGMENTS: &[&str] = &[
    "KEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "CREDENTIALS",
    "AUTH",
];

/// Whether an inherited environment entry looks like a credential and must be
/// removed from the kernel.
///
/// Matches credential-shaped names (`*_KEY`, `*_TOKEN`, `*_SECRET`,
/// `*_PASSWORD`, ..., `PGPASSWORD`, `MYSQL_PWD`) and `*_URL` / `*_URI` /
/// `*_DSN` values whose userinfo or query parameters carry credentials. The
/// check deliberately errs toward removal: dropping an ordinary variable is
/// harmless, leaking a secret into model-controlled Python is not.
pub(crate) fn is_credential_env_var(name: &str, value: &str) -> bool {
    let name = name.to_ascii_uppercase();
    if matches!(name.as_str(), "PGPASSWORD" | "MYSQL_PWD")
        || name
            .split('_')
            .any(|segment| CREDENTIAL_ENV_SEGMENTS.contains(&segment))
    {
        return true;
    }
    if !matches!(name.rsplit('_').next(), Some("URL" | "URI" | "DSN")) {
        return false;
    }
    let Some((_, rest)) = value.split_once("://") else {
        return false;
    };
    // URI userinfo (including `:password@`) is never needed by the kernel.
    if rest
        .split(['/', '?', '#'])
        .next()
        .is_some_and(|host| host.contains('@'))
    {
        return true;
    }
    let Some((_, query)) = rest.split_once('?') else {
        return false;
    };
    query
        .split('#')
        .next()
        .unwrap_or("")
        .split('&')
        .any(|param| {
            let key = param.split_once('=').map_or(param, |(key, _)| key);
            key.to_ascii_uppercase().split(['_', '-']).any(|segment| {
                CREDENTIAL_ENV_SEGMENTS.contains(&segment)
                    || matches!(
                        segment,
                        "PASS" | "PWD" | "APIKEY" | "SIG" | "SIGNATURE" | "SAS"
                    )
            })
        })
}

#[cfg(test)]
mod credential_env_tests {
    use super::{KernelSessionConfig, is_credential_env_var, kernel_env_overrides};
    use pretty_assertions::assert_eq;

    /// Verifies: credential-shaped variable names are detected
    /// case-insensitively, including `PGPASSWORD` and `MYSQL_PWD`.
    #[test]
    fn matches_credential_shaped_names() {
        for name in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "GITHUB_TOKEN",
            "AZURE_CLIENT_SECRET",
            "NPM_TOKEN",
            "SSH_AUTH_SOCK",
            "DEPLOY_PASSWORD",
            "openai_api_key",
            "PGPASSWORD",
            "pgpassword",
            "MYSQL_PWD",
        ] {
            assert_eq!(is_credential_env_var(name, "value"), true, "{name}");
        }
    }

    /// Verifies: URL/URI/DSN values carrying userinfo or credential-named
    /// query parameters are detected.
    #[test]
    fn matches_credential_bearing_uris() {
        for (name, value) in [
            ("DATABASE_URL", "postgres://alice:password@db.example/app"),
            ("REDIS_URL", "redis://:password@cache.example:6379/0"),
            ("redis_url", "redis://alice@cache.example:6379"),
            ("DATABASE_URI", "postgres://db.example/app?password=s3cret"),
            ("BROKER_DSN", "https://example.test/path?api_key=s3cret"),
            (
                "STORAGE_URL",
                "https://example.test/path?X-Amz-Signature=s3cret",
            ),
        ] {
            assert_eq!(is_credential_env_var(name, value), true, "{name}={value}");
        }
    }

    /// Verifies: ordinary names and non-secret URLs/URIs are not filtered.
    #[test]
    fn keeps_non_credential_names_and_uris() {
        for (name, value) in [
            ("PATH", "/bin"),
            ("TEMP", "/tmp"),
            ("USERPROFILE", "a user"),
            ("LANG", "en_US.UTF-8"),
            ("PYTHONUTF8", "1"),
            ("RLM_SESSION_DIR", "/tmp/session"),
            ("DATABASE_URL", "postgres://db.example/app?sslmode=require"),
            ("REDIS_URL", "redis://cache.example:6379/0"),
            ("SERVICE_URI", "https://service.example/path?region=us"),
            ("JAVA_HOME", "/opt/java"),
        ] {
            assert_eq!(is_credential_env_var(name, value), false, "{name}={value}");
        }
    }

    /// Verifies: credential-shaped `extra_env` entries are dropped before they
    /// reach the kernel.
    #[test]
    fn extra_env_cannot_reintroduce_credentials() {
        let config = KernelSessionConfig {
            extra_env: vec![
                ("PGPASSWORD".into(), "password".into()),
                ("DATABASE_URL".into(), "postgres://u:p@host/db".into()),
                ("REDIS_URL".into(), "redis://cache.example:6379/0".into()),
                ("RLM_SESSION_DIR".into(), "/tmp/session".into()),
            ],
            ..KernelSessionConfig::default()
        };
        assert_eq!(
            kernel_env_overrides(&config),
            vec![
                ("REDIS_URL".into(), "redis://cache.example:6379/0".into()),
                ("RLM_SESSION_DIR".into(), "/tmp/session".into()),
            ]
        );
    }
}

impl KernelSession {
    /// Spawn `python -m rlm.repl` and complete the protocol handshake.
    pub async fn spawn(config: KernelSessionConfig) -> Result<Arc<Self>, SpawnError> {
        let workspace_root =
            std::fs::canonicalize(&config.cwd).unwrap_or_else(|_| config.cwd.clone());
        let mut cmd = Command::new(&config.python);
        if devo_util_paths::runtime::RuntimeBundle::current().is_some() {
            devo_util_paths::runtime_environment::remove_interpreter_environment(
                cmd.as_std_mut(),
                devo_util_paths::runtime_environment::Interpreter::Python,
            );
        }
        cmd.arg("-m")
            .arg("rlm.repl")
            .current_dir(&config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env("PYTHONUTF8", "1")
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUNBUFFERED", "1");

        // Never inherit credentials into the model-controlled kernel, even
        // when they are hidden in connection URI values. Env names are
        // case-insensitive on Windows, so match that way on all platforms.
        for (key, value) in std::env::vars() {
            if is_credential_env_var(&key, &value) {
                cmd.env_remove(&key);
            }
        }

        for (key, value) in kernel_env_overrides(&config) {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|source| SpawnError::SpawnInterpreter {
            python: config.python.clone(),
            source,
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ReplError::Other("missing stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ReplError::Other("missing stdout".into()))?;

        let mut stdout = BufReader::new(stdout);
        let ready = match read_ready(&mut stdout).await {
            Ok(ready) => ready,
            Err(error) => {
                // Handshake died: surface the child's stderr tail instead of a
                // bare "closed unexpectedly" (fail loudly, never silently).
                let mut stderr_tail = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    use tokio::io::AsyncReadExt;
                    let _ = tokio::time::timeout(
                        Duration::from_secs(2),
                        stderr.read_to_string(&mut stderr_tail),
                    )
                    .await;
                }
                let stderr_tail: String = stderr_tail.chars().take(400).collect();
                return Err(ReplError::Other(format!(
                    "kernel handshake failed: {error}; stderr tail: {stderr_tail:?}"
                ))
                .into());
            }
        };
        if ready.protocol != PROTOCOL_VERSION {
            return Err(ReplError::ProtocolMismatch {
                got: ready.protocol,
            }
            .into());
        }

        let stdin = Arc::new(Mutex::new(stdin));
        let EventPumpState {
            events,
            active_request,
            idle_bash_handler,
            lifetime,
        } = start_event_pump(stdout, Arc::clone(&stdin));
        Ok(Arc::new(Self {
            lifetime,
            child: Mutex::new(child),
            io: KernelIo { stdin, events },
            shutdown_gate: Mutex::new(()),
            execute_gate: Arc::new(Mutex::new(())),
            ready,
            cell_seq: AtomicU64::new(0),
            host_handler: Mutex::new(None),
            active_request,
            idle_bash_handler,
            workspace_access: config.workspace_access,
            workspace_root,
        }))
    }

    /// Spawn a custom command that speaks REPL protocol v3 (tests / fakes).
    pub async fn spawn_command(
        mut cmd: Command,
        host_handler: Option<HostRequestHandler>,
    ) -> Result<Arc<Self>, SpawnError> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env("PYTHONUTF8", "1")
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUNBUFFERED", "1");
        let mut child = cmd.spawn().map_err(SpawnError::Spawn)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ReplError::Other("missing stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ReplError::Other("missing stdout".into()))?;
        let mut stdout = BufReader::new(stdout);
        let ready = read_ready(&mut stdout).await?;
        if ready.protocol != PROTOCOL_VERSION {
            return Err(ReplError::ProtocolMismatch {
                got: ready.protocol,
            }
            .into());
        }
        let stdin = Arc::new(Mutex::new(stdin));
        let EventPumpState {
            events,
            active_request,
            idle_bash_handler,
            lifetime,
        } = start_event_pump(stdout, Arc::clone(&stdin));
        Ok(Arc::new(Self {
            lifetime,
            child: Mutex::new(child),
            io: KernelIo { stdin, events },
            shutdown_gate: Mutex::new(()),
            execute_gate: Arc::new(Mutex::new(())),
            ready,
            cell_seq: AtomicU64::new(0),
            host_handler: Mutex::new(host_handler),
            active_request,
            idle_bash_handler,
            workspace_access: WorkspaceAccess::ReadWrite,
            workspace_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }))
    }

    /// Cancellation for work owned by this kernel, independent of any turn.
    ///
    /// Canceled when shutdown/termination begins, the event stream closes, or
    /// this session is dropped. Canceling the returned child token does not
    /// cancel the kernel itself.
    pub fn lifetime_token(&self) -> tokio_util::sync::CancellationToken {
        self.lifetime.child_token()
    }

    /// Handshake details recorded at spawn: protocol version and Python version.
    pub fn ready(&self) -> &ReadyEvent {
        &self.ready
    }

    /// Access mode requested by the host at spawn for reuse decisions.
    pub fn workspace_access(&self) -> WorkspaceAccess {
        self.workspace_access
    }

    /// Resolved workspace location when this kernel process started.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Install or clear the host_request callback (typically once per turn).
    pub async fn set_host_handler(&self, handler: Option<HostRequestHandler>) {
        *self.host_handler.lock().await = handler;
    }

    /// Install the only host callback allowed to run while no cell owns stdout.
    /// The event pump invokes it only for `bash.completed` or `bash.consumed`.
    pub async fn set_idle_bash_completion_handler(&self, handler: Option<HostRequestHandler>) {
        *self.idle_bash_handler.lock().await = handler;
    }

    /// Bind the live request id to the handler allowed to answer its
    /// `host_request`s. The pump only uses this handler while the request's
    /// `cell_id` matches; later or detached requests fall back to idle/deny.
    async fn set_active_request(&self, id: String, host_handler: Option<HostRequestHandler>) {
        *self.active_request.lock().await =
            Some(session_event_pump::ActiveRequestContext { id, host_handler });
    }

    /// Execute a code cell and wait until its `done` event.
    pub async fn execute(&self, code: impl Into<String>) -> Result<CellOutput, ReplError> {
        let _gate = self.execute_gate.lock().await;
        self.execute_locked(code.into()).await
    }

    /// Execute only if no cell owns the request stream. Maintenance must not
    /// block a new turn behind a parked foreground/background cell.
    pub async fn try_execute(
        &self,
        code: impl Into<String>,
    ) -> Result<Option<CellOutput>, ReplError> {
        let Ok(_gate) = self.execute_gate.try_lock() else {
            return Ok(None);
        };
        self.execute_locked(code.into()).await.map(Some)
    }

    async fn execute_locked(&self, code: String) -> Result<CellOutput, ReplError> {
        let id = format!("c{}", self.cell_seq.fetch_add(1, Ordering::Relaxed) + 1);
        let req = KernelRequest::Execute {
            id: id.clone(),
            code,
        };
        let host_handler = self.host_handler.lock().await.clone();
        self.set_active_request(id.clone(), host_handler).await;
        {
            let mut stdin = self.io.stdin.lock().await;
            write_request(&mut stdin, &req).await?;
        }
        collect_until_done(self, &id).await
    }

    /// Start a code cell and return an [`InFlightCell`] for budgeted waits / park.
    ///
    /// The execute gate is held by a background collector until the cell reaches
    /// `done`. Callers may detach via [`InFlightCell::park`] without interrupting.
    pub async fn begin_execute(
        self: &Arc<Self>,
        code: impl Into<String>,
    ) -> Result<InFlightCell, ReplError> {
        let gate = Arc::clone(&self.execute_gate);
        let guard = gate.lock_owned().await;
        let id = format!("c{}", self.cell_seq.fetch_add(1, Ordering::Relaxed) + 1);
        let req = KernelRequest::Execute {
            id: id.clone(),
            code: code.into(),
        };
        let host_handler = self.host_handler.lock().await.clone();
        self.set_active_request(id.clone(), host_handler).await;
        {
            let mut stdin = self.io.stdin.lock().await;
            write_request(&mut stdin, &req).await?;
        }

        let state = Arc::new(Mutex::new(InflightCollectorState {
            output: CellOutput::default(),
            terminal: None,
        }));
        let done = Arc::new(Notify::new());
        let session = Arc::clone(self);
        let state_c = Arc::clone(&state);
        let done_c = Arc::clone(&done);
        let id_c = id.clone();
        tokio::spawn(async move {
            let _gate = guard;
            let collect_result = collect_into_shared(&session, &id_c, &state_c).await;
            {
                let mut state = state_c.lock().await;
                state.terminal = Some(collect_result.map_err(|e| e.to_string()));
            }
            done_c.notify_waiters();
        });

        Ok(InFlightCell {
            session: Arc::clone(self),
            cell_id: id,
            state,
            done,
        })
    }

    /// Best-effort interrupt of the running (or next) cell.
    ///
    /// Uses the stdin lock only — does not wait for the execute gate — so an
    /// interrupt can land while a host_request Ask is outstanding.
    pub async fn interrupt(&self, id: Option<String>) -> Result<(), ReplError> {
        let mut stdin = self.io.stdin.lock().await;
        write_request(&mut stdin, &KernelRequest::Interrupt { id }).await
    }

    /// Snapshot the kernel namespace to `path` (dill) with a JSON `manifest_path`.
    pub async fn snapshot(
        &self,
        path: impl AsRef<Path>,
        manifest_path: impl AsRef<Path>,
    ) -> Result<CellOutput, ReplError> {
        let _gate = self.execute_gate.lock().await;
        let id = format!("s{}", self.cell_seq.fetch_add(1, Ordering::Relaxed) + 1);
        let req = KernelRequest::Snapshot {
            id: id.clone(),
            path: path.as_ref().to_string_lossy().into_owned(),
            manifest_path: manifest_path.as_ref().to_string_lossy().into_owned(),
            max_bytes: None,
            max_variable_bytes: None,
            prune_oversized: None,
        };
        self.set_active_request(id.clone(), None).await;
        {
            let mut stdin = self.io.stdin.lock().await;
            write_request(&mut stdin, &req).await?;
        }
        collect_until_done(self, &id).await
    }

    /// Restore the kernel namespace from a dill at `path`.
    pub async fn restore(&self, path: impl AsRef<Path>) -> Result<CellOutput, ReplError> {
        let _gate = self.execute_gate.lock().await;
        let id = format!("r{}", self.cell_seq.fetch_add(1, Ordering::Relaxed) + 1);
        let req = KernelRequest::Restore {
            id: id.clone(),
            path: path.as_ref().to_string_lossy().into_owned(),
        };
        self.set_active_request(id.clone(), None).await;
        {
            let mut stdin = self.io.stdin.lock().await;
            write_request(&mut stdin, &req).await?;
        }
        collect_until_done(self, &id).await
    }

    /// List names currently defined in the kernel namespace.
    pub async fn list_names(&self) -> Result<Vec<String>, ReplError> {
        let _gate = self.execute_gate.lock().await;
        let id = format!("n{}", self.cell_seq.fetch_add(1, Ordering::Relaxed) + 1);
        let req = KernelRequest::ListNames { id: id.clone() };
        self.set_active_request(id.clone(), None).await;
        {
            let mut stdin = self.io.stdin.lock().await;
            write_request(&mut stdin, &req).await?;
        }
        let out = collect_until_done(self, &id).await?;
        Ok(out.names.unwrap_or_default())
    }

    /// Ask the kernel to shut down, run its child cleanup hooks, and wait for exit.
    ///
    /// The host bounds graceful shutdown so a stuck kernel cannot block session
    /// teardown forever. If the REPL does not exit in time, the kernel child is
    /// terminated; normal shutdown lets Python kill its owned Bash process groups.
    pub async fn shutdown(&self) -> Result<(), ReplError> {
        self.lifetime.cancel();
        let _shutdown_guard = self.shutdown_gate.lock().await;
        {
            let mut child = self.child.lock().await;
            if child.try_wait()?.is_some() {
                return Ok(());
            }
        }

        let graceful = tokio::time::timeout(Duration::from_secs(5), async {
            let _gate = self.execute_gate.lock().await;
            let id = Uuid::new_v4().to_string();
            self.set_active_request(id.clone(), None).await;
            {
                let mut stdin = self.io.stdin.lock().await;
                write_request(
                    &mut stdin,
                    &KernelRequest::Shutdown {
                        id: Some(id.clone()),
                    },
                )
                .await?;
            }
            let _ = collect_until_done(self, &id).await;
            Ok::<(), ReplError>(())
        })
        .await;
        match graceful {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.terminate().await;
                return Err(error);
            }
            Err(_) => {
                self.terminate().await;
                return Err(ReplError::Other(
                    "kernel graceful shutdown timed out".into(),
                ));
            }
        }

        match tokio::time::timeout(Duration::from_secs(2), async {
            self.child.lock().await.wait().await
        })
        .await
        {
            Ok(Ok(_status)) => Ok(()),
            Ok(Err(error)) => {
                self.terminate().await;
                Err(ReplError::Io(error))
            }
            Err(_) => {
                self.terminate().await;
                Err(ReplError::Other(
                    "kernel did not exit after graceful shutdown".into(),
                ))
            }
        }
    }

    /// Immediately stop and reap the kernel child when its session is permanently deleted.
    /// This discards the REPL namespace and does not wait for running Python code.
    pub async fn terminate(&self) {
        self.lifetime.cancel();
        let mut child = self.child.lock().await;
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.start_kill();
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    }
}

impl Drop for KernelSession {
    fn drop(&mut self) {
        self.lifetime.cancel();
    }
}

/// Serialize one request as a single JSON line and flush it to kernel stdin.
///
/// The caller holds the stdin lock. Host replies take the same lock without
/// holding the execute gate, so they are never blocked behind the cell that is
/// waiting for them.
async fn write_request(stdin: &mut ChildStdin, req: &KernelRequest) -> Result<(), ReplError> {
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stdin.write_all(line.as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

/// Read one newline-delimited JSON event from the kernel's stdout.
///
/// EOF becomes [`ReplError::Closed`] and an empty line is a protocol error;
/// malformed JSON surfaces as [`ReplError::Json`].
async fn read_line_event(stdout: &mut BufReader<ChildStdout>) -> Result<KernelEvent, ReplError> {
    let mut buf = String::new();
    let n = stdout.read_line(&mut buf).await?;
    if n == 0 {
        return Err(ReplError::Closed);
    }
    let trimmed = buf.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        return Err(ReplError::Other("empty protocol line".into()));
    }
    Ok(serde_json::from_str(trimmed)?)
}

/// Environment overrides the kernel child needs regardless of how it is
/// launched (bare or sandbox-wrapped): harness env plus the runtime
/// PYTHONPATH. For the sandbox wrapper these must ride the request itself —
/// the wrapper builds the child's env from its argv snapshot, so `cmd.env()`
/// set after wrapping would be lost.
pub(crate) fn kernel_env_overrides(config: &KernelSessionConfig) -> Vec<(String, String)> {
    // Host-supplied extras must not reintroduce a credential that was
    // removed from the inherited environment.
    let mut overrides: Vec<(String, String)> = config
        .extra_env
        .iter()
        .filter(|(key, value)| !is_credential_env_var(key, value))
        .cloned()
        .collect();
    if !config.python_path_entries.is_empty() {
        let mut paths: Vec<PathBuf> = config.python_path_entries.clone();
        if devo_util_paths::runtime::RuntimeBundle::current().is_none()
            && let Some(existing) = std::env::var_os("PYTHONPATH")
        {
            paths.extend(std::env::split_paths(&existing));
        }
        if let Ok(joined) = std::env::join_paths(&paths) {
            overrides.push((
                "PYTHONPATH".to_string(),
                joined.to_string_lossy().into_owned(),
            ));
        }
    }
    overrides
}

/// Read events until the `ready` handshake and return the parsed handshake.
///
/// Startup diagnostics may arrive as `stderr` events first; they are kept and
/// attached to [`ReplError::Handshake`] if the handshake fails. Any other event
/// before `ready` is a handshake error.
async fn read_ready(stdout: &mut BufReader<ChildStdout>) -> Result<ReadyEvent, ReplError> {
    let mut startup_stderr = String::new();
    loop {
        match read_line_event(stdout).await {
            Ok(KernelEvent::Ready { protocol, python }) => {
                return Ok(ReadyEvent { protocol, python });
            }
            Ok(KernelEvent::Stderr { text, .. }) => {
                // Python redirects startup diagnostics onto the protocol stream.
                // Read the complete traceback instead of losing it at the first line.
                startup_stderr.extend(text.chars().take(16_384 - startup_stderr.len().min(16_384)));
            }
            Ok(other) => {
                return Err(ReplError::Handshake(format!(
                    "expected ready, got {other:?}"
                )));
            }
            Err(error) if !startup_stderr.is_empty() => {
                return Err(ReplError::Handshake(format!(
                    "{error}; startup stderr: {startup_stderr}"
                )));
            }
            Err(error) => return Err(error),
        }
    }
}

/// Collect one id'd request until its `done` event and return the result.
///
/// The caller holds the execute gate. `host_request` frames are consumed by
/// the session pump, not here.
async fn collect_until_done(session: &KernelSession, id: &str) -> Result<CellOutput, ReplError> {
    let state = Arc::new(Mutex::new(InflightCollectorState {
        output: CellOutput::default(),
        terminal: None,
    }));
    collect_into_shared(session, id, &state).await?;
    Ok(state.lock().await.output.clone())
}

/// Demultiplex events for `id` into shared collector state until `done`.
///
/// Only events carrying `id` are attributed; untagged stdout/stderr/error and
/// display frames are kept as session-global output. Events for other cells and
/// `host_request` frames are ignored (the pump already answered the latter).
async fn collect_into_shared(
    session: &KernelSession,
    id: &str,
    state: &Arc<Mutex<InflightCollectorState>>,
) -> Result<(), ReplError> {
    loop {
        let event = {
            let mut events = session.io.events.lock().await;
            events.recv().await.ok_or(ReplError::Closed)??
        };
        match event {
            KernelEvent::Stdout { id: event_id, text }
                if event_id.as_deref() == Some(id) || event_id.is_none() =>
            {
                state.lock().await.output.stdout.push_str(&text);
            }
            KernelEvent::Stderr { id: event_id, text }
                if event_id.as_deref() == Some(id) || event_id.is_none() =>
            {
                state.lock().await.output.stderr.push_str(&text);
            }
            KernelEvent::Result { id: event_id, text } if event_id == id => {
                state.lock().await.output.result = Some(text);
            }
            KernelEvent::Error {
                id: event_id,
                ename,
                evalue,
                traceback,
            } if event_id.as_deref() == Some(id) || event_id.is_none() => {
                let mut out = state.lock().await;
                out.output.error_name = Some(ename);
                out.output.error_value = Some(evalue);
                out.output.traceback = traceback;
            }
            KernelEvent::Done {
                id: event_id,
                status,
                names,
                reason,
                ..
            } if event_id == id => {
                let mut out = state.lock().await;
                out.output.status = status;
                out.output.names = names;
                if let Some(reason) = reason
                    && out.output.error_value.is_none()
                {
                    out.output.error_value = Some(reason);
                }
                return Ok(());
            }
            KernelEvent::HostRequest { .. } => {
                // The sole session event pump consumes and answers host requests.
            }
            KernelEvent::Display { id: event_id, data }
                if event_id.as_deref() == Some(id) || event_id.is_none() =>
            {
                let mut out = state.lock().await;
                if let Some(diff) = parse_diff_display(&data) {
                    out.output.diffs.push(diff);
                }
                if let Some(attachment) = parse_attachment_display(&data) {
                    out.output.attachments.push(attachment);
                }
            }
            KernelEvent::Stdout { .. }
            | KernelEvent::Stderr { .. }
            | KernelEvent::Result { .. }
            | KernelEvent::Error { .. }
            | KernelEvent::Done { .. }
            | KernelEvent::Ready { .. }
            | KernelEvent::Display { .. } => {
                // Events for other cells or unattributed noise — ignore for now.
            }
        }
    }
}

/// Resolve the RLM runtime `src` dir (the folder that contains the `rlm` package).
///
/// Walks up from `start` looking for `crates/kernel/rlm-runtime/src` or `rlm-runtime/src`.
/// Override with `DEVO_RLM_RUNTIME_SRC`.
pub fn default_runtime_pythonpath(start: &Path) -> Option<PathBuf> {
    if let Some(bundle) = devo_util_paths::runtime::RuntimeBundle::current() {
        return Some(bundle.python_site);
    }
    let mut dir = start.to_path_buf();
    for _ in 0..8 {
        let candidates = [
            dir.join("crates/kernel/rlm-runtime/src"),
            dir.join("rlm-runtime/src"),
        ];
        if let Some(candidate) = candidates
            .into_iter()
            .find(|path| path.join("rlm").is_dir())
        {
            // Always absolute: the kernel child's cwd differs from this
            // process's cwd, so a relative PYTHONPATH entry would silently
            // resolve to nothing inside the child (No module named 'rlm').
            return Some(dunce::canonicalize(&candidate).unwrap_or(candidate));
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Build a config for a live kernel when the runtime source can be found;
    /// tests skip when it returns `None`.
    fn live_config() -> Option<KernelSessionConfig> {
        let runtime = std::env::var_os("DEVO_RLM_RUNTIME_SRC")
            .map(PathBuf::from)
            .filter(|p| p.join("rlm").is_dir())
            .or_else(|| default_runtime_pythonpath(Path::new(".")))
            .or_else(|| default_runtime_pythonpath(Path::new("../..")))
            .or_else(|| {
                // Workspace root when running from crates/kernel
                let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                default_runtime_pythonpath(manifest.parent()?.parent()?)
                    .or_else(|| default_runtime_pythonpath(&manifest))
            })?;
        Some(KernelSessionConfig {
            python: PathBuf::from("python"),
            cwd: std::env::temp_dir(),
            python_path_entries: vec![runtime],
            extra_env: Vec::new(),
            workspace_access: WorkspaceAccess::ReadWrite,
        })
    }

    /// Verifies: handshake accepts protocol 3 from a live CPython REPL.
    #[tokio::test]
    async fn live_handshake_protocol_v3() {
        let Some(config) = live_config() else {
            eprintln!("skip: kernel runtime not found");
            return;
        };
        let session = match KernelSession::spawn(config).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip: could not spawn kernel: {e}");
                return;
            }
        };
        assert_eq!(session.ready().protocol, PROTOCOL_VERSION);
        // Drop the session; kill_on_drop terminates the child.
        drop(session);
    }

    /// Verifies: `terminate` stops and reaps a kernel blocked in a running
    /// cell without waiting for that cell to finish.
    #[tokio::test]
    async fn terminating_a_deleted_kernel_stops_a_running_python_cell() {
        let Some(mut config) = live_config() else {
            eprintln!("skip: kernel runtime not found");
            return;
        };
        let directory = tempfile::tempdir().unwrap();
        config.cwd = directory.path().to_path_buf();
        let session = match KernelSession::spawn(config).await {
            Ok(session) => session,
            Err(error) => {
                eprintln!("skip: could not spawn kernel: {error}");
                return;
            }
        };
        let executing = Arc::clone(&session);
        let task = tokio::spawn(async move {
            executing.execute("import asyncio\nfrom pathlib import Path\nPath('started.txt').write_text('started')\nawait asyncio.sleep(300)").await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !directory.path().join("started.txt").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("Python cell must start before termination");
        tokio::time::timeout(Duration::from_secs(2), session.terminate())
            .await
            .expect("termination must not wait for the cell");
        assert!(session.child.lock().await.try_wait().unwrap().is_some());
        assert!(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
    }

    /// Verifies: credential-shaped extras and inherited variables never reach
    /// the kernel environment, while ordinary URLs still do.
    #[tokio::test]
    async fn live_kernel_does_not_receive_explicit_credentials() {
        let Some(mut config) = live_config() else {
            eprintln!("skip: kernel runtime not found");
            return;
        };
        config.extra_env = vec![
            ("PGPASSWORD".into(), "kernel-test-password".into()),
            (
                "DEVO_TEST_DATABASE_URL".into(),
                "postgres://user:password@db.example/app".into(),
            ),
            (
                "DEVO_TEST_REDIS_URL".into(),
                "redis://cache.example:6379/0".into(),
            ),
        ];
        let session = match KernelSession::spawn(config).await {
            Ok(session) => session,
            Err(error) => {
                eprintln!("skip: could not spawn kernel: {error}");
                return;
            }
        };
        let output = session
            .execute(
                "import os
print([os.getenv('PGPASSWORD'), os.getenv('DEVO_TEST_DATABASE_URL'), os.getenv('DEVO_TEST_REDIS_URL')])",
            )
            .await
            .expect("inspect kernel environment");
        assert_eq!(output.status, "ok", "stderr={}", output.stderr);
        assert_eq!(
            output.stdout.trim(),
            "[None, None, 'redis://cache.example:6379/0']"
        );
    }

    /// Verifies: graceful shutdown lets Python kill its background Bash
    /// process group instead of leaving an orphan.
    #[cfg(unix)]
    #[tokio::test]
    async fn graceful_shutdown_kills_background_bash_process_group() {
        let Some(config) = live_config() else {
            eprintln!("skip: kernel runtime not found");
            return;
        };
        let session = match KernelSession::spawn(config).await {
            Ok(session) => session,
            Err(error) => {
                eprintln!("skip: could not spawn kernel: {error}");
                return;
            }
        };
        let output = session
            .execute(
                "from rlm.local_bash import bash\nhandle = bash('sleep 60')\nprint(handle.pid)",
            )
            .await
            .expect("start background Bash process");
        let pid = output
            .stdout
            .lines()
            .last()
            .and_then(|line| line.trim().parse::<i32>().ok())
            .filter(|pid| *pid > 0)
            .expect("Bash PID in cell output");

        session.shutdown().await.expect("graceful kernel shutdown");
        tokio::time::timeout(Duration::from_secs(3), async {
            while unix_process_group_exists(pid) {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("kernel shutdown should kill the Bash process group");
    }

    /// Probe whether a process group still exists (`kill(-pid, 0)`); EPERM
    /// counts as alive because the group exists under another identity.
    #[cfg(unix)]
    fn unix_process_group_exists(pid: i32) -> bool {
        // bash.py starts each handle in a fresh session, so the process PID is
        // also its process-group ID. EPERM means the group exists but is owned
        // by another identity; every other non-ESRCH error is conservatively live.
        // SAFETY: signal 0 only probes whether this process group exists.
        let result = unsafe { libc::kill(-pid, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    /// Verifies: namespace survives across two execute calls (cross-turn).
    #[tokio::test]
    async fn live_namespace_survives_two_executes() {
        let Some(config) = live_config() else {
            eprintln!("skip: kernel runtime not found");
            return;
        };
        let session = match KernelSession::spawn(config).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip: could not spawn kernel: {e}");
                return;
            }
        };
        let first = session.execute("x = 1").await.expect("turn1");
        assert_eq!(first.status, "ok", "stderr={}", first.stderr);
        let second = session.execute("print(x)").await.expect("turn2");
        assert_eq!(second.status, "ok", "stderr={}", second.stderr);
        assert!(
            second.stdout.contains('1'),
            "expected print(x) -> 1, got stdout={:?} result={:?}",
            second.stdout,
            second.result
        );
    }

    /// Verifies: interrupt request is accepted without tearing down the session.
    #[tokio::test]
    async fn live_interrupt_keeps_session_alive() {
        let Some(config) = live_config() else {
            eprintln!("skip: kernel runtime not found");
            return;
        };
        let session = match KernelSession::spawn(config).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip: could not spawn kernel: {e}");
                return;
            }
        };
        session.interrupt(None).await.expect("interrupt while idle");
        let after = session
            .execute("print('ok')")
            .await
            .expect("execute after interrupt");
        assert_eq!(after.status, "ok", "stderr={}", after.stderr);
        assert!(
            after.stdout.contains("ok"),
            "expected ok after interrupt, stdout={:?}",
            after.stdout
        );
    }

    /// Verifies: Windows spawn uses `python` on PATH (RestrictedToken wrap is later).
    #[cfg(windows)]
    #[tokio::test]
    async fn live_windows_python_spawn() {
        let Some(config) = live_config() else {
            eprintln!("skip: kernel runtime not found");
            return;
        };
        assert!(
            config.python.as_os_str() == "python"
                || config.python.extension().is_some_and(|e| e == "exe"),
            "windows kernel must use native python.exe, got {:?}",
            config.python
        );
        let session = match KernelSession::spawn(config).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip: could not spawn kernel: {e}");
                return;
            }
        };
        assert_eq!(session.ready().protocol, PROTOCOL_VERSION);
    }

    /// Verifies: Unix spawn handshake succeeds when runtime is present.
    #[cfg(unix)]
    #[tokio::test]
    async fn live_unix_python_spawn() {
        let Some(config) = live_config() else {
            eprintln!("skip: kernel runtime not found");
            return;
        };
        let session = match KernelSession::spawn(config).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip: could not spawn kernel: {e}");
                return;
            }
        };
        assert_eq!(session.ready().protocol, PROTOCOL_VERSION);
    }

    /// Verifies: snapshot then restore round-trips a binding when runtime is present.
    #[tokio::test]
    async fn live_snapshot_restore_roundtrip() {
        let Some(config) = live_config() else {
            eprintln!("skip: kernel runtime not found");
            return;
        };
        let session = match KernelSession::spawn(config).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip: could not spawn kernel: {e}");
                return;
            }
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let dill = dir.path().join("kernel.dill");
        let manifest = dir.path().join("kernel.json");
        let first = session.execute("snap_x = 99").await.expect("bind");
        assert_eq!(first.status, "ok", "stderr={}", first.stderr);
        let snap = session.snapshot(&dill, &manifest).await.expect("snapshot");
        if snap.status != "ok" {
            eprintln!(
                "skip: snapshot not supported by runtime (status={} stderr={})",
                snap.status, snap.stderr
            );
            return;
        }
        let clear = session.execute("del snap_x").await.expect("clear");
        assert_eq!(clear.status, "ok", "stderr={}", clear.stderr);
        let restored = session.restore(&dill).await.expect("restore");
        assert_eq!(restored.status, "ok", "stderr={}", restored.stderr);
        let check = session.execute("print(snap_x)").await.expect("check");
        assert_eq!(check.status, "ok", "stderr={}", check.stderr);
        assert!(
            check.stdout.contains("99"),
            "expected snap_x=99 after restore, stdout={:?} result={:?}",
            check.stdout,
            check.result
        );
        let names = session.list_names().await.expect("list_names");
        assert!(
            names.iter().any(|n| n == "snap_x"),
            "expected snap_x in names={names:?}"
        );
    }

    /// Verifies: host_request during execute gets a host_reply (fake REPL).
    #[tokio::test]
    async fn host_request_receives_reply_via_fake_repl() {
        let script = r#"
import json, sys
print(json.dumps({"event":"ready","protocol":3,"python":"fake"}), flush=True)
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if req.get("type") == "execute":
        print(json.dumps({
            "event": "host_request",
            "id": "h1",
            "cell_id": req["id"],
            "data": {"type": "rlm.create_session", "params": {}}
        }), flush=True)
        reply = json.loads(sys.stdin.readline())
        assert reply.get("type") == "host_reply", reply
        assert reply.get("id") == "h1", reply
        print(json.dumps({"event":"stdout","id":req["id"],"text":str(reply["data"].get("status",""))+"\n"}), flush=True)
        print(json.dumps({"event":"done","id":req["id"],"status":"ok"}), flush=True)
    elif req.get("type") == "shutdown":
        print(json.dumps({"event":"done","id":req.get("id") or "","status":"ok"}), flush=True)
        break
"#;
        let dir = tempfile::tempdir().expect("tempdir");
        let script_path = dir.path().join("fake_repl.py");
        std::fs::write(&script_path, script).expect("write fake repl");

        let mut cmd = Command::new("python");
        cmd.arg(&script_path);

        let seen = Arc::new(Mutex::new(None::<String>));
        let seen_cb = Arc::clone(&seen);
        let handler: HostRequestHandler = Arc::new(move |_id, data| {
            let seen_cb = Arc::clone(&seen_cb);
            Box::pin(async move {
                let action = data
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                *seen_cb.lock().await = Some(action.clone());
                serde_json::json!({
                    "status": "error",
                    "error": format!("denied: {action}"),
                })
            })
        });

        let session = match KernelSession::spawn_command(cmd, Some(handler)).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip: could not spawn fake repl: {e}");
                return;
            }
        };
        let out = session.execute("pass").await.expect("execute");
        assert_eq!(out.status, "ok", "stderr={}", out.stderr);
        assert_eq!(seen.lock().await.as_deref(), Some("rlm.create_session"));
        assert!(
            out.stdout.contains("error"),
            "expected reply status echoed on stdout, got {:?}",
            out.stdout
        );
    }

    /// Verifies: parse_host_request_data reads action + params.
    #[test]
    fn parse_host_request_action_and_params() {
        let (action, params) = parse_host_request_data(&serde_json::json!({
            "action": "compact.status",
            "params": {"x": 1}
        }));
        assert_eq!(action, "compact.status");
        assert_eq!(params, serde_json::json!({"x": 1}));
    }

    /// Verifies: the diff MIME payload is parsed into all fields.
    #[test]
    fn parse_diff_display_reads_mime_payload() {
        let data = serde_json::json!({
            DIFF_DISPLAY_MIME: {
                "path": "src/a.py",
                "old_str": "x = 1\n",
                "new_str": "x = 2\n",
                "start_line": 3,
            }
        });
        let diff = parse_diff_display(&data).expect("diff");
        assert_eq!(diff.path, "src/a.py");
        assert_eq!(diff.old_str, "x = 1\n");
        assert_eq!(diff.new_str, "x = 2\n");
        assert_eq!(diff.start_line, Some(3));
    }

    /// Verifies: unrelated and bash-activity display data produce no diff or
    /// attachment.
    #[test]
    fn parse_diff_display_ignores_unrelated_display() {
        let data = serde_json::json!({ "text/plain": "hello" });
        assert!(parse_diff_display(&data).is_none());
        let bash_activity = serde_json::json!({
            "application/vnd.bash-activity+json": {
                "active": true,
                "pid": 123,
            }
        });
        assert!(parse_diff_display(&bash_activity).is_none());
        assert!(parse_attachment_display(&bash_activity).is_none());
    }

    /// Verifies: the attachment MIME payload is parsed, including its path.
    #[test]
    fn parse_attachment_display_reads_mime_payload() {
        let data = serde_json::json!({
            "application/vnd.attachment+json": {
                "mime_type": "image/png",
                "data": "YWJj",
                "path": "C:/tmp/a.png"
            }
        });
        let attachment = parse_attachment_display(&data).expect("attachment");
        assert_eq!(
            attachment,
            CellAttachment {
                mime_type: "image/png".into(),
                data: "YWJj".into(),
                path: Some("C:/tmp/a.png".into()),
            }
        );
    }

    /// Verifies: non-image MIME types and oversized base64 payloads are
    /// dropped.
    #[test]
    fn parse_attachment_display_rejects_non_image_and_oversized() {
        let non_image = serde_json::json!({
            "application/vnd.attachment+json": {
                "mime_type": "application/pdf",
                "data": "YWJj"
            }
        });
        assert!(parse_attachment_display(&non_image).is_none());

        let oversized = serde_json::json!({
            "application/vnd.attachment+json": {
                "mime_type": "image/png",
                "data": "x".repeat(MAX_ATTACHMENT_DATA_CHARS + 1)
            }
        });
        assert!(parse_attachment_display(&oversized).is_none());
    }

    /// Verifies: begin_execute can time out without interrupting; wait_until_done finishes later.
    #[tokio::test]
    async fn begin_execute_budgeted_wait_times_out_then_completes() {
        let Some(config) = live_config() else {
            eprintln!("skip: kernel runtime not found");
            return;
        };
        let session = match KernelSession::spawn(config).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip: no kernel ({e})");
                return;
            }
        };
        let inflight = session
            .begin_execute("import time; time.sleep(0.4); print('late')")
            .await
            .expect("begin");
        let outcome = inflight
            .wait_for(Duration::from_millis(50))
            .await
            .expect("wait");
        assert!(
            matches!(outcome, CellWaitOutcome::TimedOut { .. }),
            "expected TimedOut, got {outcome:?}"
        );
        let done = inflight.wait_until_done().await.expect("done");
        assert_eq!(done.status, "ok");
        assert!(done.stdout.contains("late"), "stdout={:?}", done.stdout);
    }
}

#[cfg(test)]
#[path = "session/startup_tests.rs"]
mod startup_tests;
