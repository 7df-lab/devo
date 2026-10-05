//! Ask-wired host bridge for kernel `host_request` mutating actions.
//!
//! Owns the session/turn capability context (`PermissionChecker`, FS, MCP) and
//! runs bash/write/edit/mcp/web through the same authorize → execute path as tools.
//! Goal / agent_message actions upgrade a [`Weak`][`std::sync::Weak`] to
//! [`ServerRuntime`] when available.

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::ServerRuntime;
use devo_core::McpManager;
use devo_core::tools::contracts::ToolBudgets;
use devo_core::tools::handlers::{
    EditHandler, PlanHandler, QuestionHandler, ShellCommandHandler, WebFetchHandler,
    WebSearchHandler, WriteHandler,
};
use devo_core::tools::{
    ClientFilesystem, FileReadLedger, PermissionChecker, SandboxPermissionRequest, ToolAgentScope,
    ToolCallError, ToolCallId, ToolContext, ToolHandler, ToolPermissionRequest, ToolResult,
    ToolResultContent, ToolTerminalStatus,
};
use devo_protocol::CollaborationMode;
use devo_protocol::InputModality;
use devo_protocol::native::ids::{SessionId, TurnId};
use devo_safety::ResourceKind;

const LOCAL_WEB_SEARCH_CONFIG_KEY: &str = "__devo_local_web_search";

/// Per-turn host capability context shared by kernel `host_request` handlers.
pub(crate) struct HostBridge {
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub cwd: PathBuf,
    /// Session artifact directory (parent of rollout JSONL); harness local store root.
    pub session_dir: Option<PathBuf>,
    pub collaboration_mode: CollaborationMode,
    pub permission: PermissionChecker,
    pub client_filesystem: Option<Arc<dyn ClientFilesystem>>,
    pub file_read_ledger: Arc<FileReadLedger>,
    pub sandbox_profile: Option<String>,
    pub cancel_token: CancellationToken,
    pub mcp_manager: Option<Arc<dyn McpManager>>,
    pub agent_scope: ToolAgentScope,
    pub local_web_search: Option<Value>,
    pub network_proxy: Option<String>,
    pub network_no_proxy: Option<String>,
    /// Weak to avoid Arc cycles (runtime → session → kernel → handler → runtime).
    pub runtime: Weak<ServerRuntime>,
    /// Current turn model id (wire / catalog id) for `model.info`.
    pub model_id: Option<String>,
    /// Input modalities for `model.info` / attach-image vision gate.
    pub input_modalities: Vec<InputModality>,
    pub context_window: Option<u32>,
}

impl HostBridge {
    /// Minimal always-allow bridge for unit tests / session-id-only handlers.
    pub(crate) fn for_tests(session_id: impl Into<SessionId>) -> Self {
        Self {
            session_id: session_id.into(),
            turn_id: None,
            cwd: std::env::temp_dir(),
            session_dir: None,
            collaboration_mode: CollaborationMode::Build,
            permission: PermissionChecker::always_allow(),
            client_filesystem: None,
            file_read_ledger: Arc::new(FileReadLedger::new()),
            sandbox_profile: None,
            cancel_token: CancellationToken::new(),
            mcp_manager: None,
            agent_scope: ToolAgentScope::Parent,
            local_web_search: None,
            network_proxy: None,
            network_no_proxy: None,
            runtime: Weak::new(),
            model_id: None,
            input_modalities: vec![InputModality::Text],
            context_window: None,
        }
    }

    pub(crate) fn runtime(&self) -> Option<Arc<ServerRuntime>> {
        self.runtime.upgrade()
    }

    /// Payload for kernel `host_request("model.info")` (attach-image vision gate).
    pub(crate) fn model_info_payload(&self) -> Value {
        let mut input = Vec::new();
        if self.input_modalities.is_empty() || self.input_modalities.contains(&InputModality::Text)
        {
            input.push("text");
        }
        let vision = self.input_modalities.contains(&InputModality::Image);
        if vision {
            input.push("image");
        }
        json!({
            "id": self.model_id.clone().unwrap_or_else(|| "unknown".into()),
            "input": input,
            "vision": vision,
            "context_window": self.context_window,
        })
    }
}

