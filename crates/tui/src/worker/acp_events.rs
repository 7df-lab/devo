use std::collections::HashMap;
use std::collections::HashSet;

use devo_core::SessionId;
use devo_protocol::AcpContentBlock;
use devo_protocol::AcpEmbeddedResource;
use devo_protocol::AcpPlanEntryStatus;
use devo_protocol::AcpSessionNotification;
use devo_protocol::AcpSessionUpdate;
use devo_protocol::AcpToolCallContent;
use devo_protocol::AcpToolCallStatus;
use devo_protocol::AcpToolKind;
use devo_protocol::ItemId;

use crate::events::PlanStep;
use crate::events::PlanStepStatus;
use crate::events::TextItemKind;
use crate::events::WorkerEvent;

struct AcpToolCallEventData {
    tool_call_id: String,
    title: Option<String>,
    status: Option<AcpToolCallStatus>,
    raw_input: Option<serde_json::Value>,
    raw_output: Option<serde_json::Value>,
    content: Vec<AcpToolCallContent>,
}

struct AcpTerminalRenderState<'a> {
    visible_terminal_ids: &'a mut HashSet<String>,
    pending_terminal_output: &'a mut HashMap<String, String>,
}

struct AcpSessionUpdateRender<'a> {
    session_id: SessionId,
    update: AcpSessionUpdate,
    terminal_state: AcpTerminalRenderState<'a>,
}

impl From<AcpSessionUpdateRender<'_>> for Vec<WorkerEvent> {
    fn from(render: AcpSessionUpdateRender<'_>) -> Self {
        let AcpSessionUpdateRender {
            session_id,
            update,
            terminal_state,
        } = render;
        match update {
            AcpSessionUpdate::AgentMessageChunk {
                content,
                message_id,
                ..
            } => acp_content_display_text(&content)
                .into_iter()
                .map(|delta| {
                    if let Some(item_id) = message_item_id(message_id.as_deref()) {
                        WorkerEvent::TextItemDelta {
                            item_id,
                            kind: TextItemKind::Assistant,
                            research: None,
                            delta,
                        }
                    } else {
                        WorkerEvent::TextDelta(delta)
                    }
                })
                .collect(),
            AcpSessionUpdate::AgentThoughtChunk {
                content,
                message_id,
                ..
            } => acp_content_display_text(&content)
                .into_iter()
                .map(|delta| {
                    if let Some(item_id) = message_item_id(message_id.as_deref()) {
                        WorkerEvent::TextItemDelta {
                            item_id,
                            kind: TextItemKind::Reasoning,
                            research: None,
                            delta,
                        }
                    } else {
                        WorkerEvent::ReasoningDelta(delta)
                    }
                })
                .collect(),
            AcpSessionUpdate::Plan { entries, .. } => vec![WorkerEvent::PlanUpdated {
                explanation: None,
                steps: entries
                    .into_iter()
                    .map(|entry| PlanStep {
                        text: entry.content,
                        status: match entry.status {
                            AcpPlanEntryStatus::Pending => PlanStepStatus::Pending,
                            AcpPlanEntryStatus::InProgress => PlanStepStatus::InProgress,
                            AcpPlanEntryStatus::Completed => PlanStepStatus::Completed,
                        },
                    })
                    .collect(),
            }],
            AcpSessionUpdate::SessionInfoUpdate {
                title: Some(title), ..
            } => vec![WorkerEvent::SessionTitleUpdated {
                session_id: session_id.to_string(),
                title,
            }],
            AcpSessionUpdate::AvailableCommandsUpdate {
                available_commands, ..
            } => vec![WorkerEvent::AcpAvailableCommandsUpdated {
                commands: available_commands,
            }],
            AcpSessionUpdate::CurrentModeUpdate {
                current_mode_id, ..
            } => vec![WorkerEvent::AcpCurrentModeUpdated { current_mode_id }],
            AcpSessionUpdate::ConfigOptionUpdate { config_options, .. } => {
                vec![WorkerEvent::AcpConfigOptionsUpdated { config_options }]
            }
            AcpSessionUpdate::UsageUpdate {
                used, size, cost, ..
            } => vec![WorkerEvent::AcpUsageUpdated { used, size, cost }],
            AcpSessionUpdate::ToolCall {
                tool_call_id,
                title,
                kind,
                status,
                raw_input,
                raw_output,
                content,
                ..
            } => worker_events_from_acp_tool_call(
                AcpToolCallEventData {
                    tool_call_id,
                    title: Some(title),
                    status: Some(status),
                    raw_input,
                    raw_output,
                    content,
                },
                kind,
                terminal_state,
            ),
            AcpSessionUpdate::ToolCallUpdate {
                tool_call_id,
                title,
                kind,
                status,
                raw_input,
                raw_output,
                content,
                ..
            } => worker_events_from_acp_tool_call_update(
                AcpToolCallEventData {
                    tool_call_id,
                    title,
                    status,
                    raw_input,
                    raw_output,
                    content,
                },
                kind,
                terminal_state,
            ),
            AcpSessionUpdate::UserMessageChunk { .. }
            | AcpSessionUpdate::SessionInfoUpdate { title: None, .. } => Vec::new(),
        }
    }
}

