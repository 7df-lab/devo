use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;
use std::time::Instant;

use super::proposed_plan::{ProposedPlanParser, ProposedPlanSegment};
use super::*;
use crate::{FileChangePayload, TurnPlanStepPayload, TurnPlanUpdatedPayload};
use devo_core::tools::tool_spec::ToolPreparationFeedback;
use devo_util_git::extract_paths_from_patch;
use tokio::sync::mpsc;

const QUERY_EVENT_CHANNEL_CAPACITY: usize = 1024;
const QUERY_EVENT_FORWARD_CHANNEL_CAPACITY: usize = 1;
const QUERY_EVENT_BACKPRESSURE_LOG_THRESHOLD: Duration = Duration::from_millis(50);

struct PendingToolCall {
    item_id: Option<ItemId>,
    item_seq: Option<u64>,
    input: serde_json::Value,
    display_kind: ToolDisplayKind,
    command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolDisplayKind {
    CommandExecution,
    Generic,
}

impl ToolDisplayKind {
    fn for_tool_name(name: &str) -> Self {
        if is_unified_exec_tool(name) {
            Self::CommandExecution
        } else {
            Self::Generic
        }
    }

    fn is_command_execution(self) -> bool {
        self == Self::CommandExecution
    }
}

struct ToolStartItem {
    item_kind: ItemKind,
    payload: serde_json::Value,
}

pub(super) struct ExecuteTurnRequest {
    pub(super) session_id: SessionId,
    pub(super) turn: TurnMetadata,
    pub(super) turn_config: TurnConfig,
    pub(super) display_input: String,
    pub(super) input: String,
    pub(super) collaboration_mode: devo_protocol::CollaborationMode,
    pub(super) input_mode: TurnInputMode,
}

#[derive(Clone)]
struct BoundedQueryEventSender {
    tx: std_mpsc::SyncSender<QueryEvent>,
    queue_depth: Arc<AtomicUsize>,
    queue_max_depth: Arc<AtomicUsize>,
}

impl BoundedQueryEventSender {
    fn send(&self, event: QueryEvent) {
        let event_kind = query_event_trace_kind(&event);
        let delta_len = query_event_trace_delta_len(&event);
        let assistant_token_text = query_event_trace_token_preview(&event);
        let depth = self.queue_depth.fetch_add(1, Ordering::AcqRel) + 1;
        self.queue_max_depth.fetch_max(depth, Ordering::AcqRel);
        if let Some(assistant_token_text) = assistant_token_text.as_deref() {
            tracing::debug!(
                stream_elapsed_ms = stream_trace_elapsed_ms(),
                event_kind,
                delta_len,
                queue_depth = depth,
                assistant_token_text,
                "query event bridge enqueue requested"
            );
        } else {
            tracing::debug!(
                stream_elapsed_ms = stream_trace_elapsed_ms(),
                event_kind,
                delta_len,
                queue_depth = depth,
                "query event bridge enqueue requested"
            );
        }
        match self.tx.try_send(event) {
            Ok(()) => {
                tracing::trace!(
                    stream_elapsed_ms = stream_trace_elapsed_ms(),
                    event_kind,
                    queue_depth = depth,
                    "query event bridge enqueue accepted"
                );
            }
            Err(std_mpsc::TrySendError::Full(event)) => {
                let send_started_at = Instant::now();
                if self.tx.send(event).is_err() {
                    decrement_query_event_queue_depth(&self.queue_depth);
                    return;
                }
                let waited = send_started_at.elapsed();
                if waited >= QUERY_EVENT_BACKPRESSURE_LOG_THRESHOLD {
                    tracing::warn!(
                        stream_elapsed_ms = stream_trace_elapsed_ms(),
                        event_kind,
                        waited_ms = waited.as_millis(),
                        threshold_ms = QUERY_EVENT_BACKPRESSURE_LOG_THRESHOLD.as_millis(),
                        "query event bridge applied backpressure"
                    );
                }
            }
            Err(std_mpsc::TrySendError::Disconnected(_)) => {
                decrement_query_event_queue_depth(&self.queue_depth);
            }
        }
    }
}

fn bounded_query_event_channel(
    capacity: usize,
    queue_depth: Arc<AtomicUsize>,
    queue_max_depth: Arc<AtomicUsize>,
) -> (
    BoundedQueryEventSender,
    mpsc::Receiver<QueryEvent>,
    tokio::task::JoinHandle<()>,
) {
    let (ingress_tx, ingress_rx) = std_mpsc::sync_channel::<QueryEvent>(capacity);
    let (event_tx, event_rx) = mpsc::channel::<QueryEvent>(QUERY_EVENT_FORWARD_CHANNEL_CAPACITY);
    let queue_depth_for_forwarder = Arc::clone(&queue_depth);
    let forwarder = tokio::task::spawn_blocking(move || {
        while let Ok(event) = ingress_rx.recv() {
            if event_tx.blocking_send(event).is_err() {
                decrement_query_event_queue_depth(&queue_depth_for_forwarder);
                while ingress_rx.try_recv().is_ok() {
                    decrement_query_event_queue_depth(&queue_depth_for_forwarder);
                }
                break;
            }
        }
    });
    (
        BoundedQueryEventSender {
            tx: ingress_tx,
            queue_depth,
            queue_max_depth,
        },
        event_rx,
        forwarder,
    )
}

fn decrement_query_event_queue_depth(queue_depth: &AtomicUsize) {
    let _ = queue_depth.fetch_update(Ordering::AcqRel, Ordering::Acquire, |depth| {
        Some(depth.saturating_sub(1))
    });
}

fn stream_trace_elapsed_ms() -> u128 {
    static STREAM_TRACE_START: OnceLock<Instant> = OnceLock::new();
    STREAM_TRACE_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}

fn query_event_trace_kind(event: &QueryEvent) -> &'static str {
    match event {
        QueryEvent::TextDelta(_) => "text_delta",
        QueryEvent::ReasoningDelta(_) => "reasoning_delta",
        QueryEvent::ReasoningCompleted => "reasoning_completed",
        QueryEvent::UsageDelta { .. } => "usage_delta",
        QueryEvent::ToolUseStart { .. } => "tool_use_start",
        QueryEvent::ToolProgress { .. } => "tool_progress",
        QueryEvent::ToolResult { .. } => "tool_result",
        QueryEvent::TurnComplete { .. } => "turn_complete",
        QueryEvent::Usage { .. } => "usage",
    }
}

fn query_event_trace_delta_len(event: &QueryEvent) -> usize {
    match event {
        QueryEvent::TextDelta(text) | QueryEvent::ReasoningDelta(text) => text.len(),
        QueryEvent::ToolProgress { content, .. } => content.len(),
        QueryEvent::ReasoningCompleted
        | QueryEvent::UsageDelta { .. }
        | QueryEvent::ToolUseStart { .. }
        | QueryEvent::ToolResult { .. }
        | QueryEvent::TurnComplete { .. }
        | QueryEvent::Usage { .. } => 0,
    }
}

fn query_event_trace_token_preview(event: &QueryEvent) -> Option<String> {
    match event {
        QueryEvent::TextDelta(text) => assistant_token_log_preview(text),
        QueryEvent::ReasoningDelta(_)
        | QueryEvent::ReasoningCompleted
        | QueryEvent::UsageDelta { .. }
        | QueryEvent::ToolUseStart { .. }
        | QueryEvent::ToolProgress { .. }
        | QueryEvent::ToolResult { .. }
        | QueryEvent::TurnComplete { .. }
        | QueryEvent::Usage { .. } => None,
    }
}

fn assistant_token_log_preview(text: &str) -> Option<String> {
    assistant_token_logging_enabled()
        .then(|| format_assistant_token_log_preview(text, assistant_token_log_max_chars()))
}

fn assistant_token_logging_enabled() -> bool {
    static ASSISTANT_TOKEN_LOGGING_ENABLED: OnceLock<bool> = OnceLock::new();
    *ASSISTANT_TOKEN_LOGGING_ENABLED.get_or_init(|| {
        std::env::var("DEVO_LOG_ASSISTANT_TOKEN_TEXT")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.as_str(),
                    "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
                )
            })
    })
}

fn assistant_token_log_max_chars() -> usize {
    static ASSISTANT_TOKEN_LOG_MAX_CHARS: OnceLock<usize> = OnceLock::new();
    *ASSISTANT_TOKEN_LOG_MAX_CHARS.get_or_init(|| {
        std::env::var("DEVO_ASSISTANT_TOKEN_LOG_MAX_CHARS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(512)
    })
}

fn format_assistant_token_log_preview(text: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(1);
    let mut preview = String::new();
    let mut chars = text.chars();
    for ch in chars.by_ref().take(max_chars) {
        preview.extend(ch.escape_default());
    }
    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
}

async fn complete_reasoning_item(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    item_seq: u64,
    text: String,
) {
    runtime
        .complete_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            ItemKind::Reasoning,
            TurnItem::Reasoning(TextItem { text: text.clone() }),
            serde_json::json!({ "title": "Reasoning", "text": text }),
        )
        .await;
}

async fn complete_assistant_item(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    item_seq: u64,
    text: String,
) {
    runtime
        .complete_item(
            session_id,
            turn_id,
            item_id,
            item_seq,
            ItemKind::AgentMessage,
            TurnItem::AgentMessage(TextItem { text: text.clone() }),
            serde_json::json!({ "title": "Assistant", "text": text }),
        )
        .await;
}

#[derive(Debug, Default)]
struct ProposedPlanStreamItem {
    item_id: Option<ItemId>,
    item_seq: Option<u64>,
    text: String,
}

impl ProposedPlanStreamItem {
    async fn start(
        &mut self,
        runtime: &Arc<ServerRuntime>,
        session_id: SessionId,
        turn_id: TurnId,
    ) {
        if self.item_id.is_some() && self.item_seq.is_some() {
            return;
        }
        let (item_id, item_seq) = runtime
            .start_item(
                session_id,
                turn_id,
                ItemKind::Plan,
                serde_json::json!({ "title": "Proposed Plan", "text": "" }),
            )
            .await;
        self.item_id = Some(item_id);
        self.item_seq = Some(item_seq);
    }

    async fn push_delta(
        &mut self,
        runtime: &Arc<ServerRuntime>,
        session_id: SessionId,
        turn_id: TurnId,
        delta: String,
    ) {
        if delta.is_empty() {
            return;
        }
        self.start(runtime, session_id, turn_id).await;
        self.text.push_str(&delta);
        runtime
            .broadcast_event(ServerEvent::ItemDelta {
                delta_kind: ItemDeltaKind::PlanDelta,
                payload: ItemDeltaPayload {
                    context: EventContext {
                        session_id,
                        turn_id: Some(turn_id),
                        item_id: self.item_id,
                        seq: 0,
                    },
                    delta,
                    stream_index: None,
                    channel: None,
                },
            })
            .await;
    }