/// Build a [`devo_kernel::HostRequestHandler`] bound to a live [`HostBridge`].
pub fn host_handler_for_bridge(bridge: Arc<HostBridge>) -> devo_kernel::HostRequestHandler {
    Arc::new(move |_req_id, data| {
        let bridge = Arc::clone(&bridge);
        Box::pin(async move {
            let reply =
                crate::runtime::kernel_host::dispatch_host_request_with_bridge(&bridge, data).await;
            crate::runtime::kernel_host::ensure_host_reply_envelope(reply)
        })
    })
}

pub(crate) fn tool_context_from_bridge(bridge: &HostBridge, tool_call_id: String) -> ToolContext {
    ToolContext {
        output_store: None,
        tool_call_id: ToolCallId(tool_call_id),
        session_id: bridge.session_id,
        turn_id: bridge.turn_id,
        workspace_root: bridge.cwd.clone(),
        budgets: ToolBudgets {
            output_limit_bytes: 32 * 1024,
            wall_time_limit_ms: Some(6_000),
        },
        cancel_token: bridge.cancel_token.clone(),
        agent_scope: bridge.agent_scope,
        collaboration_mode: bridge.collaboration_mode,
        agent_coordinator: bridge
            .runtime()
            .map(|rt| rt as Arc<dyn devo_core::tools::AgentToolCoordinator>),
        client_filesystem: bridge.client_filesystem.clone(),
        file_read_ledger: Some(Arc::clone(&bridge.file_read_ledger)),
        network_proxy: bridge.network_proxy.clone(),
        network_no_proxy: bridge.network_no_proxy.clone(),
        sandbox_profile: bridge.sandbox_profile.clone(),
        sandbox_permission_overlay: None,
        kernel: None,
        python_cell_first_wait_ms: None,
        python_cell_watch: None,
        python_cell_completion: None,
        session_dir: bridge.session_dir.clone(),
    }
}

fn plan_mode_denies_mutation(bridge: &HostBridge) -> bool {
    matches!(bridge.collaboration_mode, CollaborationMode::Plan)
}

fn deny_plan_mutation(action: &str) -> Value {
    json!({
        "status": "error",
        "error": format!("denied: {action} blocked in Plan mode"),
    })
}

fn error_reply(msg: impl Into<String>) -> Value {
    json!({ "status": "error", "error": msg.into() })
}

fn synthetic_tool_call_id(prefix: &str) -> String {
    format!("host_{prefix}_{}", Uuid::new_v4())
}

#[allow(clippy::too_many_arguments)]
fn permission_request(
    bridge: &HostBridge,
    tool_call_id: String,
    tool_name: &str,
    input: Value,
    resource: ResourceKind,
    action_summary: String,
    path: Option<PathBuf>,
    target: Option<String>,
) -> ToolPermissionRequest {
    permission_request_with_host(
        bridge,
        tool_call_id,
        tool_name,
        input,
        resource,
        action_summary,
        path,
        /*host*/ None,
        target,
    )
}

#[allow(clippy::too_many_arguments)]
fn permission_request_with_host(
    bridge: &HostBridge,
    tool_call_id: String,
    tool_name: &str,
    input: Value,
    resource: ResourceKind,
    action_summary: String,
    path: Option<PathBuf>,
    host: Option<String>,
    target: Option<String>,
) -> ToolPermissionRequest {
    ToolPermissionRequest {
        tool_call_id,
        tool_name: tool_name.to_string(),
        input,
        cwd: bridge.cwd.clone(),
        session_id: bridge.session_id,
        turn_id: bridge.turn_id,
        resource,
        action_summary,
        justification: None,
        path,
        host,
        target,
        command_prefix: None,
        command_argv: None,
        command_pattern: None,
        sandbox_permissions: SandboxPermissionRequest::Default,
    }
}