pub(super) fn acp_terminal_output_event(
    params: &serde_json::Value,
    visible_terminal_ids: &HashSet<String>,
    pending_terminal_output: &mut HashMap<String, String>,
) -> Option<WorkerEvent> {
    let terminal_id = params.get("terminalId")?.as_str()?.to_string();
    let delta = params.get("delta")?.as_str()?.to_string();
    if delta.is_empty() {
        return None;
    }
    if !visible_terminal_ids.contains(&terminal_id) {
        pending_terminal_output
            .entry(terminal_id)
            .or_default()
            .push_str(&delta);
        return None;
    }
    Some(WorkerEvent::ToolOutputDelta {
        tool_use_id: terminal_id,
        delta,
    })
}

pub(super) fn worker_events_from_acp_notification(
    params: &serde_json::Value,
    active_session_id: Option<SessionId>,
) -> Vec<WorkerEvent> {
    let mut visible_terminal_ids = HashSet::new();
    let mut pending_terminal_output = HashMap::new();
    worker_events_from_acp_notification_with_terminal_state(
        params,
        active_session_id,
        &mut visible_terminal_ids,
        &mut pending_terminal_output,
    )
}

pub(super) fn worker_events_from_acp_notification_with_terminal_state(
    params: &serde_json::Value,
    active_session_id: Option<SessionId>,
    visible_terminal_ids: &mut HashSet<String>,
    pending_terminal_output: &mut HashMap<String, String>,
) -> Vec<WorkerEvent> {
    let Ok(notification) = serde_json::from_value::<AcpSessionNotification>(params.clone()) else {
        return Vec::new();
    };
    if Some(notification.session_id) != active_session_id {
        return Vec::new();
    }
    Vec::from(AcpSessionUpdateRender {
        session_id: notification.session_id,
        update: notification.update,
        terminal_state: AcpTerminalRenderState {
            visible_terminal_ids,
            pending_terminal_output,
        },
    })
}

fn message_item_id(message_id: Option<&str>) -> Option<ItemId> {
    message_id.and_then(|message_id| ItemId::try_from(message_id).ok())
}