    async fn complete(
        &mut self,
        runtime: &Arc<ServerRuntime>,
        session_id: SessionId,
        turn_id: TurnId,
    ) {
        let (Some(item_id), Some(item_seq)) = (self.item_id.take(), self.item_seq.take()) else {
            return;
        };
        let text = std::mem::take(&mut self.text);
        runtime
            .complete_item(
                session_id,
                turn_id,
                item_id,
                item_seq,
                ItemKind::Plan,
                TurnItem::Plan(TextItem { text: text.clone() }),
                serde_json::json!({ "title": "Proposed Plan", "text": text }),
            )
            .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn push_assistant_text_delta(
    runtime: &Arc<ServerRuntime>,
    event_session_arc: &Arc<tokio::sync::Mutex<RuntimeSession>>,
    session_id: SessionId,
    turn_id: TurnId,
    assistant_item_id: &mut Option<ItemId>,
    assistant_item_seq: &mut Option<u64>,
    assistant_text: &mut String,
    assistant_delta_seq: &mut u64,
    text: String,
) {
    if text.is_empty() {
        return;
    }
    let (item_id, item_seq) = match (*assistant_item_id, *assistant_item_seq) {
        (Some(item_id), Some(item_seq)) => (item_id, item_seq),
        (None, None) => {
            let (item_id, item_seq) = runtime
                .start_item(
                    session_id,
                    turn_id,
                    ItemKind::AgentMessage,
                    serde_json::json!({ "title": "Assistant", "text": "" }),
                )
                .await;
            *assistant_item_id = Some(item_id);
            *assistant_item_seq = Some(item_seq);
            (item_id, item_seq)
        }
        _ => return,
    };
    assistant_text.push_str(&text);
    *assistant_delta_seq = (*assistant_delta_seq).saturating_add(1);
    runtime
        .broadcast_event(ServerEvent::ItemDelta {
            delta_kind: ItemDeltaKind::AgentMessageDelta,
            payload: ItemDeltaPayload {
                context: EventContext {
                    session_id,
                    turn_id: Some(turn_id),
                    item_id: Some(item_id),
                    seq: 0,
                },
                delta: text,
                stream_index: None,
                channel: None,
            },
        })
        .await;
    if let Ok(mut session) = event_session_arc.try_lock() {
        session.deferred_assistant = Some((item_id, item_seq, assistant_text.clone()));
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_proposed_plan_segments(
    runtime: &Arc<ServerRuntime>,
    event_session_arc: &Arc<tokio::sync::Mutex<RuntimeSession>>,
    session_id: SessionId,
    turn_id: TurnId,
    segments: Vec<ProposedPlanSegment>,
    assistant_item_id: &mut Option<ItemId>,
    assistant_item_seq: &mut Option<u64>,
    assistant_text: &mut String,
    assistant_delta_seq: &mut u64,
    proposed_plan_item: &mut ProposedPlanStreamItem,
    leading_normal_buffer: &mut String,
) {
    for segment in segments {
        match segment {
            ProposedPlanSegment::Normal(delta) => {
                if delta.is_empty() {
                    continue;
                }
                if assistant_item_id.is_none() && delta.chars().all(char::is_whitespace) {
                    leading_normal_buffer.push_str(&delta);
                    continue;
                }
                let delta = if assistant_item_id.is_none() && !leading_normal_buffer.is_empty() {
                    format!("{}{}", std::mem::take(leading_normal_buffer), delta)
                } else {
                    delta
                };
                push_assistant_text_delta(
                    runtime,
                    event_session_arc,
                    session_id,
                    turn_id,
                    assistant_item_id,
                    assistant_item_seq,
                    assistant_text,
                    assistant_delta_seq,
                    delta,
                )
                .await;
            }
            ProposedPlanSegment::PlanStart => {
                leading_normal_buffer.clear();
                proposed_plan_item.start(runtime, session_id, turn_id).await;
            }
            ProposedPlanSegment::PlanDelta(delta) => {
                proposed_plan_item
                    .push_delta(runtime, session_id, turn_id, delta)
                    .await;
            }
            ProposedPlanSegment::PlanEnd => {}
        }
    }
}

fn is_unified_exec_tool(name: &str) -> bool {
    matches!(name, "exec_command" | "write_stdin")
}

fn is_file_change_tool(name: &str) -> bool {
    matches!(name, "apply_patch" | "write")
}

fn is_plan_tool(name: &str) -> bool {
    matches!(name, "update_plan")
}

fn tool_start_item_kind(
    tool_name: &str,
    display_kind: ToolDisplayKind,
    preparation_feedback: ToolPreparationFeedback,
) -> ItemKind {
    if preparation_feedback == ToolPreparationFeedback::LiveOnly {
        ItemKind::ToolCall
    } else if is_file_change_tool(tool_name) {
        ItemKind::FileChange
    } else if display_kind.is_command_execution() {
        ItemKind::CommandExecution
    } else if is_plan_tool(tool_name) {
        ItemKind::Plan
    } else {
        ItemKind::ToolCall
    }
}

fn tool_start_item(
    tool_call_id: &str,
    tool_name: &str,
    command: &str,
    input: &serde_json::Value,
    display_kind: ToolDisplayKind,
    preparation_feedback: ToolPreparationFeedback,
    command_actions: Vec<devo_protocol::parse_command::ParsedCommand>,
) -> ToolStartItem {
    let item_kind = tool_start_item_kind(tool_name, display_kind, preparation_feedback);
    let payload = match item_kind {
        ItemKind::ToolCall => serde_json::to_value(ToolCallPayload {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            parameters: input.clone(),
            command_actions,
        })
        .expect("serialize tool call payload"),
        ItemKind::FileChange => serde_json::to_value(FileChangePayload {
            tool_call_id: tool_call_id.to_string(),
            tool_name: Some(tool_name.to_string()),
            input: Some(input.clone()),
            changes: Vec::new(),
            is_error: false,
        })
        .expect("serialize file change payload"),
        ItemKind::CommandExecution => serde_json::to_value(CommandExecutionPayload {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            command: command.to_string(),
            input: Some(input.clone()),
            source: devo_protocol::protocol::ExecCommandSource::Agent,
            command_actions,
            output: None,
            is_error: false,
        })
        .expect("serialize command execution payload"),
        ItemKind::Plan => serde_json::json!({
            "title": "Plan",
            "text": ""
        }),
        ItemKind::UserMessage
        | ItemKind::AgentMessage
        | ItemKind::Reasoning
        | ItemKind::ToolResult
        | ItemKind::McpToolCall
        | ItemKind::WebSearch
        | ItemKind::ImageView
        | ItemKind::ContextCompaction
        | ItemKind::ApprovalRequest
        | ItemKind::ApprovalDecision => unreachable!("tool start item kind must be tool-like"),
    };
    ToolStartItem { item_kind, payload }
}

fn tool_start_item_from_input(
    tool_call_id: &str,
    tool_name: &str,
    command: &str,
    input: &serde_json::Value,
    display_kind: ToolDisplayKind,
    preparation_feedback: ToolPreparationFeedback,
) -> ToolStartItem {
    tool_start_item(
        tool_call_id,
        tool_name,
        command,
        input,
        display_kind,
        preparation_feedback,
        command_actions_from_tool_input(tool_name, command, input),
    )
}

fn tool_start_item_from_result(
    tool_call_id: &str,
    tool_name: &str,
    command: &str,
    input: &serde_json::Value,
    display_kind: ToolDisplayKind,
    preparation_feedback: ToolPreparationFeedback,
    summary: &str,
) -> ToolStartItem {
    tool_start_item(
        tool_call_id,
        tool_name,
        command,
        input,
        display_kind,
        preparation_feedback,
        command_actions_from_tool_result(tool_name, command, input, summary),
    )
}

fn command_display_from_input(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "exec_command" => input
            .get("cmd")
            .or_else(|| input.get("command"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        "write_stdin" => {
            let session_id = input
                .get("session_id")
                .and_then(serde_json::Value::as_i64)
                .map(|id| id.to_string())
                .unwrap_or_else(|| "?".to_string());
            let chars = input
                .get("chars")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if chars.is_empty() {
                format!("poll session {session_id}")
            } else {
                format!("write_stdin session {session_id}")
            }
        }
        "read" => {
            let path = input
                .get("filePath")
                .or_else(|| input.get("path"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            format!("read {path}")
        }
        "find" | "glob" => {
            let pattern = input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let command_name = if tool_name == "find" { "find" } else { "glob" };
            if path.is_empty() {
                format!("{command_name} {pattern}")
            } else {
                format!("{command_name} {pattern} in {path}")
            }
        }
        "grep" => {
            let pattern = input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if path.is_empty() {
                format!("grep {pattern}")
            } else {
                format!("grep {pattern} in {path}")
            }
        }
        "code_search" => code_search_display_from_input(input),
        _ => String::new(),
    }
}

fn command_actions_from_tool_input(
    tool_name: &str,
    command: &str,
    input: &serde_json::Value,
) -> Vec<devo_protocol::parse_command::ParsedCommand> {
    match tool_name {
        "read" => crate::tool_actions::read_action_from_tool_input(command, input)
            .into_iter()
            .collect(),
        "find" | "glob" => vec![devo_protocol::parse_command::ParsedCommand::ListFiles {
            cmd: command.to_string(),
            path: find_display_from_input(input),
        }],
        "grep" => vec![devo_protocol::parse_command::ParsedCommand::Search {
            cmd: command.to_string(),
            query: input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            path: input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
        }],
        "code_search" => code_search_action_from_input(command, input)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn code_search_display_from_input(input: &serde_json::Value) -> String {
    match input
        .get("operation")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("search")
    {
        "find_related" => {
            let path = input
                .get("file_path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let line = input.get("line").and_then(serde_json::Value::as_u64);
            match (path.is_empty(), line) {
                (false, Some(line)) => format!("code_search related {path}:{line}"),
                (false, None) => format!("code_search related {path}"),
                (true, _) => "code_search related".to_string(),
            }
        }
        _ => {
            let query = input
                .get("query")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            match (query.is_empty(), path.is_empty()) {
                (false, false) => format!("code_search {query} in {path}"),
                (false, true) => format!("code_search {query}"),
                (true, false) => format!("code_search in {path}"),
                (true, true) => "code_search".to_string(),
            }
        }
    }
}

fn code_search_action_from_input(
    command: &str,
    input: &serde_json::Value,
) -> Option<devo_protocol::parse_command::ParsedCommand> {
    match input
        .get("operation")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("search")
    {
        "find_related" => {
            let path = input
                .get("file_path")
                .and_then(serde_json::Value::as_str)
                .filter(|path| !path.is_empty())?;
            let line = input
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .map(|line| line.to_string())
                .unwrap_or_else(|| "?".to_string());
            Some(devo_protocol::parse_command::ParsedCommand::Search {
                cmd: command.to_string(),
                query: Some(format!("related {path}:{line}")),
                path: Some(path.to_string()),
            })
        }
        _ => {
            let query = input
                .get("query")
                .and_then(serde_json::Value::as_str)
                .filter(|query| !query.is_empty())?;
            Some(devo_protocol::parse_command::ParsedCommand::Search {
                cmd: command.to_string(),
                query: Some(query.to_string()),
                path: input
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
            })
        }
    }
}

fn find_display_from_input(input: &serde_json::Value) -> Option<String> {
    let pattern = input
        .get("pattern")
        .and_then(serde_json::Value::as_str)
        .filter(|pattern| !pattern.is_empty())?;
    let path = input.get("path").and_then(serde_json::Value::as_str);
    Some(match path.filter(|path| !path.is_empty()) {
        Some(path) => format!("{pattern} in {path}"),
        None => pattern.to_string(),
    })
}

fn command_actions_from_tool_result(
    tool_name: &str,
    command: &str,
    input: &serde_json::Value,
    summary: &str,
) -> Vec<devo_protocol::parse_command::ParsedCommand> {
    let actions = command_actions_from_tool_input(tool_name, command, input);
    if !actions.is_empty() {
        return actions;
    }
    match tool_name {
        "read" => crate::tool_actions::read_action_from_tool_summary(summary)
            .into_iter()
            .collect(),
        _ => actions,
    }
}

fn collaboration_mode_from_pending_metadata(
    metadata: Option<&serde_json::Value>,
) -> devo_protocol::CollaborationMode {
    metadata
        .and_then(|metadata| {
            metadata
                .get("collaboration_mode")
                .or_else(|| metadata.get("interaction_mode"))
        })
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn command_execution_item_id_for_progress(
    pending_tool_calls: &HashMap<String, PendingToolCall>,
    tool_use_id: &str,
) -> Option<ItemId> {
    pending_tool_calls
        .get(tool_use_id)
        .filter(|pending| pending.display_kind.is_command_execution())
        .and_then(|pending| pending.item_id)
}

fn user_shell_exec_input(command: &str, cwd: std::path::PathBuf) -> serde_json::Value {
    serde_json::json!({
        "cmd": command,
        "workdir": cwd,
        "login": true,
        "tty": true,
    })
}

fn user_shell_command_payload(
    tool_call_id: &str,
    command: &str,
    input: serde_json::Value,
    command_actions: Vec<devo_protocol::parse_command::ParsedCommand>,
    output: Option<serde_json::Value>,
    is_error: bool,
) -> CommandExecutionPayload {
    CommandExecutionPayload {
        tool_call_id: tool_call_id.to_string(),
        tool_name: "exec_command".to_string(),
        command: command.to_string(),
        input: Some(input),
        source: devo_protocol::protocol::ExecCommandSource::UserShell,
        command_actions,
        output,
        is_error,
    }
}

impl ServerRuntime {
    pub(super) async fn execute_shell_command_turn(
        self: Arc<Self>,
        session_id: SessionId,
        turn: TurnMetadata,
        command: String,
        cwd: std::path::PathBuf,
    ) {
        if let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() {
            session_arc.lock().await.turn_approval_cache =
                crate::execution::ApprovalGrantCache::default();
        }

        let tool_call_id = format!("user-shell-{}", turn.turn_id);
        let input = user_shell_exec_input(&command, cwd.clone());
        let command_actions = parse_command(std::slice::from_ref(&command));
        let (item_id, item_seq) = self
            .start_item(
                session_id,
                turn.turn_id,
                ItemKind::CommandExecution,
                serde_json::to_value(user_shell_command_payload(
                    &tool_call_id,
                    &command,
                    input.clone(),
                    command_actions.clone(),
                    None,
                    false,
                ))
                .expect("serialize command execution payload"),
            )
            .await;

        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return;
        };
        let (permission_mode, permission_profile) = {
            let session = session_arc.lock().await;
            let core_session = session.core_session.lock().await;
            (
                core_session.config.permission_mode,
                core_session.config.permission_profile.clone(),
            )
        };
        let network_proxy = self
            .deps
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .provider_http
            .proxy_url
            .clone();
        let runtime = ToolRuntime::new_with_context(
            Arc::clone(&self.deps.registry),
            self.build_permission_checker(
                session_id,
                turn.turn_id,
                permission_mode,
                permission_profile,
            ),
            ToolRuntimeContext {
                session_id: session_id.to_string(),
                turn_id: Some(turn.turn_id.to_string()),
                cwd,
                agent_scope: ToolAgentScope::Parent,
                collaboration_mode: devo_protocol::CollaborationMode::Build,
                agent_coordinator: None,
                local_web_search: None,
                hooks: self.hook_context_for_session(session_id).await,
                network_proxy,
            },
        );
        let result = runtime
            .execute_batch(&[ToolCall {
                id: tool_call_id.clone(),
                name: "exec_command".to_string(),
                input: input.clone(),
            }])
            .await
            .into_iter()
            .next()
            .unwrap_or_else(|| ToolCallResult::error(&tool_call_id, "shell command did not run"));
        let output = match result.content.clone() {
            ToolContent::Text(text) => serde_json::Value::String(text),
            ToolContent::Json(json) => json,
            ToolContent::Mixed { text, json } => {
                json.unwrap_or_else(|| serde_json::Value::String(text.unwrap_or_default()))
            }
        };
        let is_error = result.is_error;
        self.complete_item(
            session_id,
            turn.turn_id,
            item_id,
            item_seq,
            ItemKind::CommandExecution,
            TurnItem::CommandExecution(CommandExecutionItem {
                tool_call_id: tool_call_id.clone(),
                tool_name: "exec_command".to_string(),
                command: command.clone(),
                input: input.clone(),
                output: output.clone(),
                is_error,
            }),
            serde_json::to_value(user_shell_command_payload(
                &tool_call_id,
                &command,
                input.clone(),
                command_actions,
                Some(output),
                is_error,
            ))
            .expect("serialize command execution payload"),
        )
        .await;

        let final_turn = {
            let mut session = session_arc.lock().await;
            let mut final_turn = turn.clone();
            final_turn.completed_at = Some(Utc::now());
            final_turn.status = if is_error {
                TurnStatus::Failed
            } else {
                TurnStatus::Completed
            };
            session.latest_turn = Some(final_turn.clone());
            session.active_turn = None;
            session.summary.status = SessionRuntimeStatus::Idle;
            session.summary.updated_at = Utc::now();
            final_turn
        };
        let (record, session_context, turn_context) = {
            let session = session_arc.lock().await;
            let core_session = session.core_session.lock().await;
            (
                session.record.clone(),
                core_session.session_context.clone(),
                core_session.latest_turn_context.clone(),
            )
        };
        if let Some(record) = record
            && let Err(error) = self.rollout_store.append_turn(
                &record,
                build_turn_record(&final_turn, session_context, turn_context),
            )
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist shell command turn line");
        }
        if is_error {
            self.broadcast_event(ServerEvent::TurnFailed(TurnEventPayload {
                session_id,
                turn: final_turn.clone(),
            }))
            .await;
        }
        self.broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
            session_id,
            turn: final_turn,
        }))
        .await;
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id,
                status: SessionRuntimeStatus::Idle,
            },
        ))
        .await;
    }

    /// Execute one turn end-to-end, including streaming query events,
    /// persisting turn state, and draining queued follow-up inputs.
    pub(super) async fn execute_turn(self: Arc<Self>, request: ExecuteTurnRequest) {
        let ExecuteTurnRequest {
            session_id,
            turn,
            turn_config,
            display_input,
            input,
            collaboration_mode,
            input_mode,
        } = request;
        if let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() {
            session_arc.lock().await.turn_approval_cache =
                crate::execution::ApprovalGrantCache::default();
        }
        if input_mode.emits_user_message() {
            // Record the user's message immediately so the UI can show it even if
            // the model call or event stream takes a moment to start.
            self.emit_turn_item(
                session_id,
                turn.turn_id,
                ItemKind::UserMessage,
                TurnItem::UserMessage(TextItem {
                    text: display_input.clone(),
                }),
                serde_json::json!({ "title": "You", "text": display_input.clone() }),
            )
            .await;
        }

        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return;
        };
        let event_queue_depth = Arc::new(AtomicUsize::new(0));
        let event_queue_max_depth = Arc::new(AtomicUsize::new(0));
        let (event_tx, mut event_rx, event_forwarder_task) = bounded_query_event_channel(
            QUERY_EVENT_CHANNEL_CAPACITY,
            Arc::clone(&event_queue_depth),
            Arc::clone(&event_queue_max_depth),
        );
        let runtime = Arc::clone(&self);
        let turn_for_events = turn.clone();
        let turn_for_plan_updates = turn.clone();
        let event_session_arc = Arc::clone(&session_arc);
        let event_queue_depth_for_task = Arc::clone(&event_queue_depth);
        let event_queue_max_depth_for_task = Arc::clone(&event_queue_max_depth);
        let event_task = tokio::spawn(async move {
            // This task owns the streamed model output. It turns raw query
            // callbacks into persisted turn items and keeps enough state to
            // resume cleanly if the turn is interrupted mid-stream.
            let mut assistant_item_id = None;
            let mut assistant_item_seq = None;
            let mut assistant_delta_seq = 0_u64;
            let mut assistant_text = String::new();
            let mut reasoning_item_id = None;
            let mut reasoning_item_seq = None;
            let mut reasoning_text = String::new();
            let mut tool_names_by_id = HashMap::new();
            let mut pending_tool_calls: HashMap<String, PendingToolCall> = HashMap::new();
            let mut proposed_plan_parser = (collaboration_mode
                == devo_protocol::CollaborationMode::Plan)
                .then(ProposedPlanParser::default);
            let mut proposed_plan_item = ProposedPlanStreamItem::default();
            let mut proposed_plan_leading_normal = String::new();
            let mut latest_usage: Option<TurnUsage> = None;
            let mut usage_base: Option<(usize, usize, usize)> = None;
            while let Some(event) = event_rx.recv().await {
                decrement_query_event_queue_depth(&event_queue_depth_for_task);
                let assistant_token_text = query_event_trace_token_preview(&event);
                if let Some(assistant_token_text) = assistant_token_text.as_deref() {
                    tracing::debug!(
                        stream_elapsed_ms = stream_trace_elapsed_ms(),
                        event_kind = query_event_trace_kind(&event),
                        delta_len = query_event_trace_delta_len(&event),
                        queue_depth = event_queue_depth_for_task.load(Ordering::Acquire),
                        assistant_token_text,
                        "query event bridge dequeued by turn event task"
                    );
                } else {
                    tracing::debug!(
                        stream_elapsed_ms = stream_trace_elapsed_ms(),
                        event_kind = query_event_trace_kind(&event),
                        delta_len = query_event_trace_delta_len(&event),
                        queue_depth = event_queue_depth_for_task.load(Ordering::Acquire),
                        "query event bridge dequeued by turn event task"
                    );
                }
                match event {
                    QueryEvent::TextDelta(text) => {
                        if let Some(parser) = proposed_plan_parser.as_mut() {
                            let segments = parser.push_str(&text);
                            handle_proposed_plan_segments(
                                &runtime,
                                &event_session_arc,
                                session_id,
                                turn_for_events.turn_id,
                                segments,
                                &mut assistant_item_id,
                                &mut assistant_item_seq,
                                &mut assistant_text,
                                &mut assistant_delta_seq,
                                &mut proposed_plan_item,
                                &mut proposed_plan_leading_normal,
                            )
                            .await;
                        } else {
                            push_assistant_text_delta(
                                &runtime,
                                &event_session_arc,
                                session_id,
                                turn_for_events.turn_id,
                                &mut assistant_item_id,
                                &mut assistant_item_seq,
                                &mut assistant_text,
                                &mut assistant_delta_seq,
                                text,
                            )
                            .await;
                        }
                    }
                    QueryEvent::ReasoningDelta(text) => {
                        let (item_id, item_seq) = match (reasoning_item_id, reasoning_item_seq) {
                            (Some(item_id), Some(item_seq)) => (item_id, item_seq),
                            (None, None) => {
                                let (item_id, item_seq) = runtime
                                    .start_item(
                                        session_id,
                                        turn_for_events.turn_id,
                                        ItemKind::Reasoning,
                                        serde_json::json!({ "title": "Reasoning", "text": "" }),
                                    )
                                    .await;
                                reasoning_item_id = Some(item_id);
                                reasoning_item_seq = Some(item_seq);
                                (item_id, item_seq)
                            }
                            _ => continue,
                        };
                        reasoning_text.push_str(&text);
                        runtime
                            .broadcast_event(ServerEvent::ItemDelta {
                                delta_kind: ItemDeltaKind::ReasoningTextDelta,
                                payload: ItemDeltaPayload {
                                    context: EventContext {
                                        session_id,
                                        turn_id: Some(turn_for_events.turn_id),
                                        item_id: Some(item_id),
                                        seq: 0,
                                    },
                                    delta: text,
                                    stream_index: None,
                                    channel: None,
                                },
                            })
                            .await;
                        let _ = item_seq;

                        // Store deferred completion info for interrupt recovery
                        if let Ok(mut session) = event_session_arc.try_lock() {
                            session.deferred_reasoning =
                                Some((item_id, item_seq, reasoning_text.clone()));
                        }
                    }
                    QueryEvent::ReasoningCompleted => {
                        if let (Some(item_id), Some(item_seq)) =
                            (reasoning_item_id.take(), reasoning_item_seq.take())
                        {
                            if let Ok(mut session) = event_session_arc.try_lock() {
                                session.deferred_reasoning.take();
                            }
                            complete_reasoning_item(
                                &runtime,
                                session_id,
                                turn_for_events.turn_id,
                                item_id,
                                item_seq,
                                reasoning_text.clone(),
                            )
                            .await;
                            reasoning_text.clear();
                        }
                    }
                    QueryEvent::ToolUseStart { id, name, input } => {
                        tool_names_by_id.insert(id.clone(), name.clone());
                        if let (Some(item_id), Some(item_seq)) =
                            (reasoning_item_id.take(), reasoning_item_seq.take())
                        {
                            complete_reasoning_item(
                                &runtime,
                                session_id,
                                turn_for_events.turn_id,
                                item_id,
                                item_seq,
                                reasoning_text.clone(),
                            )
                            .await;
                            reasoning_text.clear();
                        }
                        if let (Some(item_id), Some(item_seq)) =
                            (assistant_item_id.take(), assistant_item_seq.take())
                        {
                            complete_assistant_item(
                                &runtime,
                                session_id,
                                turn_for_events.turn_id,
                                item_id,
                                item_seq,
                                assistant_text.clone(),
                            )
                            .await;
                            assistant_text.clear();
                        }
                        let display_kind = ToolDisplayKind::for_tool_name(&name);
                        let command = command_display_from_input(&name, &input);
                        let preparation_feedback =
                            runtime.deps.registry.preparation_feedback(&name);
                        let start_item = tool_start_item_from_input(
                            &id,
                            &name,
                            &command,
                            &input,
                            display_kind,
                            preparation_feedback,
                        );
                        let (item_id, item_seq) = runtime
                            .start_item(
                                session_id,
                                turn_for_events.turn_id,
                                start_item.item_kind,
                                start_item.payload,
                            )
                            .await;
                        pending_tool_calls.insert(
                            id,
                            PendingToolCall {
                                item_id: Some(item_id),
                                item_seq: Some(item_seq),
                                input,
                                display_kind,
                                command,
                            },
                        );
                    }
                    QueryEvent::ToolResult {
                        tool_use_id,
                        tool_name: final_tool_name,
                        input: final_input,
                        content,
                        display_content,
                        is_error,
                        summary,
                    } => {
                        let tool_name = if final_tool_name.is_empty() {
                            tool_names_by_id.get(&tool_use_id).cloned()
                        } else {
                            Some(final_tool_name)
                        };
                        let mut result_input =
                            (!final_input.is_null()).then(|| final_input.clone());
                        // First complete the pending ToolCall item so its item/completed
                        // arrives before the ToolResult item/completed.
                        if let Some(mut pending) = pending_tool_calls.remove(&tool_use_id) {
                            if !final_input.is_null() {
                                pending.command = tool_name
                                    .as_deref()
                                    .map(|tool_name| {
                                        command_display_from_input(tool_name, &final_input)
                                    })
                                    .unwrap_or_default();
                                pending.input = final_input;
                            }
                            result_input = Some(pending.input.clone());
                            if (pending.item_id.is_none() || pending.item_seq.is_none())
                                && let Some(tool_name) = tool_name.clone()
                            {
                                let preparation_feedback =
                                    runtime.deps.registry.preparation_feedback(&tool_name);
                                let start_item = tool_start_item_from_result(
                                    &tool_use_id,
                                    &tool_name,
                                    &pending.command,
                                    &pending.input,
                                    pending.display_kind,
                                    preparation_feedback,
                                    &summary,
                                );
                                let (item_id, item_seq) = runtime
                                    .start_item(
                                        session_id,
                                        turn_for_events.turn_id,
                                        start_item.item_kind,
                                        start_item.payload,
                                    )
                                    .await;
                                pending.item_id = Some(item_id);
                                pending.item_seq = Some(item_seq);
                            }

                            let pending_item_id = pending.item_id.expect("pending item id");
                            let pending_item_seq = pending.item_seq.expect("pending item seq");
                            if let Some(tool_name) = tool_name.clone()
                                && is_plan_tool(&tool_name)
                            {
                                let output_json = match content.clone() {
                                    devo_core::tools::ToolContent::Text(text) => {
                                        serde_json::Value::String(text)
                                    }
                                    devo_core::tools::ToolContent::Json(json) => json,
                                    devo_core::tools::ToolContent::Mixed { text, json } => json
                                        .unwrap_or_else(|| {
                                            serde_json::Value::String(text.unwrap_or_default())
                                        }),
                                };
                                let explanation = output_json
                                    .get("explanation")
                                    .and_then(serde_json::Value::as_str)
                                    .map(ToOwned::to_owned);
                                let plan = output_json
                                    .get("plan")
                                    .and_then(serde_json::Value::as_array)
                                    .cloned()
                                    .unwrap_or_default();

                                runtime
                                    .complete_item(
                                        session_id,
                                        turn_for_events.turn_id,
                                        pending_item_id,
                                        pending_item_seq,
                                        ItemKind::Plan,
                                        TurnItem::Plan(TextItem {
                                            text: output_json.to_string(),
                                        }),
                                        serde_json::json!({
                                            "title": "Plan",
                                            "text": output_json.to_string(),
                                        }),
                                    )
                                    .await;

                                runtime
                                    .broadcast_event(ServerEvent::TurnPlanUpdated(
                                        TurnPlanUpdatedPayload {
                                            session_id,
                                            turn: turn_for_plan_updates.clone(),
                                            explanation,
                                            plan: plan
                                                .into_iter()
                                                .filter_map(|item| {
                                                    Some(TurnPlanStepPayload {
                                                        step: item
                                                            .get("step")?
                                                            .as_str()?
                                                            .to_string(),
                                                        status: item
                                                            .get("status")?
                                                            .as_str()?
                                                            .to_string(),
                                                    })
                                                })
                                                .collect(),
                                        },
                                    ))
                                    .await;
                                continue;
                            }

                            if let Some(tool_name) = tool_name.clone()
                                && is_file_change_tool(&tool_name)
                            {
                                let output_json = match content.clone() {
                                    devo_core::tools::ToolContent::Text(text) => {
                                        serde_json::Value::String(text)
                                    }
                                    devo_core::tools::ToolContent::Json(json) => json,
                                    devo_core::tools::ToolContent::Mixed { text, json } => json
                                        .unwrap_or_else(|| {
                                            serde_json::Value::String(text.unwrap_or_default())
                                        }),
                                };
                                let changes = output_json
                                    .get("files")
                                    .and_then(serde_json::Value::as_array)
                                    .cloned()
                                    .unwrap_or_default()
                                    .into_iter()
                                    .filter_map(|file| {
                                        let path =
                                            std::path::PathBuf::from(file.get("path")?.as_str()?);
                                        let kind = file.get("kind")?.as_str()?;
                                        let additions = file
                                            .get("additions")
                                            .and_then(serde_json::Value::as_u64)
                                            .unwrap_or(0);
                                        let deletions = file
                                            .get("deletions")
                                            .and_then(serde_json::Value::as_u64)
                                            .unwrap_or(0);
                                        let change = match kind {
                                            "add" => devo_protocol::protocol::FileChange::Add {
                                                content: file
                                                    .get("content")
                                                    .and_then(serde_json::Value::as_str)
                                                    .map(ToOwned::to_owned)
                                                    .unwrap_or_else(|| {
                                                        "\n".repeat(additions as usize)
                                                    }),
                                            },
                                            "delete" => {
                                                devo_protocol::protocol::FileChange::Delete {
                                                    content: file
                                                        .get("content")
                                                        .and_then(serde_json::Value::as_str)
                                                        .map(ToOwned::to_owned)
                                                        .unwrap_or_else(|| {
                                                            "\n".repeat(deletions as usize)
                                                        }),
                                                }
                                            }
                                            "update" | "move" => {
                                                devo_protocol::protocol::FileChange::Update {
                                                    unified_diff: file
                                                        .get("diff")
                                                        .or_else(|| file.get("patch"))
                                                        .or_else(|| output_json.get("diff"))
                                                        .and_then(serde_json::Value::as_str)
                                                        .unwrap_or("")
                                                        .to_string(),
                                                    move_path: file
                                                        .get("movePath")
                                                        .or_else(|| file.get("move_path"))
                                                        .and_then(serde_json::Value::as_str)
                                                        .map(std::path::PathBuf::from),
                                                }
                                            }
                                            _ => return None,
                                        };
                                        Some((path, change))
                                    })
                                    .collect::<Vec<_>>();
                                let changes = if changes.is_empty() {
                                    output_json
                                        .get("diff")
                                        .and_then(serde_json::Value::as_str)
                                        .map(extract_paths_from_patch)
                                        .unwrap_or_default()
                                        .into_iter()
                                        .map(|path| {
                                            (
                                                std::path::PathBuf::from(path),
                                                devo_protocol::protocol::FileChange::Update {
                                                    unified_diff: output_json
                                                        .get("diff")
                                                        .and_then(serde_json::Value::as_str)
                                                        .unwrap_or("")
                                                        .to_string(),
                                                    move_path: None,
                                                },
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                } else {
                                    changes
                                };

                                runtime
                                    .complete_item(
                                        session_id,
                                        turn_for_events.turn_id,
                                        pending_item_id,
                                        pending_item_seq,
                                        ItemKind::FileChange,
                                        TurnItem::ToolResult(ToolResultItem {
                                            tool_call_id: tool_use_id.clone(),
                                            tool_name: Some(tool_name.clone()),
                                            output: output_json.clone(),
                                            display_content: display_content.clone(),
                                            is_error,
                                        }),
                                        serde_json::to_value(FileChangePayload {
                                            tool_call_id: tool_use_id.clone(),
                                            tool_name: Some(tool_name.clone()),
                                            input: Some(pending.input.clone()),
                                            changes,
                                            is_error,
                                        })
                                        .expect("serialize file change payload"),
                                    )
                                    .await;
                                continue;
                            }

                            if pending.display_kind.is_command_execution() {
                                let tool_name = tool_name.clone().unwrap_or_default();
                                let output = match content.clone() {
                                    devo_core::tools::ToolContent::Text(text) => {
                                        serde_json::Value::String(text)
                                    }
                                    devo_core::tools::ToolContent::Json(json) => json,
                                    devo_core::tools::ToolContent::Mixed { text, json } => json
                                        .unwrap_or_else(|| {
                                            serde_json::Value::String(text.unwrap_or_default())
                                        }),
                                };
                                let completed_payload =
                                    serde_json::to_value(CommandExecutionPayload {
                                        tool_call_id: tool_use_id.clone(),
                                        tool_name: tool_name.clone(),
                                        command: pending.command.clone(),
                                        input: Some(pending.input.clone()),
                                        source: devo_protocol::protocol::ExecCommandSource::Agent,
                                        command_actions: command_actions_from_tool_result(
                                            &tool_name,
                                            &pending.command,
                                            &pending.input,
                                            &summary,
                                        ),
                                        output: Some(output.clone()),
                                        is_error,
                                    })
                                    .expect("serialize command execution payload");
                                runtime
                                    .complete_item(
                                        session_id,
                                        turn_for_events.turn_id,
                                        pending_item_id,
                                        pending_item_seq,
                                        ItemKind::CommandExecution,
                                        TurnItem::CommandExecution(CommandExecutionItem {
                                            tool_call_id: tool_use_id.clone(),
                                            tool_name,
                                            command: pending.command,
                                            input: pending.input,
                                            output,
                                            is_error,
                                        }),
                                        completed_payload,
                                    )
                                    .await;
                                continue;
                            }
                            let completed_payload = serde_json::to_value(ToolCallPayload {
                                tool_call_id: tool_use_id.clone(),
                                tool_name: tool_name.clone().unwrap_or_default(),
                                parameters: pending.input.clone(),
                                command_actions: command_actions_from_tool_result(
                                    tool_name.clone().unwrap_or_default().as_str(),
                                    &pending.command,
                                    &pending.input,
                                    &summary,
                                ),
                            })
                            .expect("serialize tool call payload");
                            runtime
                                .complete_item(
                                    session_id,
                                    turn_for_events.turn_id,
                                    pending_item_id,
                                    pending_item_seq,
                                    ItemKind::ToolCall,
                                    TurnItem::ToolCall(ToolCallItem {
                                        tool_call_id: tool_use_id.clone(),
                                        tool_name: tool_name.clone().unwrap_or_default(),
                                        input: pending.input,
                                    }),
                                    completed_payload,
                                )
                                .await;
                        }
                        runtime
                            .emit_turn_item(
                                session_id,
                                turn_for_events.turn_id,
                                ItemKind::ToolResult,
                                TurnItem::ToolResult(ToolResultItem {
                                    tool_call_id: tool_use_id.clone(),
                                    tool_name: tool_name.clone(),
                                    output: match content.clone() {
                                        devo_core::tools::ToolContent::Text(text) => {
                                            serde_json::Value::String(text)
                                        }
                                        devo_core::tools::ToolContent::Json(json) => json,
                                        devo_core::tools::ToolContent::Mixed { text, json } => json
                                            .unwrap_or_else(|| {
                                                serde_json::Value::String(text.unwrap_or_default())
                                            }),
                                    },
                                    display_content: display_content.clone(),
                                    is_error,
                                }),
                                serde_json::to_value(ToolResultPayload {
                                    tool_call_id: tool_use_id.clone(),
                                    tool_name,
                                    input: result_input,
                                    content: match content {
                                        devo_core::tools::ToolContent::Text(text) => {
                                            serde_json::Value::String(text)
                                        }
                                        devo_core::tools::ToolContent::Json(json) => json,
                                        devo_core::tools::ToolContent::Mixed { text, json } => json
                                            .unwrap_or_else(|| {
                                                serde_json::Value::String(text.unwrap_or_default())
                                            }),
                                    },
                                    display_content,
                                    is_error,
                                    summary,
                                })
                                .expect("serialize tool result payload"),
                            )
                            .await;
                    }
                    QueryEvent::ToolProgress {
                        tool_use_id,
                        content,
                    } => {
                        let item_id = command_execution_item_id_for_progress(
                            &pending_tool_calls,
                            &tool_use_id,
                        );
                        let _ = runtime
                            .broadcast_event(ServerEvent::ItemDelta {
                                delta_kind: ItemDeltaKind::CommandExecutionOutputDelta,
                                payload: ItemDeltaPayload {
                                    context: EventContext {
                                        session_id,
                                        turn_id: Some(turn_for_events.turn_id),
                                        item_id,
                                        seq: 0,
                                    },
                                    delta: serde_json::json!({
                                        "tool_use_id": tool_use_id,
                                        "text": content,
                                    })
                                    .to_string(),
                                    stream_index: None,
                                    channel: None,
                                },
                            })
                            .await;
                    }
                    QueryEvent::UsageDelta {
                        input_tokens,
                        output_tokens,
                        cache_creation_input_tokens,
                        cache_read_input_tokens,
                    } => {
                        let usage = TurnUsage {
                            input_tokens: input_tokens as u32,
                            output_tokens: output_tokens as u32,
                            cache_creation_input_tokens: cache_creation_input_tokens
                                .map(|value| value as u32),
                            cache_read_input_tokens: cache_read_input_tokens
                                .map(|value| value as u32),
                        };
                        latest_usage = Some(usage.clone());

                        let base = if let Some(base) = usage_base {
                            base
                        } else {
                            let base = {
                                let session = event_session_arc.lock().await;
                                (
                                    session.summary.total_input_tokens,
                                    session.summary.total_output_tokens,
                                    session.summary.total_cache_read_tokens,
                                )
                            };
                            usage_base = Some(base);
                            base
                        };
                        {
                            let mut session = event_session_arc.lock().await;
                            session.summary.total_input_tokens =
                                base.0 + usage.input_tokens as usize;
                            session.summary.total_output_tokens =
                                base.1 + usage.output_tokens as usize;
                        }
                        let _ = runtime
                            .broadcast_event(ServerEvent::TurnUsageUpdated(
                                TurnUsageUpdatedPayload {
                                    session_id,
                                    turn_id: turn_for_events.turn_id,
                                    usage,
                                    total_input_tokens: base.0 + input_tokens,
                                    total_output_tokens: base.1 + output_tokens,
                                    total_cache_read_tokens: base.2
                                        + cache_read_input_tokens.unwrap_or(0),
                                    last_query_input_tokens: input_tokens,
                                },
                            ))
                            .await;
                    }
                    QueryEvent::Usage {
                        input_tokens,
                        output_tokens,
                        cache_creation_input_tokens,
                        cache_read_input_tokens,
                    } => {
                        let usage = TurnUsage {
                            input_tokens: input_tokens as u32,
                            output_tokens: output_tokens as u32,
                            cache_creation_input_tokens: cache_creation_input_tokens
                                .map(|value| value as u32),
                            cache_read_input_tokens: cache_read_input_tokens
                                .map(|value| value as u32),
                        };
                        latest_usage = Some(usage.clone());

                        let base = if let Some(base) = usage_base {
                            base
                        } else {
                            let base = {
                                let session = event_session_arc.lock().await;
                                (
                                    session.summary.total_input_tokens,
                                    session.summary.total_output_tokens,
                                    session.summary.total_cache_read_tokens,
                                )
                            };
                            usage_base = Some(base);
                            base
                        };
                        {
                            let mut session = event_session_arc.lock().await;
                            session.summary.total_input_tokens =
                                base.0 + usage.input_tokens as usize;
                            session.summary.total_output_tokens =
                                base.1 + usage.output_tokens as usize;
                            session.summary.total_cache_read_tokens =
                                base.2 + usage.cache_read_input_tokens.unwrap_or(0) as usize;
                            session.summary.last_query_total_tokens =
                                usage.input_tokens as usize + usage.output_tokens as usize;
                        }
                        let _ = runtime
                            .broadcast_event(ServerEvent::TurnUsageUpdated(
                                TurnUsageUpdatedPayload {
                                    session_id,
                                    turn_id: turn_for_events.turn_id,
                                    usage,
                                    total_input_tokens: base.0 + input_tokens,
                                    total_output_tokens: base.1 + output_tokens,
                                    total_cache_read_tokens: base.2
                                        + cache_read_input_tokens.unwrap_or(0),
                                    last_query_input_tokens: input_tokens,
                                },
                            ))
                            .await;
                    }
                    QueryEvent::TurnComplete { .. } => {}
                }
            }
            if let Some(parser) = proposed_plan_parser.as_mut() {
                let segments = parser.finish();
                handle_proposed_plan_segments(
                    &runtime,
                    &event_session_arc,
                    session_id,
                    turn_for_events.turn_id,
                    segments,
                    &mut assistant_item_id,
                    &mut assistant_item_seq,
                    &mut assistant_text,
                    &mut assistant_delta_seq,
                    &mut proposed_plan_item,
                    &mut proposed_plan_leading_normal,
                )
                .await;
                proposed_plan_item
                    .complete(&runtime, session_id, turn_for_events.turn_id)
                    .await;
            }
            // Complete any deferred items that the interrupt handler didn't already take.
            // handle_interrupt takes deferred_assistant/deferred_reasoning from the session
            // and completes them; if they're already None we must skip to avoid persisting duplicates.
            if let Some((item_id, item_seq, text)) = {
                let mut session = event_session_arc.lock().await;
                session.deferred_reasoning.take()
            } {
                complete_reasoning_item(
                    &runtime,
                    session_id,
                    turn_for_events.turn_id,
                    item_id,
                    item_seq,
                    text,
                )
                .await;
            }
            if let Some((item_id, item_seq, text)) = {
                let mut session = event_session_arc.lock().await;
                session.deferred_assistant.take()
            } {
                complete_assistant_item(
                    &runtime,
                    session_id,
                    turn_for_events.turn_id,
                    item_id,
                    item_seq,
                    text,
                )
                .await;
            }
            tracing::debug!(
                session_id = %session_id,
                turn_id = %turn_for_events.turn_id,
                query_event_queue_max_depth =
                    event_queue_max_depth_for_task.load(Ordering::Acquire),
                query_event_queue_remaining =
                    event_queue_depth_for_task.load(Ordering::Acquire),
                "query event stream drained"
            );
            latest_usage
        });

        let (
            result,
            session_total_input_tokens,
            session_total_output_tokens,
            session_total_cache_creation_tokens,
            session_total_cache_read_tokens,
            session_last_input_tokens,
            session_prompt_token_estimate,
        ) = {
            // Run the model query only after the event pipeline is ready so
            // streamed deltas can be consumed and persisted immediately.
            let (core_session, agent_scope, agent_tool_policy) = {
                let session = session_arc.lock().await;
                let agent_scope = if session.summary.parent_session_id.is_some() {
                    ToolAgentScope::Subagent
                } else {
                    ToolAgentScope::Parent
                };
                (
                    Arc::clone(&session.core_session),
                    agent_scope,
                    session.agent_tool_policy,
                )
            };
            let turn_goal = match &input_mode {
                TurnInputMode::VisibleUserMessage => {
                    let stores = self.goal_stores.lock().await;
                    stores
                        .get(&session_id)
                        .and_then(GoalStore::get)
                        .map(crate::goal::Goal::to_thread_goal)
                }
                TurnInputMode::HiddenGoalContinuation { goal } => Some(goal.clone()),
            };
            let mut core_session = core_session.lock().await;
            core_session.collaboration_mode = collaboration_mode;
            if let Some(goal) = turn_goal {
                core_session.set_active_goal(goal);
            } else {
                core_session.clear_active_goal();
            }
            if input_mode.emits_user_message() {
                core_session.push_message(Message::user(input.clone()));
            }
            let event_callback_tx = event_tx.clone();
            let callback = std::sync::Arc::new(move |event: QueryEvent| {
                event_callback_tx.send(event);
            });
            let registry = match agent_tool_policy {
                devo_protocol::AgentToolPolicy::Inherit => Arc::clone(&self.deps.registry),
                devo_protocol::AgentToolPolicy::DenyAll => {
                    Arc::new(devo_core::tools::ToolRegistry::new())
                }
            };
            let permission_mode = core_session.config.permission_mode;
            let permission_profile = core_session.config.permission_profile.clone();
            let hook_context = self.hook_context_for_session(session_id).await;
            let turn_cancel_token = self
                .active_turn_cancellations
                .lock()
                .await
                .get(&session_id)
                .cloned()
                .unwrap_or_else(CancellationToken::new);
            let network_proxy = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned")
                .effective_config()
                .provider_http
                .proxy_url
                .clone();
            let runtime = ToolRuntime::new_with_context_and_options(
                Arc::clone(&registry),
                self.build_permission_checker(
                    session_id,
                    turn_for_events.turn_id,
                    permission_mode,
                    permission_profile,
                ),
                ToolRuntimeContext {
                    session_id: session_id.to_string(),
                    turn_id: Some(turn_for_events.turn_id.to_string()),
                    cwd: core_session.cwd.clone(),
                    agent_scope,
                    collaboration_mode,
                    agent_coordinator: Some(Arc::clone(&self) as Arc<dyn AgentToolCoordinator>),
                    local_web_search: match &turn_config.web_search {
                        devo_core::ResolvedWebSearchConfig::Local(config) => Some(config.clone()),
                        devo_core::ResolvedWebSearchConfig::Disabled
                        | devo_core::ResolvedWebSearchConfig::Provider => None,
                    },
                    hooks: hook_context,
                    network_proxy,
                },
                ToolExecutionOptions {
                    cancel_token: turn_cancel_token,
                    ..ToolExecutionOptions::default()
                },
            );
            let result = query(
                &mut core_session,
                &turn_config,
                self.deps
                    .provider_for_route(turn_config.provider_route.clone()),
                registry,
                &runtime,
                Some(callback),
            )
            .await;
            (
                result,
                core_session.total_input_tokens,
                core_session.total_output_tokens,
                core_session.total_cache_creation_tokens,
                core_session.total_cache_read_tokens,
                core_session.last_input_tokens,
                core_session.prompt_token_estimate,
            )
        };
        drop(event_tx);
        if let Err(error) = event_forwarder_task.await {
            tracing::warn!(
                session_id = %session_id,
                turn_id = %turn.turn_id,
                error = %error,
                "query event forwarder failed"
            );
        }
        // Wait for the event task to finish draining buffered stream events
        // before we persist the terminal turn state.
        let latest_usage = event_task.await.ok().flatten();
        self.active_tasks.lock().await.remove(&session_id);
        self.active_turn_cancellations
            .lock()
            .await
            .remove(&session_id);
        match &result {
            Ok(()) => {
                self.run_session_hook(
                    session_id,
                    devo_core::HookEvent::Stop,
                    serde_json::Map::from_iter([(
                        "stop_hook_active".to_string(),
                        serde_json::Value::Bool(false),
                    )]),
                )
                .await;
            }
            Err(error) => {
                self.run_session_hook(
                    session_id,
                    devo_core::HookEvent::StopFailure,
                    serde_json::Map::from_iter([
                        (
                            "error".to_string(),
                            serde_json::Value::String(error.to_string()),
                        ),
                        (
                            "error_details".to_string(),
                            serde_json::Value::String(error.to_string()),
                        ),
                    ]),
                )
                .await;
            }
        }

        let final_turn = {
            let mut session = session_arc.lock().await;
            let mut final_turn = turn.clone();
            final_turn.completed_at = Some(Utc::now());
            final_turn.status = if result.is_ok() {
                TurnStatus::Completed
            } else {
                TurnStatus::Failed
            };
            final_turn.usage = latest_usage.clone();
            session.latest_turn = Some(final_turn.clone());
            session.active_turn = None;
            session.summary.status = SessionRuntimeStatus::Idle;
            session.summary.updated_at = Utc::now();
            session.summary.total_input_tokens = session_total_input_tokens;
            session.summary.total_output_tokens = session_total_output_tokens;
            session.summary.total_cache_creation_tokens = session_total_cache_creation_tokens;
            session.summary.total_cache_read_tokens = session_total_cache_read_tokens;
            session.summary.prompt_token_estimate = session_prompt_token_estimate;
            if let Some(usage) = &final_turn.usage {
                session.summary.last_query_total_tokens =
                    usage.input_tokens as usize + usage.output_tokens as usize;
            }

            // Persist token stats to SQLite (skip for ephemeral sessions)
            if !session.summary.ephemeral {
                let stats = SessionStats {
                    total_input_tokens: session_total_input_tokens,
                    total_output_tokens: session_total_output_tokens,
                    total_cache_creation_tokens: session_total_cache_creation_tokens,
                    total_cache_read_tokens: session_total_cache_read_tokens,
                    last_input_tokens: final_turn
                        .usage
                        .as_ref()
                        .map(|u| u.input_tokens as usize)
                        .unwrap_or(session_last_input_tokens),
                    turn_count: session.summary.updated_at.timestamp() as usize,
                    prompt_token_estimate: session_prompt_token_estimate,
                };
                if let Err(err) = self.deps.db.update_stats(&session_id, &stats) {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %err,
                        "failed to persist token stats to database"
                    );
                }
            }

            final_turn
        };

        // The turn is finished, so any queued "btw" input no longer applies.
        // Clear both the in-memory queue and the persisted mirror.
        {
            let is_ephemeral = {
                let session = session_arc.lock().await;
                session.summary.ephemeral
            };
            let btw_input_queue = {
                let session = session_arc.lock().await;
                Arc::clone(&session.btw_input_queue)
            };
            btw_input_queue
                .lock()
                .expect("btw input queue mutex should not be poisoned")
                .clear();
            if !is_ephemeral
                && let Err(err) = self.deps.db.clear_pending(&session_id, QueueType::Btw)
            {
                tracing::warn!(
                    session_id = %session_id,
                    error = %err,
                    "failed to clear btw input messages from database"
                );
            }
        }

        let (record, session_context, turn_context) = {
            let session = session_arc.lock().await;
            let core_session = session.core_session.lock().await;
            (
                session.record.clone(),
                core_session.session_context.clone(),
                core_session.latest_turn_context.clone(),
            )
        };
        if let Some(record) = record
            && let Err(error) = self.rollout_store.append_turn(
                &record,
                build_turn_record(&final_turn, session_context, turn_context),
            )
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist terminal turn line");
        }
        // Emit the terminal result before we look at queued follow-up input.
        if let Err(error) = result {
            tracing::warn!(
                session_id = %session_id,
                turn_id = %final_turn.turn_id,
                status = ?final_turn.status,
                error = %error,
                "turn execution failed"
            );
            self.emit_turn_item(
                session_id,
                final_turn.turn_id,
                ItemKind::AgentMessage,
                TurnItem::AgentMessage(TextItem {
                    text: error.to_string(),
                }),
                serde_json::json!({ "title": "Error", "text": error.to_string() }),
            )
            .await;
            self.broadcast_event(ServerEvent::TurnFailed(TurnEventPayload {
                session_id,
                turn: final_turn.clone(),
            }))
            .await;
        } else {
            tracing::info!(
                session_id = %session_id,
                turn_id = %final_turn.turn_id,
                status = ?final_turn.status,
                total_input_tokens = final_turn.usage.as_ref().map(|usage| usage.input_tokens),
                total_output_tokens = final_turn.usage.as_ref().map(|usage| usage.output_tokens),
                "turn execution completed"
            );
        }
        self.handle_subagent_turn_completed(session_id, &final_turn)
            .await;
        self.broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
            session_id,
            turn: final_turn,
        }))
        .await;
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id,
                status: SessionRuntimeStatus::Idle,
            },
        ))
        .await;

        // After the turn completes, check for queued inputs and start the next turn.
        let queued_input = {
            let session_arc = match self.sessions.lock().await.get(&session_id).cloned() {
                Some(s) => s,
                None => return,
            };
            let pending_turn_queue = {
                let session = session_arc.lock().await;
                if session.active_turn.is_some() {
                    return;
                }
                Arc::clone(&session.pending_turn_queue)
            };
            let mut queue = pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            match queue.pop_front() {
                Some(devo_core::PendingInputItem {
                    kind: devo_core::PendingInputKind::UserText { text },
                    metadata,
                    ..
                }) => Some((
                    text.clone(),
                    text,
                    collaboration_mode_from_pending_metadata(metadata.as_ref()),
                )),
                Some(devo_core::PendingInputItem {
                    kind:
                        devo_core::PendingInputKind::UserInput {
                            display_text,
                            prompt_text,
                            ..
                        },
                    metadata,
                    ..
                }) => Some((
                    display_text,
                    prompt_text,
                    collaboration_mode_from_pending_metadata(metadata.as_ref()),
                )),
                _ => None,
            }
        };
        let Some((display_input, input_text, queued_collaboration_mode)) = queued_input else {
            self.maybe_start_goal_continuation_turn(session_id).await;
            return;
        };
        // Update clients before starting the next turn so dequeued input is
        // removed from any pending queue display.
        self.broadcast_updated_queue(session_id).await;

        let (turn_config, resolved_request) = {
            let session_arc = match self.sessions.lock().await.get(&session_id).cloned() {
                Some(s) => s,
                None => return,
            };
            let session = session_arc.lock().await;
            let model_override = session.summary.model.as_deref();
            let thinking_override = session.summary.thinking.clone();
            let turn_config = self
                .deps
                .resolve_turn_config(model_override, thinking_override);
            let resolved_request = turn_config
                .model
                .resolve_thinking_selection(turn_config.thinking_selection.as_deref());
            (turn_config, resolved_request)
        };
        let request_model = turn_config.provider_request_model(&resolved_request.request_model);

        let sequence = {
            let session_arc = match self.sessions.lock().await.get(&session_id).cloned() {
                Some(s) => s,
                None => return,
            };
            let session = session_arc.lock().await;
            session.latest_turn.as_ref().map_or(1, |t| t.sequence + 1)
        };

        let now = Utc::now();
        let turn = TurnMetadata {
            turn_id: TurnId::new(),
            session_id,
            sequence,
            status: TurnStatus::Running,
            kind: devo_core::TurnKind::Regular,
            model: turn_config.model.slug.clone(),
            thinking: turn_config.thinking_selection.clone(),
            reasoning_effort: resolved_request.effective_reasoning_effort,
            request_model,
            request_thinking: resolved_request.request_thinking.clone(),
            started_at: now,
            completed_at: None,
            usage: None,
        };
        {
            let session_arc = match self.sessions.lock().await.get(&session_id).cloned() {
                Some(s) => s,
                None => return,
            };
            let mut session = session_arc.lock().await;
            session.summary.status = SessionRuntimeStatus::ActiveTurn;
            session.summary.updated_at = now;
            session.active_turn = Some(turn.clone());
        }
        self.broadcast_event(ServerEvent::TurnStarted(TurnEventPayload {
            session_id,
            turn: turn.clone(),
        }))
        .await;
        // Chain directly instead of spawning so this drain loop can keep
        // consuming queued input until the queue is empty.
        Box::pin(Arc::clone(&self).execute_turn(ExecuteTurnRequest {
            session_id,
            turn,
            turn_config,
            display_input,
            input: input_text,
            collaboration_mode: queued_collaboration_mode,
            input_mode: TurnInputMode::VisibleUserMessage,
        }))
        .await;
    }

    /// Pop the first queued input and start a new turn in a background task.
    /// Used from the interrupt handler where the calling function must return
    /// its response immediately.
    pub(super) async fn spawn_next_turn_from_queue(self: &Arc<Self>, session_id: SessionId) {
        // Pop one queued input.
        let (display_input, input_text, queued_collaboration_mode) = {
            let session_arc = match self.sessions.lock().await.get(&session_id).cloned() {
                Some(s) => s,
                None => return,
            };
            let pending_turn_queue = {
                let session = session_arc.lock().await;
                Arc::clone(&session.pending_turn_queue)
            };
            let mut guard = pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            match guard.pop_front() {
                Some(devo_core::PendingInputItem {
                    kind: devo_core::PendingInputKind::UserText { text },
                    metadata,
                    ..
                }) => (
                    text.clone(),
                    text,
                    collaboration_mode_from_pending_metadata(metadata.as_ref()),
                ),
                Some(devo_core::PendingInputItem {
                    kind:
                        devo_core::PendingInputKind::UserInput {
                            display_text,
                            prompt_text,
                            ..
                        },
                    metadata,
                    ..
                }) => (
                    display_text,
                    prompt_text,
                    collaboration_mode_from_pending_metadata(metadata.as_ref()),
                ),
                _ => return,
            }
        };
        // Broadcast the updated queue state so the TUI removes this item
        // from its pending cells list.
        self.broadcast_updated_queue(session_id).await;

        // Resolve turn config from session metadata.
        let (turn_config, resolved_request) = {
            let session_arc = match self.sessions.lock().await.get(&session_id).cloned() {
                Some(s) => s,
                None => return,
            };
            let session = session_arc.lock().await;
            let model = session.summary.model.as_deref();
            let thinking = session.summary.thinking.clone();
            let tc = self.deps.resolve_turn_config(model, thinking);
            let rr = tc
                .model
                .resolve_thinking_selection(tc.thinking_selection.as_deref());
            (tc, rr)
        };
        let request_model = turn_config.provider_request_model(&resolved_request.request_model);

        let sequence = {
            let session_arc = match self.sessions.lock().await.get(&session_id).cloned() {
                Some(s) => s,
                None => return,
            };
            let session = session_arc.lock().await;
            session.latest_turn.as_ref().map_or(1, |t| t.sequence + 1)
        };

        let now = Utc::now();
        let turn = TurnMetadata {
            turn_id: TurnId::new(),
            session_id,
            sequence,
            status: TurnStatus::Running,
            kind: devo_core::TurnKind::Regular,
            model: turn_config.model.slug.clone(),
            thinking: turn_config.thinking_selection.clone(),
            reasoning_effort: resolved_request.effective_reasoning_effort,
            request_model,
            request_thinking: resolved_request.request_thinking.clone(),
            started_at: now,
            completed_at: None,
            usage: None,
        };
        {
            let session_arc = match self.sessions.lock().await.get(&session_id).cloned() {
                Some(s) => s,
                None => return,
            };
            let mut session = session_arc.lock().await;
            session.summary.status = SessionRuntimeStatus::ActiveTurn;
            session.summary.updated_at = now;
            session.active_turn = Some(turn.clone());
        }
        self.broadcast_event(ServerEvent::TurnStarted(TurnEventPayload {
            session_id,
            turn: turn.clone(),
        }))
        .await;
        // Spawn the turn in the background so the caller (interrupt handler)
        // can return its response immediately. The spawned task will call
        // drain_and_start_next_turn on completion, draining the entire queue.
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            runtime
                .execute_turn(ExecuteTurnRequest {
                    session_id,
                    turn,
                    turn_config,
                    display_input,
                    input: input_text,
                    collaboration_mode: queued_collaboration_mode,
                    input_mode: TurnInputMode::VisibleUserMessage,
                })
                .await;
        });
    }

    /// Read the current steering queue and broadcast its state to connected clients.
    /// Called after any queue mutation (enqueue, dequeue, clear) so the TUI preview
    /// stays in sync.
    pub(super) async fn broadcast_updated_queue(&self, session_id: SessionId) {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return;
        };
        let (pending_count, pending_texts) = {
            let pending_turn_queue = {
                let session = session_arc.lock().await;
                Arc::clone(&session.pending_turn_queue)
            };
            let queue = pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            let texts: Vec<String> = queue
                .iter()
                .filter_map(|item| match &item.kind {
                    devo_core::PendingInputKind::UserText { text } => Some(text.clone()),
                    devo_core::PendingInputKind::UserInput { display_text, .. } => {
                        Some(display_text.clone())
                    }
                    _ => None,
                })
                .collect();
            (texts.len(), texts)
        };
        self.broadcast_event(ServerEvent::InputQueueUpdated(
            devo_core::InputQueueUpdatedPayload {
                session_id,
                pending_count,
                pending_texts,
            },
        ))
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn command_progress_uses_command_execution_item_id() {
        let command_item_id = ItemId::new();
        let tool_item_id = ItemId::new();
        let mut pending_tool_calls = HashMap::new();
        pending_tool_calls.insert(
            "exec".to_string(),
            PendingToolCall {
                item_id: Some(command_item_id),
                item_seq: Some(1),
                input: serde_json::json!({}),
                display_kind: ToolDisplayKind::CommandExecution,
                command: "cargo test".to_string(),
            },
        );
        pending_tool_calls.insert(
            "read".to_string(),
            PendingToolCall {
                item_id: Some(tool_item_id),
                item_seq: Some(2),
                input: serde_json::json!({}),
                display_kind: ToolDisplayKind::Generic,
                command: String::new(),
            },
        );

        assert_eq!(
            command_execution_item_id_for_progress(&pending_tool_calls, "exec"),
            Some(command_item_id)
        );
        assert_eq!(
            command_execution_item_id_for_progress(&pending_tool_calls, "read"),
            None
        );
        assert_eq!(
            command_execution_item_id_for_progress(&pending_tool_calls, "missing"),
            None
        );
    }

    #[test]
    fn file_change_tool_detection_matches_apply_patch_and_write() {
        assert!(is_file_change_tool("apply_patch"));
        assert!(is_file_change_tool("write"));
        assert!(!is_file_change_tool("read"));
    }

    #[test]
    fn plan_tool_detection_matches_update_plan() {
        assert!(is_plan_tool("update_plan"));
        assert!(!is_plan_tool("read"));
    }

    #[test]
    fn read_tool_start_item_contains_live_read_action() {
        let input = serde_json::json!({
            "path": "crates/tui/src/mod.rs"
        });
        let start_item = tool_start_item_from_input(
            "call-1",
            "read",
            "read crates/tui/src/mod.rs",
            &input,
            ToolDisplayKind::Generic,
            ToolPreparationFeedback::None,
        );

        let payload: ToolCallPayload =
            serde_json::from_value(start_item.payload).expect("tool call payload");

        assert_eq!(start_item.item_kind, ItemKind::ToolCall);
        assert_eq!(
            payload,
            ToolCallPayload {
                tool_call_id: "call-1".to_string(),
                tool_name: "read".to_string(),
                parameters: input,
                command_actions: vec![devo_protocol::parse_command::ParsedCommand::Read {
                    cmd: "read crates/tui/src/mod.rs".to_string(),
                    name: "mod.rs".to_string(),
                    path: std::path::PathBuf::from("crates/tui/src/mod.rs"),
                }],
            }
        );
    }

    #[test]
    fn grep_tool_start_item_contains_live_search_action() {
        let input = serde_json::json!({
            "pattern": "ToolUseStart",
            "path": "crates/server/src"
        });
        let start_item = tool_start_item_from_input(
            "call-1",
            "grep",
            "grep ToolUseStart in crates/server/src",
            &input,
            ToolDisplayKind::Generic,
            ToolPreparationFeedback::None,
        );

        let payload: ToolCallPayload =
            serde_json::from_value(start_item.payload).expect("tool call payload");

        assert_eq!(start_item.item_kind, ItemKind::ToolCall);
        assert_eq!(
            payload,
            ToolCallPayload {
                tool_call_id: "call-1".to_string(),
                tool_name: "grep".to_string(),
                parameters: input,
                command_actions: vec![devo_protocol::parse_command::ParsedCommand::Search {
                    cmd: "grep ToolUseStart in crates/server/src".to_string(),
                    query: Some("ToolUseStart".to_string()),
                    path: Some("crates/server/src".to_string()),
                }],
            }
        );
    }

    #[test]
    fn code_search_tool_start_item_contains_live_search_action() {
        let input = serde_json::json!({
            "operation": "search",
            "query": "live tool feedback",
            "path": "crates"
        });
        let start_item = tool_start_item_from_input(
            "call-1",
            "code_search",
            "code_search live tool feedback in crates",
            &input,
            ToolDisplayKind::Generic,
            ToolPreparationFeedback::None,
        );

        let payload: ToolCallPayload =
            serde_json::from_value(start_item.payload).expect("tool call payload");

        assert_eq!(start_item.item_kind, ItemKind::ToolCall);
        assert_eq!(
            payload,
            ToolCallPayload {
                tool_call_id: "call-1".to_string(),
                tool_name: "code_search".to_string(),
                parameters: input,
                command_actions: vec![devo_protocol::parse_command::ParsedCommand::Search {
                    cmd: "code_search live tool feedback in crates".to_string(),
                    query: Some("live tool feedback".to_string()),
                    path: Some("crates".to_string()),
                }],
            }
        );
    }

    #[test]
    fn exec_tool_start_item_uses_command_execution_payload() {
        let input = serde_json::json!({
            "cmd": "cargo test -p devo-server"
        });
        let start_item = tool_start_item_from_input(
            "call-1",
            "exec_command",
            "cargo test -p devo-server",
            &input,
            ToolDisplayKind::CommandExecution,
            ToolPreparationFeedback::None,
        );

        let payload: CommandExecutionPayload =
            serde_json::from_value(start_item.payload).expect("command execution payload");

        assert_eq!(start_item.item_kind, ItemKind::CommandExecution);
        assert_eq!(
            payload,
            CommandExecutionPayload {
                tool_call_id: "call-1".to_string(),
                tool_name: "exec_command".to_string(),
                command: "cargo test -p devo-server".to_string(),
                input: Some(input),
                source: devo_protocol::protocol::ExecCommandSource::Agent,
                command_actions: Vec::new(),
                output: None,
                is_error: false,
            }
        );
    }

    #[test]
    fn user_shell_exec_input_uses_pty_backed_exec_command() {
        let cwd = std::path::PathBuf::from("workspace");

        let input = user_shell_exec_input("pwd", cwd.clone());

        assert_eq!(
            input,
            serde_json::json!({
                "cmd": "pwd",
                "workdir": cwd,
                "login": true,
                "tty": true,
            })
        );
    }

    #[test]
    fn user_shell_command_payload_uses_user_shell_source() {
        let output = serde_json::json!({ "output": "done" });

        let input = user_shell_exec_input("pwd", std::path::PathBuf::from("workspace"));
        let payload = user_shell_command_payload(
            "call-1",
            "pwd",
            input.clone(),
            Vec::new(),
            Some(output.clone()),
            false,
        );

        assert_eq!(
            payload,
            CommandExecutionPayload {
                tool_call_id: "call-1".to_string(),
                tool_name: "exec_command".to_string(),
                command: "pwd".to_string(),
                input: Some(input),
                source: devo_protocol::protocol::ExecCommandSource::UserShell,
                command_actions: Vec::new(),
                output: Some(output),
                is_error: false,
            }
        );
    }

    #[test]
    fn live_only_apply_patch_start_item_stays_tool_call() {
        let input = serde_json::json!({
            "patch": "*** Begin Patch\n*** End Patch"
        });
        let start_item = tool_start_item_from_input(
            "call-1",
            "apply_patch",
            "apply_patch",
            &input,
            ToolDisplayKind::Generic,
            ToolPreparationFeedback::LiveOnly,
        );

        let payload: ToolCallPayload =
            serde_json::from_value(start_item.payload).expect("tool call payload");

        assert_eq!(start_item.item_kind, ItemKind::ToolCall);
        assert_eq!(
            payload,
            ToolCallPayload {
                tool_call_id: "call-1".to_string(),
                tool_name: "apply_patch".to_string(),
                parameters: input,
                command_actions: Vec::new(),
            }
        );
    }

    #[tokio::test]
    async fn bounded_query_event_bridge_preserves_order_and_depth() {
        let queue_depth = Arc::new(AtomicUsize::new(0));
        let queue_max_depth = Arc::new(AtomicUsize::new(0));
        let (sender, mut rx, forwarder) = bounded_query_event_channel(
            /*capacity*/ 2,
            Arc::clone(&queue_depth),
            Arc::clone(&queue_max_depth),
        );

        sender.send(QueryEvent::TextDelta("one".to_string()));
        sender.send(QueryEvent::ReasoningDelta("two".to_string()));
        drop(sender);

        let first = rx.recv().await.expect("first event");
        decrement_query_event_queue_depth(&queue_depth);
        let second = rx.recv().await.expect("second event");
        decrement_query_event_queue_depth(&queue_depth);
        forwarder.await.expect("forwarder");

        assert!(matches!(first, QueryEvent::TextDelta(text) if text == "one"));
        assert!(matches!(second, QueryEvent::ReasoningDelta(text) if text == "two"));
        assert_eq!(queue_depth.load(Ordering::Acquire), 0);
        assert!(queue_max_depth.load(Ordering::Acquire) >= 1);
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn bounded_query_event_bridge_keeps_terminal_event_after_backpressure() {
        let queue_depth = Arc::new(AtomicUsize::new(0));
        let queue_max_depth = Arc::new(AtomicUsize::new(0));
        let (sender, mut rx, forwarder) = bounded_query_event_channel(
            /*capacity*/ 1,
            Arc::clone(&queue_depth),
            Arc::clone(&queue_max_depth),
        );
        let sender_for_task = sender.clone();
        let send_task = tokio::task::spawn_blocking(move || {
            sender_for_task.send(QueryEvent::TextDelta("first".to_string()));
            sender_for_task.send(QueryEvent::TextDelta("second".to_string()));
            sender_for_task.send(QueryEvent::TurnComplete {
                stop_reason: devo_core::StopReason::EndTurn,
            });
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        let first = rx.recv().await.expect("first event");
        decrement_query_event_queue_depth(&queue_depth);
        let second = rx.recv().await.expect("second event");
        decrement_query_event_queue_depth(&queue_depth);
        let terminal = rx.recv().await.expect("terminal event");
        decrement_query_event_queue_depth(&queue_depth);

        send_task.await.expect("send task");
        drop(sender);
        forwarder.await.expect("forwarder");

        assert!(matches!(first, QueryEvent::TextDelta(text) if text == "first"));
        assert!(matches!(second, QueryEvent::TextDelta(text) if text == "second"));
        assert!(matches!(
            terminal,
            QueryEvent::TurnComplete {
                stop_reason: devo_core::StopReason::EndTurn,
            }
        ));
        assert_eq!(queue_depth.load(Ordering::Acquire), 0);
        assert!(queue_max_depth.load(Ordering::Acquire) >= 2);
    }

    #[test]
    fn command_actions_from_read_tool_input_builds_read_action() {
        let actions = command_actions_from_tool_input(
            "read",
            "read crates/tui/src/mod.rs",
            &serde_json::json!({
                "filePath": "crates/tui/src/mod.rs"
            }),
        );
        assert_eq!(
            actions,
            vec![devo_protocol::parse_command::ParsedCommand::Read {
                cmd: "read crates/tui/src/mod.rs".to_string(),
                name: "mod.rs".to_string(),
                path: std::path::PathBuf::from("crates/tui/src/mod.rs"),
            }]
        );
    }

    #[test]
    fn command_actions_from_read_tool_input_without_path_is_empty() {
        let actions =
            command_actions_from_tool_input("read", "read", &serde_json::json!({ "limit": 10 }));
        assert_eq!(actions, Vec::new());
    }

    #[test]
    fn command_actions_from_read_tool_result_summary_recovers_final_path() {
        let actions = command_actions_from_tool_result(
            "read",
            "read ",
            &serde_json::json!({}),
            "read: crates/tui/src/mod.rs",
        );
        assert_eq!(
            actions,
            vec![devo_protocol::parse_command::ParsedCommand::Read {
                cmd: "read crates/tui/src/mod.rs".to_string(),
                name: "mod.rs".to_string(),
                path: std::path::PathBuf::from("crates/tui/src/mod.rs"),
            }]
        );
    }

    #[test]
    fn command_actions_from_grep_tool_input_builds_search_action() {
        let actions = command_actions_from_tool_input(
            "grep",
            "grep rebuild_restored_session in crates/tui/src",
            &serde_json::json!({
                "pattern": "rebuild_restored_session",
                "path": "crates/tui/src"
            }),
        );
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            devo_protocol::parse_command::ParsedCommand::Search { query, path, .. }
            if query.as_deref() == Some("rebuild_restored_session")
                && path.as_deref() == Some("crates/tui/src")
        ));
    }

    #[test]
    fn command_actions_from_glob_tool_input_include_pattern_and_path() {
        let actions = command_actions_from_tool_input(
            "glob",
            "glob **/Cargo.toml in crates",
            &serde_json::json!({
                "pattern": "**/Cargo.toml",
                "path": "crates"
            }),
        );
        assert_eq!(
            actions,
            vec![devo_protocol::parse_command::ParsedCommand::ListFiles {
                cmd: "glob **/Cargo.toml in crates".to_string(),
                path: Some("**/Cargo.toml in crates".to_string()),
            }]
        );
    }

    #[test]
    fn command_actions_from_find_tool_input_include_pattern_and_path() {
        let actions = command_actions_from_tool_input(
            "find",
            "find **/Cargo.toml in crates",
            &serde_json::json!({
                "pattern": "**/Cargo.toml",
                "path": "crates"
            }),
        );
        assert_eq!(
            actions,
            vec![devo_protocol::parse_command::ParsedCommand::ListFiles {
                cmd: "find **/Cargo.toml in crates".to_string(),
                path: Some("**/Cargo.toml in crates".to_string()),
            }]
        );
    }
}