fn host_from_url(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = url.split("://").nth(1).unwrap_or(url);
    let hostport = rest.split('/').next()?.split('@').next_back()?;
    let host = hostport.split(':').next()?.trim();
    (!host.is_empty()).then(|| host.to_string())
}

async fn check_or_error(bridge: &HostBridge, req: ToolPermissionRequest) -> Result<(), Value> {
    match bridge.permission.check(req).await {
        Ok(_) => Ok(()),
        Err(err) => Err(error_reply(err)),
    }
}

fn path_from_write_input(cwd: &Path, input: &Value) -> Option<PathBuf> {
    let raw = input
        .get("filePath")
        .or_else(|| input.get("path"))
        .or_else(|| input.get("file_path"))
        .and_then(|v| v.as_str())?;
    let path = PathBuf::from(raw);
    Some(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

fn tool_result_to_reply(result: ToolResult) -> Value {
    let (status, error_msg) = match &result.structured_status {
        ToolTerminalStatus::Completed => ("ok", None),
        ToolTerminalStatus::Denied { reason } | ToolTerminalStatus::BlockedByMode { reason } => {
            ("error", Some(reason.clone()))
        }
        ToolTerminalStatus::NeedsConfiguration { message } => ("error", Some(message.clone())),
        ToolTerminalStatus::InvalidInput { details } => ("error", Some(details.clone())),
        ToolTerminalStatus::Failed(err) => ("error", Some(err.to_string())),
        ToolTerminalStatus::Canceled | ToolTerminalStatus::Interrupted => {
            ("error", Some("cancelled".to_string()))
        }
    };
    let mut body = json!({
        "status": status,
        "summary": result.result_summary,
    });
    if let Some(msg) = error_msg {
        body["error"] = json!(msg);
    }
    if let Some(display) = result.display_content {
        body["display"] = json!(display);
    }
    match result.content {
        ToolResultContent::Text(text) => {
            body["stdout"] = json!(text);
        }
        ToolResultContent::Json(value) => {
            body["result"] = value;
        }
        ToolResultContent::Mixed { text, json: meta } => {
            if let Some(text) = text {
                body["stdout"] = json!(text);
            }
            if let Some(meta) = meta {
                if let Some(obj) = meta.as_object() {
                    for (key, value) in obj {
                        if body.get(key).is_none() {
                            body[key] = value.clone();
                        }
                    }
                } else {
                    body["result"] = meta;
                }
            }
        }
    }
    body
}

fn map_tool_outcome(result: Result<ToolResult, ToolCallError>) -> Value {
    match result {
        Ok(tool_result) => tool_result_to_reply(tool_result),
        Err(err) => error_reply(err.to_string()),
    }
}

pub(crate) async fn handle_bash(bridge: &HostBridge, params: &Value) -> Value {
    if plan_mode_denies_mutation(bridge) {
        return deny_plan_mutation("bash");
    }
    let command = params
        .get("command")
        .or_else(|| params.get("cmd"))
        .cloned()
        .unwrap_or(Value::Null);
    let command_str = command.as_str().unwrap_or("").to_string();
    if command_str.is_empty() {
        return error_reply("missing bash command");
    }
    let mut input = params.clone();
    if input.get("command").is_none() {
        input["command"] = json!(command_str.clone());
    }
    let tool_call_id = synthetic_tool_call_id("bash");
    let req = permission_request(
        bridge,
        tool_call_id.clone(),
        "shell_command",
        input.clone(),
        ResourceKind::ShellExec,
        format!("Run shell command: {command_str}"),
        None,
        Some(command_str),
    );
    let grant = match check_or_grant(bridge, req).await {
        Ok(grant) => grant,
        Err(reply) => return reply,
    };
    let ctx = tool_context_from_bridge(bridge, tool_call_id.clone());
    let outcome = ShellCommandHandler::new()
        .handle(ctx, input.clone(), None)
        .await;
    // UnlessTrusted retry (mirrors `ToolRuntime::execute_single`): a
    // user-approved shell call the OS sandbox refused retries once unsandboxed
    // — without this, kernel `bash` approvals visibly succeed while the
    // command still fails with EACCES. Policy-allows (no approval) keep the
    // denial so the model re-requests with `require_escalated`.
    let outcome = match outcome {
        Ok(ref output)
            if grant.already_approved
                && !grant.bypass_sandbox
                && tool_result_is_sandbox_denied_bridge(output)
                && devo_sandbox::unsandboxed_execution_allowed(
                    bridge.sandbox_profile.as_deref(),
                    &bridge.cwd,
                ) =>
        {
            let mut ctx = tool_context_from_bridge(bridge, tool_call_id);
            ctx.sandbox_profile = Some("off".to_string());
            ShellCommandHandler::new().handle(ctx, input, None).await
        }
        other => other,
    };
    map_tool_outcome(outcome)
}

/// Bridge twin of the router's `tool_result_is_sandbox_denied` (private there):
/// only the structured `SANDBOX_DENIED:` prefix triggers the retry.
fn tool_result_is_sandbox_denied_bridge(output: &ToolResult) -> bool {
    let text = match &output.content {
        devo_core::tools::contracts::ToolResultContent::Text(text) => text.as_str(),
        devo_core::tools::contracts::ToolResultContent::Mixed {
            text: Some(text), ..
        } => text.as_str(),
        devo_core::tools::contracts::ToolResultContent::Json(_)
        | devo_core::tools::contracts::ToolResultContent::Mixed { text: None, .. } => "",
    };
    text.starts_with("SANDBOX_DENIED:") || output.result_summary.starts_with("SANDBOX_DENIED:")
}

async fn check_or_grant(
    bridge: &HostBridge,
    req: ToolPermissionRequest,
) -> Result<devo_core::tools::PermissionGrant, Value> {
    match bridge.permission.check(req).await {
        Ok(grant) => Ok(grant),
        Err(err) => Err(error_reply(err)),
    }
}

pub(crate) async fn handle_write(bridge: &HostBridge, params: &Value) -> Value {
    if plan_mode_denies_mutation(bridge) {
        return deny_plan_mutation("write");
    }
    let input = params.clone();
    let path = path_from_write_input(&bridge.cwd, &input);
    let tool_call_id = synthetic_tool_call_id("write");
    let summary = path
        .as_ref()
        .map(|p| format!("Write file {}", p.display()))
        .unwrap_or_else(|| "Write file".to_string());
    let req = permission_request(
        bridge,
        tool_call_id.clone(),
        "write",
        input.clone(),
        ResourceKind::FileWrite,
        summary,
        path,
        None,
    );
    if let Err(reply) = check_or_error(bridge, req).await {
        return reply;
    }
    let ctx = tool_context_from_bridge(bridge, tool_call_id);
    map_tool_outcome(WriteHandler::new().handle(ctx, input, None).await)
}

pub(crate) async fn handle_edit(bridge: &HostBridge, params: &Value) -> Value {
    if plan_mode_denies_mutation(bridge) {
        return deny_plan_mutation("edit");
    }
    let input = params.clone();
    let path = path_from_write_input(&bridge.cwd, &input);
    let tool_call_id = synthetic_tool_call_id("edit");
    let summary = path
        .as_ref()
        .map(|p| format!("Edit file {}", p.display()))
        .unwrap_or_else(|| "Edit file".to_string());
    let req = permission_request(
        bridge,
        tool_call_id.clone(),
        "edit",
        input.clone(),
        ResourceKind::FileWrite,
        summary,
        path,
        None,
    );
    if let Err(reply) = check_or_error(bridge, req).await {
        return reply;
    }
    let ctx = tool_context_from_bridge(bridge, tool_call_id);
    map_tool_outcome(EditHandler::new().handle(ctx, input, None).await)
}

/// Pin the exact file before awaiting an interactive permission decision. Never
/// reopen by path after approval: the caller may replace any path component
/// while the approval prompt is pending.
#[cfg(unix)]
fn same_file_identity(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(windows)]
fn same_file_identity(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    match (
        a.volume_serial_number(),
        a.file_index(),
        b.volume_serial_number(),
        b.file_index(),
    ) {
        (Some(a_volume), Some(a_index), Some(b_volume), Some(b_index)) => {
            a_volume == b_volume && a_index == b_index
        }
        // Identity unavailable: fail closed rather than trust a mutable path.
        _ => false,
    }
}

#[cfg(not(any(unix, windows)))]
fn same_file_identity(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    false
}

fn open_mediated_read(path: &Path) -> std::io::Result<(PathBuf, File)> {
    let canonical = std::fs::canonicalize(path)?;
    // Capture the inode *before* the open, so a regular-file replacement
    // between canonicalization and open cannot be approved under the old path.
    let expected = std::fs::metadata(&canonical)?;
    let mut options = OpenOptions::new();
    options.read(true);
    // A final-component symlink inserted after canonicalization must not be
    // followed. The identity checks below also catch parent-path swaps.
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = options.open(&canonical)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::other(
            "fs.read target is not a regular file",
        ));
    }
    // Make sure the opened inode still corresponds to the path for which
    // permission will be requested. A file rename *after* these checks is
    // safe: the open descriptor, not the mutable pathname, is read.
    if std::fs::canonicalize(path)? != canonical {
        return Err(std::io::Error::other("fs.read target changed during open"));
    }
    let current = std::fs::metadata(&canonical)?;
    if !same_file_identity(&expected, &metadata) || !same_file_identity(&metadata, &current) {
        return Err(std::io::Error::other("fs.read target changed during open"));
    }
    Ok((canonical, file))
}

