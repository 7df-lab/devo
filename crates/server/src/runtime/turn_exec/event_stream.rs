use std::sync::Arc;

use devo_core::{ItemId, SessionId, TurnId};
use devo_protocol::TurnFailureReason;

use super::super::ServerRuntime;
use super::super::proposed_plan::ProposedPlanParser;
use super::item_stream::{
    ProposedPlanStreamItem, complete_assistant_item, complete_reasoning_item,
    handle_proposed_plan_segments, push_assistant_text_delta,
};
use super::tool_display::{
    command_display_from_input, tool_start_item_from_input, tool_start_item_from_result,
};
use super::tool_results;
use super::tool_results::complete_pending_tool_call;
use super::trace::{
    query_event_trace_delta_len, query_event_trace_kind, query_event_trace_token_preview,
    stream_trace_elapsed_ms,
};
use super::types::{PendingToolCall, ToolDisplayKind, TurnEventStreamSummary};
use crate::runtime::session_actor::state::SessionStreamState;
use crate::{ItemDeltaKind, ItemDeltaPayload, ServerEvent};
use tokio::sync::mpsc;

pub(crate) const QUERY_EVENT_CHANNEL_CAPACITY: usize = 1024;

/// Enqueue a query event without blocking the caller when the channel is full.
///
/// `run_turn_model_query` holds the session `core_session` mutex for the entire
/// `query()` call. Blocking on `event_tx.send().await` while the event task is
/// stalled on client-notification backpressure would freeze tool execution
/// (including `spawn_agent`) and can wedge the whole connection.
pub(super) fn enqueue_query_event(
    event_tx: &mpsc::Sender<devo_core::QueryEvent>,
    event: devo_core::QueryEvent,
) {
    match event_tx.try_send(event) {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(event)) => {
            let event_tx = event_tx.clone();
            tokio::spawn(async move {
                let _ = event_tx.send(event).await;
            });
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
    }
}