fn worker_events_from_acp_tool_call(
    tool_call: AcpToolCallEventData,
    kind: AcpToolKind,
    terminal_state: AcpTerminalRenderState<'_>,
) -> Vec<WorkerEvent> {
    let title = tool_call
        .title
        .clone()
        .expect("ACP tool_call notifications always include a title");
    let status = tool_call
        .status
        .expect("ACP tool_call notifications always include a status");
    let tool_call_id = tool_call.tool_call_id.clone();
    let mut events = vec![WorkerEvent::ToolCall {
        tool_use_id: tool_call_id.clone(),
        summary: title.clone(),
        preparing: status == AcpToolCallStatus::Pending,
        parsed_commands: None,
    }];
    if let Some(input) = tool_call.raw_input.clone() {
        events.push(WorkerEvent::ToolCallDetails {
            tool_use_id: tool_call_id.clone(),
            tool_name: acp_tool_kind_label(kind).to_string(),
            input,
        });
    }
    events.extend(worker_events_from_acp_tool_content(
        AcpToolCallEventData {
            tool_call_id,
            title: Some(title),
            status: Some(status),
            raw_input: tool_call.raw_input,
            raw_output: tool_call.raw_output,
            content: tool_call.content,
        },
        terminal_state,
    ));
    events
}

fn worker_events_from_acp_tool_call_update(
    tool_call: AcpToolCallEventData,
    kind: Option<AcpToolKind>,
    terminal_state: AcpTerminalRenderState<'_>,
) -> Vec<WorkerEvent> {
    let mut events = Vec::new();
    if let Some(input) = tool_call.raw_input.clone() {
        events.push(WorkerEvent::ToolCallDetails {
            tool_use_id: tool_call.tool_call_id.clone(),
            tool_name: kind.map(acp_tool_kind_label).unwrap_or("tool").to_string(),
            input,
        });
    }
    if let Some(summary) = tool_call
        .title
        .clone()
        .or_else(|| tool_call.status.map(acp_tool_status_text))
    {
        events.push(WorkerEvent::ToolCallUpdated {
            tool_use_id: tool_call.tool_call_id.clone(),
            summary,
            parsed_commands: Vec::new(),
        });
    }
    events.extend(worker_events_from_acp_tool_content(
        tool_call,
        terminal_state,
    ));
    events
}

fn worker_events_from_acp_tool_content(
    tool_call: AcpToolCallEventData,
    terminal_state: AcpTerminalRenderState<'_>,
) -> Vec<WorkerEvent> {
    let mut events = Vec::new();
    let mut changes = HashMap::new();
    let mut text_parts = Vec::new();
    for item in tool_call.content {
        match item {
            AcpToolCallContent::Content { content } => {
                if let Some(text) = acp_content_display_text(&content) {
                    text_parts.push(text);
                }
            }
            AcpToolCallContent::Diff {
                path,
                old_text,
                new_text,
            } => {
                changes.insert(path, file_change_from_acp_diff(old_text, new_text));
            }
            AcpToolCallContent::Terminal { terminal_id } => {
                if terminal_state
                    .visible_terminal_ids
                    .insert(terminal_id.clone())
                {
                    events.push(WorkerEvent::ToolCall {
                        tool_use_id: terminal_id.clone(),
                        summary: format!("Terminal {terminal_id}"),
                        preparing: false,
                        parsed_commands: None,
                    });
                    if let Some(delta) = terminal_state.pending_terminal_output.remove(&terminal_id)
                        && !delta.is_empty()
                    {
                        events.push(WorkerEvent::ToolOutputDelta {
                            tool_use_id: terminal_id,
                            delta,
                        });
                    }
                }
            }
        }
    }
    if !changes.is_empty() {
        if let Some(input) = tool_call.raw_input.clone() {
            events.push(WorkerEvent::PatchAppliedIo {
                tool_name: tool_call
                    .title
                    .clone()
                    .unwrap_or_else(|| "tool".to_string()),
                input,
                changes,
            });
        } else {
            events.push(WorkerEvent::PatchApplied { changes });
        }
    }
    let text = text_parts.join("\n");
    if !text.is_empty() {
        if matches!(
            tool_call.status,
            Some(AcpToolCallStatus::Completed | AcpToolCallStatus::Failed)
        ) {
            events.push(acp_tool_result_event(
                tool_call.tool_call_id,
                tool_call.title,
                tool_call.raw_input,
                tool_call.raw_output,
                text,
                tool_call.status == Some(AcpToolCallStatus::Failed),
            ));
        } else {
            events.push(WorkerEvent::ToolOutputDelta {
                tool_use_id: tool_call.tool_call_id,
                delta: text,
            });
        }
    }
    events
}