const MAX_MEDIATED_READ_BYTES: u64 = 1024 * 1024;

fn read_mediated_bytes(file: File, canonical: &Path, operation: &str) -> Result<Vec<u8>, Value> {
    let meta = file
        .metadata()
        .map_err(|err| error_reply(format!("{operation} {}: {err}", canonical.display())))?;
    if meta.len() > MAX_MEDIATED_READ_BYTES {
        return Err(error_reply(format!(
            "{operation} {}: file is {} bytes; mediated read limit is {MAX_MEDIATED_READ_BYTES} bytes",
            canonical.display(),
            meta.len()
        )));
    }
    // A writer could extend the file after metadata(). Bound the actual read
    // as well so neither read mode can return arbitrarily large host replies.
    let mut bytes = Vec::new();
    file.take(MAX_MEDIATED_READ_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| error_reply(format!("{operation} {}: {err}", canonical.display())))?;
    if bytes.len() as u64 > MAX_MEDIATED_READ_BYTES {
        return Err(error_reply(format!(
            "{operation} {}: mediated read limit is {MAX_MEDIATED_READ_BYTES} bytes",
            canonical.display()
        )));
    }
    Ok(bytes)
}

/// Front-door single-file read (design doc §6.1/§7): the Python facade tries a
/// direct `open()` first and only reaches this handler when the OS fence
/// refused (or the caller asked explicitly). Reads stay allowed in Plan mode.
/// The approval target is the canonicalized absolute path; a 1 MiB cap keeps
/// the mediated reply bounded.
pub(crate) async fn handle_fs_read(bridge: &HostBridge, params: &Value) -> Value {
    let Some(path_str) = params.get("path").and_then(Value::as_str) else {
        return error_reply("missing fs.read path");
    };
    if params.get("once").and_then(Value::as_bool).unwrap_or(false) {
        return handle_fs_read_once(bridge, params, path_str).await;
    }
    let raw = Path::new(path_str);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        bridge.cwd.join(raw)
    };
    let (canonical, file) = match open_mediated_read(&joined) {
        Ok(file) => file,
        Err(err) => return error_reply(format!("fs.read {}: {err}", joined.display())),
    };
    let tool_call_id = synthetic_tool_call_id("fs.read");
    let req = permission_request(
        bridge,
        tool_call_id,
        "read",
        json!({ "path": canonical.to_string_lossy() }),
        ResourceKind::FileRead,
        format!("Read file {}", canonical.display()),
        Some(canonical.clone()),
        None,
    );
    if let Err(reply) = check_or_error(bridge, req).await {
        return reply;
    }
    let bytes = match read_mediated_bytes(file, &canonical, "fs.read") {
        Ok(bytes) => bytes,
        Err(reply) => return reply,
    };
    let n = bytes.len();
    let content = String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    json!({ "status": "ok", "result": { "content": content, "bytes": n } })
}