pub(super) fn turn_failure_reason_from_error(
    error: &devo_core::AgentError,
) -> Option<TurnFailureReason> {
    match error {
        devo_core::AgentError::MaxTurnsExceeded(_) => Some(TurnFailureReason::MaxTurnRequests),
        devo_core::AgentError::Provider(_)
        | devo_core::AgentError::ContextTooLong
        | devo_core::AgentError::Aborted => None,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_turn_event_stream(
    runtime: Arc<ServerRuntime>,
    event_stream: Arc<tokio::sync::Mutex<SessionStreamState>>,
    session_id: SessionId,
    turn: crate::TurnMetadata,
    collaboration_mode: devo_protocol::CollaborationMode,
    event_tool_registry: Arc<devo_core::tools::ToolRegistry>,
    usage_parent_session_id: Option<SessionId>,
    usage_context_window: Option<u64>,
    mut event_rx: tokio::sync::mpsc::Receiver<devo_core::QueryEvent>,
) -> tokio::task::JoinHandle<TurnEventStreamSummary> {
    let turn_for_events = turn.clone();
    let turn_for_plan_updates = turn;
    tokio::spawn(async move {
        let mut assistant_item_id = None;
        let mut assistant_item_seq = None;
        let mut assistant_delta_seq = 0_u64;
        let mut assistant_text = String::new();
        let mut reasoning_item_id = None;
        let mut reasoning_item_seq = None;
        let mut reasoning_text = String::new();
        let mut tool_names_by_id = std::collections::HashMap::new();
        let mut pending_tool_calls: std::collections::HashMap<String, PendingToolCall> =
            std::collections::HashMap::new();
        let mut proposed_plan_parser = (collaboration_mode
            == devo_protocol::CollaborationMode::Plan)
            .then(ProposedPlanParser::default);
        let mut proposed_plan_item = ProposedPlanStreamItem::default();
        let mut proposed_plan_leading_normal = String::new();
        let mut latest_usage = None;
        let mut stop_reason = None;
        while let Some(event) = event_rx.recv().await {
            log_dequeued_query_event(&event);
            match event {
                devo_core::QueryEvent::TextDelta(text) => {
                    if let Some(parser) = proposed_plan_parser.as_mut() {
                        let segments = parser.push_str(&text);
                        handle_proposed_plan_segments(
                            &runtime,
                            &event_stream,
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
                            &event_stream,
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
                devo_core::QueryEvent::ReasoningDelta(text) => {
                    handle_reasoning_delta(
                        &runtime,
                        &event_stream,
                        session_id,
                        turn_for_events.turn_id,
                        text,
                        &mut reasoning_item_id,
                        &mut reasoning_item_seq,
                        &mut reasoning_text,
                    )
                    .await;
                }
                devo_core::QueryEvent::ReasoningCompleted => {
                    complete_open_reasoning_item(
                        &runtime,
                        session_id,
                        turn_for_events.turn_id,
                        &mut reasoning_item_id,
                        &mut reasoning_item_seq,
                        &mut reasoning_text,
                        &event_stream,
                    )
                    .await;
                }
                devo_core::QueryEvent::ToolUseStart { id, name, input } => {
                    handle_tool_use_start(
                        &runtime,
                        session_id,
                        turn_for_events.turn_id,
                        id,
                        name,
                        input,
                        &mut tool_names_by_id,
                        &mut pending_tool_calls,
                        &mut reasoning_item_id,
                        &mut reasoning_item_seq,
                        &mut reasoning_text,
                        &mut assistant_item_id,
                        &mut assistant_item_seq,
                        &mut assistant_text,
                        &event_tool_registry,
                    )
                    .await;
                }
                devo_core::QueryEvent::ToolExecutionStart { id } => {
                    runtime
                        .broadcast_event(ServerEvent::ToolCallStatusUpdated(
                            devo_protocol::ToolCallStatusUpdatedPayload {
                                session_id,
                                turn_id: turn_for_events.turn_id,
                                tool_call_id: id,
                                status: "in_progress".to_string(),
                                terminal_id: None,
                            },
                        ))
                        .await;
                }
                devo_core::QueryEvent::ToolResult {
                    tool_use_id,
                    tool_name: final_tool_name,
                    input: final_input,
                    content,
                    display_content,
                    is_error,
                    summary,
                } => {
                    handle_tool_result(
                        &runtime,
                        session_id,
                        turn_for_events.turn_id,
                        &turn_for_plan_updates,
                        tool_use_id,
                        final_tool_name,
                        final_input,
                        content,
                        display_content,
                        is_error,
                        summary,
                        &tool_names_by_id,
                        &mut pending_tool_calls,
                        &event_tool_registry,
                    )
                    .await;
                }
                devo_core::QueryEvent::ToolProgress {
                    tool_use_id,
                    progress,
                } => {
                    handle_tool_progress(
                        &runtime,
                        session_id,
                        turn_for_events.turn_id,
                        tool_use_id,
                        progress,
                        &pending_tool_calls,
                    )
                    .await;
                }
                devo_core::QueryEvent::UsageDelta { usage }
                | devo_core::QueryEvent::Usage { usage } => {
                    let turn_usage = devo_core::TurnUsage::from_usage(&usage);
                    latest_usage = Some(turn_usage.clone());
                    if usage_parent_session_id.is_some() {
                        let _ = runtime
                            .publish_subagent_turn_usage(
                                session_id,
                                turn_for_events.turn_id,
                                turn_usage,
                            )
                            .await;
                    } else if let Some(snapshot) = runtime
                        .publish_parent_turn_usage(
                            session_id,
                            turn_for_events.turn_id,
                            turn_usage,
                            usage_context_window,
                        )
                        .await
                    {
                        latest_usage = Some(snapshot.turn_usage.to_turn_usage());
                    }
                }
                devo_core::QueryEvent::TurnComplete {
                    stop_reason: terminal_stop_reason,
                } => {
                    stop_reason = Some(terminal_stop_reason);
                }
            }
        }
        finish_proposed_plan_stream(
            &runtime,
            &event_stream,
            session_id,
            turn_for_events.turn_id,
            &mut proposed_plan_parser,
            &mut assistant_item_id,
            &mut assistant_item_seq,
            &mut assistant_text,
            &mut assistant_delta_seq,
            &mut proposed_plan_item,
            &mut proposed_plan_leading_normal,
        )
        .await;
        complete_deferred_stream_items(
            &runtime,
            &event_stream,
            session_id,
            turn_for_events.turn_id,
        )
        .await;
        tracing::debug!(
            session_id = %session_id,
            turn_id = %turn_for_events.turn_id,
            "query event stream drained"
        );
        TurnEventStreamSummary {
            latest_usage,
            stop_reason,
        }
    })
}

fn log_dequeued_query_event(event: &devo_core::QueryEvent) {
    let assistant_token_text = query_event_trace_token_preview(event);
    if let Some(assistant_token_text) = assistant_token_text.as_deref() {
        tracing::debug!(
            stream_elapsed_ms = stream_trace_elapsed_ms(),
            event_kind = query_event_trace_kind(event),
            delta_len = query_event_trace_delta_len(event),
            assistant_token_text,
            "query event bridge dequeued by turn event task"
        );
    } else {
        tracing::debug!(
            stream_elapsed_ms = stream_trace_elapsed_ms(),
            event_kind = query_event_trace_kind(event),
            delta_len = query_event_trace_delta_len(event),
            "query event bridge dequeued by turn event task"
        );
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_reasoning_delta(
    runtime: &Arc<ServerRuntime>,
    event_stream: &Arc<tokio::sync::Mutex<SessionStreamState>>,
    session_id: SessionId,
    turn_id: TurnId,
    text: String,
    reasoning_item_id: &mut Option<ItemId>,
    reasoning_item_seq: &mut Option<u64>,
    reasoning_text: &mut String,
) {
    let (item_id, item_seq) = match (*reasoning_item_id, *reasoning_item_seq) {
        (Some(item_id), Some(item_seq)) => (item_id, item_seq),
        (None, None) => {
            let (item_id, item_seq) = runtime
                .start_item(
                    session_id,
                    turn_id,
                    crate::ItemKind::Reasoning,
                    serde_json::json!({ "title": "Reasoning", "text": "" }),
                )
                .await;
            *reasoning_item_id = Some(item_id);
            *reasoning_item_seq = Some(item_seq);
            (item_id, item_seq)
        }
        _ => return,
    };
    reasoning_text.push_str(&text);
    runtime
        .broadcast_event(ServerEvent::ItemDelta {
            delta_kind: ItemDeltaKind::ReasoningTextDelta,
            payload: ItemDeltaPayload {
                context: crate::EventContext {
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
    let _ = item_seq;
    if let Ok(mut stream) = event_stream.try_lock() {
        stream.deferred_reasoning = Some((item_id, item_seq, reasoning_text.clone()));
    }
}

async fn complete_open_reasoning_item(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    reasoning_item_id: &mut Option<ItemId>,
    reasoning_item_seq: &mut Option<u64>,
    reasoning_text: &mut String,
    event_stream: &Arc<tokio::sync::Mutex<SessionStreamState>>,
) {
    if let (Some(item_id), Some(item_seq)) = (reasoning_item_id.take(), reasoning_item_seq.take()) {
        if let Ok(mut stream) = event_stream.try_lock() {
            stream.deferred_reasoning.take();
        }
        complete_reasoning_item(
            runtime,
            session_id,
            turn_id,
            item_id,
            item_seq,
            reasoning_text.clone(),
        )
        .await;
        reasoning_text.clear();
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_tool_use_start(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    id: String,
    name: String,
    input: serde_json::Value,
    tool_names_by_id: &mut std::collections::HashMap<String, String>,
    pending_tool_calls: &mut std::collections::HashMap<String, PendingToolCall>,
    reasoning_item_id: &mut Option<ItemId>,
    reasoning_item_seq: &mut Option<u64>,
    reasoning_text: &mut String,
    assistant_item_id: &mut Option<ItemId>,
    assistant_item_seq: &mut Option<u64>,
    assistant_text: &mut String,
    event_tool_registry: &Arc<devo_core::tools::ToolRegistry>,
) {
    tool_names_by_id.insert(id.clone(), name.clone());
    if let (Some(item_id), Some(item_seq)) = (reasoning_item_id.take(), reasoning_item_seq.take()) {
        complete_reasoning_item(
            runtime,
            session_id,
            turn_id,
            item_id,
            item_seq,
            reasoning_text.clone(),
        )
        .await;
        reasoning_text.clear();
    }
    if let (Some(item_id), Some(item_seq)) = (assistant_item_id.take(), assistant_item_seq.take()) {
        complete_assistant_item(
            runtime,
            session_id,
            turn_id,
            item_id,
            item_seq,
            assistant_text.clone(),
        )
        .await;
        assistant_text.clear();
    }
    let display_kind = ToolDisplayKind::for_tool_name(&name);
    let command = command_display_from_input(&name, &input);
    let preparation_feedback = event_tool_registry.preparation_feedback(&name);
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
            turn_id,
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

#[allow(clippy::too_many_arguments)]
async fn handle_tool_result(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    turn_for_plan_updates: &crate::TurnMetadata,
    tool_use_id: String,
    final_tool_name: String,
    final_input: serde_json::Value,
    content: devo_core::tools::ToolContent,
    display_content: Option<String>,
    is_error: bool,
    summary: String,
    tool_names_by_id: &std::collections::HashMap<String, String>,
    pending_tool_calls: &mut std::collections::HashMap<String, PendingToolCall>,
    event_tool_registry: &Arc<devo_core::tools::ToolRegistry>,
) {
    let tool_name = if final_tool_name.is_empty() {
        tool_names_by_id.get(&tool_use_id).cloned()
    } else {
        Some(final_tool_name)
    };
    let mut result_input = (!final_input.is_null()).then(|| final_input.clone());
    if let Some(mut pending) = pending_tool_calls.remove(&tool_use_id) {
        if !final_input.is_null() {
            pending.command = tool_name
                .as_deref()
                .map(|tool_name| command_display_from_input(tool_name, &final_input))
                .unwrap_or_default();
            pending.input = final_input;
        }
        result_input = Some(pending.input.clone());
        if (pending.item_id.is_none() || pending.item_seq.is_none())
            && let Some(tool_name) = tool_name.clone()
        {
            let preparation_feedback = event_tool_registry.preparation_feedback(&tool_name);
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
                    turn_id,
                    start_item.item_kind,
                    start_item.payload,
                )
                .await;
            pending.item_id = Some(item_id);
            pending.item_seq = Some(item_seq);
        }
        if complete_pending_tool_call(
            runtime,
            session_id,
            turn_id,
            turn_for_plan_updates,
            &tool_use_id,
            tool_name.clone(),
            &pending,
            &content,
            display_content.clone(),
            is_error,
            &summary,
        )
        .await
        {
            return;
        }
    }
    tool_results::emit_tool_result_item(
        runtime,
        session_id,
        turn_id,
        tool_use_id,
        tool_name,
        result_input,
        content,
        display_content,
        is_error,
        summary,
    )
    .await;
}

async fn handle_tool_progress(
    runtime: &Arc<ServerRuntime>,
    session_id: SessionId,
    turn_id: TurnId,
    tool_use_id: String,
    progress: devo_core::tools::ToolProgress,
    pending_tool_calls: &std::collections::HashMap<String, PendingToolCall>,
) {
    let content = match progress {
        devo_core::tools::ToolProgress::OutputDelta { delta } => Some(delta),
        devo_core::tools::ToolProgress::StatusUpdate { message, percent } => Some(match percent {
            Some(percent) => format!("{message} ({percent}%)"),
            None => message,
        }),
        devo_core::tools::ToolProgress::Completion { summary } => Some(summary),
        devo_core::tools::ToolProgress::Terminal { terminal_id } => {
            runtime
                .broadcast_event(ServerEvent::ToolCallStatusUpdated(
                    devo_protocol::ToolCallStatusUpdatedPayload {
                        session_id,
                        turn_id,
                        tool_call_id: tool_use_id.clone(),
                        status: "in_progress".to_string(),
                        terminal_id: Some(terminal_id),
                    },
                ))
                .await;
            None
        }
    };
    let Some(content) = content else {
        return;
    };
    let item_id = super::tool_display::command_execution_item_id_for_progress(
        pending_tool_calls,
        &tool_use_id,
    );
    let _ = runtime
        .broadcast_event(ServerEvent::ItemDelta {
            delta_kind: ItemDeltaKind::CommandExecutionOutputDelta,
            payload: ItemDeltaPayload {
                context: crate::EventContext {
                    session_id,
                    turn_id: Some(turn_id),
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

#[allow(clippy::too_many_arguments)]
async fn finish_proposed_plan_stream(
    runtime: &Arc<ServerRuntime>,
    event_stream: &Arc<tokio::sync::Mutex<SessionStreamState>>,
    session_id: SessionId,
    turn_id: TurnId,
    proposed_plan_parser: &mut Option<ProposedPlanParser>,
    assistant_item_id: &mut Option<ItemId>,
    assistant_item_seq: &mut Option<u64>,
    assistant_text: &mut String,
    assistant_delta_seq: &mut u64,
    proposed_plan_item: &mut ProposedPlanStreamItem,
    proposed_plan_leading_normal: &mut String,
) {
    if let Some(parser) = proposed_plan_parser.as_mut() {
        let segments = parser.finish();
        handle_proposed_plan_segments(
            runtime,
            event_stream,
            session_id,
            turn_id,
            segments,
            assistant_item_id,
            assistant_item_seq,
            assistant_text,
            assistant_delta_seq,
            proposed_plan_item,
            proposed_plan_leading_normal,
        )
        .await;
        proposed_plan_item
            .complete(runtime, session_id, turn_id)
            .await;
    }
}

async fn complete_deferred_stream_items(
    runtime: &Arc<ServerRuntime>,
    event_stream: &Arc<tokio::sync::Mutex<SessionStreamState>>,
    session_id: SessionId,
    turn_id: TurnId,
) {
    if let Some((item_id, item_seq, text)) = {
        let mut stream = event_stream.lock().await;
        stream.deferred_reasoning.take()
    } {
        complete_reasoning_item(runtime, session_id, turn_id, item_id, item_seq, text).await;
    }
    if let Some((item_id, item_seq, text)) = {
        let mut stream = event_stream.lock().await;
        stream.deferred_assistant.take()
    } {
        complete_assistant_item(runtime, session_id, turn_id, item_id, item_seq, text).await;
    }
}