fn acp_tool_result_event(
    tool_call_id: String,
    title: Option<String>,
    raw_input: Option<serde_json::Value>,
    raw_output: Option<serde_json::Value>,
    preview: String,
    is_error: bool,
) -> WorkerEvent {
    let title = title.unwrap_or_else(|| {
        if is_error {
            "Tool failed".to_string()
        } else {
            "Tool completed".to_string()
        }
    });
    match (raw_input, raw_output) {
        (Some(input), Some(output)) => WorkerEvent::ToolResultIo {
            tool_use_id: tool_call_id,
            tool_name: title.clone(),
            title,
            input,
            output,
            display_content: Some(preview),
            is_error,
            truncated: false,
        },
        _ => WorkerEvent::ToolResult {
            tool_use_id: tool_call_id,
            title,
            preview,
            is_error,
            truncated: false,
        },
    }
}

fn file_change_from_acp_diff(
    old_text: Option<String>,
    new_text: String,
) -> devo_protocol::protocol::FileChange {
    match old_text {
        None => devo_protocol::protocol::FileChange::Add { content: new_text },
        Some(old_text) if new_text.is_empty() => {
            devo_protocol::protocol::FileChange::Delete { content: old_text }
        }
        Some(old_text) => devo_protocol::protocol::FileChange::Update {
            unified_diff: diffy::create_patch(&old_text, &new_text).to_string(),
            old_text: Some(old_text),
            new_text: Some(new_text),
            move_path: None,
        },
    }
}

fn acp_tool_kind_label(kind: AcpToolKind) -> &'static str {
    match kind {
        AcpToolKind::Read => "read",
        AcpToolKind::Edit => "edit",
        AcpToolKind::Delete => "delete",
        AcpToolKind::Move => "move",
        AcpToolKind::Search => "search",
        AcpToolKind::Execute => "execute",
        AcpToolKind::Think => "think",
        AcpToolKind::Fetch => "fetch",
        AcpToolKind::Other => "tool",
    }
}

fn acp_tool_status_text(status: AcpToolCallStatus) -> String {
    match status {
        AcpToolCallStatus::Pending => "Pending",
        AcpToolCallStatus::InProgress => "Running",
        AcpToolCallStatus::Completed => "Completed",
        AcpToolCallStatus::Failed => "Failed",
        AcpToolCallStatus::Cancelled => "Cancelled",
    }
    .to_string()
}

fn acp_content_display_text(content: &AcpContentBlock) -> Option<String> {
    let text = match content {
        AcpContentBlock::Text { text, .. } => text.clone(),
        AcpContentBlock::Image { mime_type, uri, .. } => uri
            .as_ref()
            .map_or_else(|| format!("[image: {mime_type}]"), ToString::to_string),
        AcpContentBlock::Audio { mime_type, .. } => format!("[audio: {mime_type}]"),
        AcpContentBlock::ResourceLink {
            uri, title, name, ..
        } => {
            let label = title
                .as_deref()
                .filter(|title| !title.is_empty())
                .unwrap_or(name);
            format!("{label}: {uri}")
        }
        AcpContentBlock::Resource { resource, .. } => match resource {
            AcpEmbeddedResource::Text(resource) => resource.text.clone(),
            AcpEmbeddedResource::Blob(resource) => {
                let mime_type = resource.mime_type.as_deref().unwrap_or("unknown");
                format!("[resource: {} ({mime_type})]", resource.uri)
            }
        },
    };
    (!text.is_empty()).then_some(text)
}