/// once = ephemeral single-file credential (design doc §6.1/§9): approve the
/// read once, then return the bytes to the caller. The opened descriptor pins
/// the approved target across the potentially long approval wait.
async fn handle_fs_read_once(bridge: &HostBridge, _params: &Value, path_str: &str) -> Value {
    let raw = Path::new(path_str);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        bridge.cwd.join(raw)
    };
    let (canonical, file) = match open_mediated_read(&joined) {
        Ok(file) => file,
        Err(err) => return error_reply(format!("fs.read(once) {}: {err}", joined.display())),
    };
    let tool_call_id = synthetic_tool_call_id("fs.read.once");
    let req = permission_request(
        bridge,
        tool_call_id,
        "read",
        json!({ "path": canonical.to_string_lossy(), "once": true }),
        devo_safety::ResourceKind::FileRead,
        format!("Read file once {}", canonical.display()),
        Some(canonical.clone()),
        None,
    );
    if let Err(reply) = check_or_error(bridge, req).await {
        return reply;
    }
    let bytes = match read_mediated_bytes(file, &canonical, "fs.read(once)") {
        Ok(bytes) => bytes,
        Err(reply) => return reply,
    };
    let n = bytes.len();
    let content = String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    json!({ "status": "ok", "result": { "content": content, "bytes": n, "once": true } })
}

pub(crate) async fn handle_mcp_call(bridge: &HostBridge, params: &Value) -> Value {
    if plan_mode_denies_mutation(bridge) {
        return deny_plan_mutation("mcp.call");
    }
    let Some(manager) = bridge.mcp_manager.as_ref() else {
        return error_reply("mcp manager not available on this host bridge");
    };
    let server = params
        .get("server")
        .or_else(|| params.get("serverId"))
        .or_else(|| params.get("server_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool_name = params
        .get("tool")
        .or_else(|| params.get("toolName"))
        .or_else(|| params.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if server.is_empty() || tool_name.is_empty() {
        return error_reply("mcp.call requires server and tool");
    }
    let input = params
        .get("arguments")
        .or_else(|| params.get("input"))
        .or_else(|| params.get("args"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let tool_call_id = synthetic_tool_call_id("mcp");
    let req = permission_request(
        bridge,
        tool_call_id,
        "mcp.call",
        json!({
            "server": server,
            "tool": tool_name,
            "arguments": input,
        }),
        ResourceKind::Custom("mcp.call".into()),
        format!("Call MCP tool {server}/{tool_name}"),
        None,
        Some(format!("{server}/{tool_name}")),
    );
    if let Err(reply) = check_or_error(bridge, req).await {
        return reply;
    }
    let server_id = devo_core::McpServerId(server.to_string());
    match manager.invoke_tool(&server_id, tool_name, input).await {
        Ok(result) => json!({
            "status": "ok",
            "result": result,
        }),
        Err(err) => error_reply(err.to_string()),
    }
}

pub(crate) async fn handle_mcp_list_tools(bridge: &HostBridge, params: &Value) -> Value {
    let Some(server) = params
        .get("server")
        .and_then(Value::as_str)
        .filter(|server| !server.is_empty())
    else {
        return error_reply("mcp.list_tools requires server");
    };
    // Listing is read-only; skip Ask. Refreshing one configured server lists its
    // tools once and avoids discovering every server for this per-server API.
    let Some(manager) = bridge.mcp_manager.as_ref() else {
        return error_reply("mcp manager not available on this host bridge");
    };
    let server_id = devo_core::McpServerId(server.to_string());
    match manager.refresh(&server_id).await {
        Ok(status) => {
            let tools: Vec<Value> = status
                .tools
                .into_iter()
                .filter(|tool| tool.server_id == server_id)
                .map(|tool| {
                    json!({
                        "server": tool.server_id.0,
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": tool.input_schema,
                    })
                })
                .collect();
            json!({ "status": "ok", "result": { "tools": tools } })
        }
        Err(err) => error_reply(err.to_string()),
    }
}

pub(crate) async fn handle_web_search(bridge: &HostBridge, params: &Value) -> Value {
    let query = params
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if query.is_empty() {
        return error_reply("web.search requires query");
    }
    let Some(config) = bridge.local_web_search.as_ref() else {
        return error_reply("local web_search provider is not configured for this turn");
    };
    let mut input = params.clone();
    input["query"] = json!(query.clone());
    input[LOCAL_WEB_SEARCH_CONFIG_KEY] = config.clone();
    let tool_call_id = synthetic_tool_call_id("web_search");
    let req = permission_request_with_host(
        bridge,
        tool_call_id.clone(),
        "web_search",
        input.clone(),
        ResourceKind::Network,
        format!("Web search: {query}"),
        None,
        Some("web_search".to_string()),
        Some(query),
    );
    if let Err(reply) = check_or_error(bridge, req).await {
        return reply;
    }
    let ctx = tool_context_from_bridge(bridge, tool_call_id);
    map_tool_outcome(WebSearchHandler::new().handle(ctx, input, None).await)
}

pub(crate) async fn handle_web_fetch(bridge: &HostBridge, params: &Value) -> Value {
    let url = params
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if url.is_empty() {
        return error_reply("web.fetch requires url");
    }
    let input = params.clone();
    let tool_call_id = synthetic_tool_call_id("webfetch");
    let host = host_from_url(&url);
    let req = permission_request_with_host(
        bridge,
        tool_call_id.clone(),
        "webfetch",
        input.clone(),
        ResourceKind::Network,
        format!("Web fetch: {url}"),
        None,
        host,
        Some(url),
    );
    if let Err(reply) = check_or_error(bridge, req).await {
        return reply;
    }
    let ctx = tool_context_from_bridge(bridge, tool_call_id);
    map_tool_outcome(WebFetchHandler::new().handle(ctx, input, None).await)
}

pub(crate) async fn handle_plan_update(bridge: &HostBridge, params: &Value) -> Value {
    let Some(turn_id) = bridge.turn_id else {
        return error_reply("plan.update requires an active turn");
    };
    let Some(runtime) = bridge.runtime() else {
        return error_reply("plan.update requires a live runtime");
    };

    let tool_call_id = synthetic_tool_call_id("update_plan");
    let context = tool_context_from_bridge(bridge, tool_call_id);
    let reply = map_tool_outcome(
        PlanHandler::new()
            .handle(context, params.clone(), None)
            .await,
    );
    if reply.get("status").and_then(Value::as_str) != Some("ok") {
        return reply;
    }
    let Some(entries) =
        devo_protocol::native::plan_parse::plan_entries_from_update_plan_json(&reply)
    else {
        return error_reply("plan.update returned invalid plan entries");
    };

    runtime
        .emit_turn_native_item(
            bridge.session_id,
            turn_id,
            devo_protocol::native::item::Item::Plan {
                call_id: None,
                entries,
            },
        )
        .await;
    reply
}

pub(crate) async fn handle_question(bridge: &HostBridge, params: &Value) -> Value {
    let tool_call_id = synthetic_tool_call_id("question");
    let ctx = tool_context_from_bridge(bridge, tool_call_id);
    if ctx.turn_id.is_none() {
        return error_reply("question requires an active turn");
    }
    if ctx.agent_coordinator.is_none() {
        return error_reply("question requires a live runtime (userInput reverse-RPC)");
    }
    map_tool_outcome(
        QuestionHandler::new()
            .handle(ctx, params.clone(), None)
            .await,
    )
}

#[cfg(test)]
#[path = "kernel_host_bridge_tests.rs"]
mod tests;
