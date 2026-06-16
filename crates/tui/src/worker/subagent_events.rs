//! Sub-agent event routing helpers for the TUI worker.
//!
//! The stdio worker receives server events for every runtime session. This
//! module classifies direct child-agent events and translates them into
//! read-only monitor updates so they do not mutate the active parent transcript.

use std::collections::HashMap;
use std::collections::HashSet;

use tokio::sync::mpsc;

use devo_core::SessionId;
use devo_protocol::AgentInfo;
use devo_protocol::SessionMetadata;
use devo_server::ItemEnvelope;
use devo_server::ItemEventPayload;
use devo_server::ItemKind;
use devo_server::ServerEvent;
use devo_server::SessionStatusChangedPayload;
use devo_server::ToolCallPayload;
use devo_server::ToolResultPayload;
use devo_server::TurnEventPayload;

use crate::events::PlanStep;
use crate::events::SubagentMonitorAgent;
use crate::events::SubagentMonitorEvent;
use crate::events::TextItemKind;
use crate::events::WorkerEvent;

pub(super) enum RoutedServerEvent {
    Parent,
    Child,
    Discovered(SubagentMonitorAgent),
    Ignore,
}

pub(super) fn route_server_event(
    active_session_id: Option<SessionId>,
    child_agent_sessions: &HashSet<SessionId>,
    event: &ServerEvent,
) -> RoutedServerEvent {
    if let Some(agent) = agent_from_started_event(active_session_id, event) {
        return RoutedServerEvent::Discovered(agent);
    }
    let Some(event_session_id) = event.session_id() else {
        return RoutedServerEvent::Parent;
    };
    if child_agent_sessions.contains(&event_session_id) {
        return RoutedServerEvent::Child;
    }
    if active_session_id == Some(event_session_id) {
        RoutedServerEvent::Parent
    } else {
        RoutedServerEvent::Ignore
    }
}

pub(super) fn agent_from_info(info: AgentInfo) -> Option<SubagentMonitorAgent> {
    Some(SubagentMonitorAgent {
        session_id: info.session_id,
        parent_session_id: info.parent_session_id?,
        agent_path: info.agent_path,
        nickname: info.agent_nickname,
        role: info.agent_role,
        status: info.status,
        last_task_message: info.last_task_message,
    })
}

pub(super) fn agent_from_session(session: &SessionMetadata) -> Option<SubagentMonitorAgent> {
    Some(SubagentMonitorAgent {
        session_id: session.session_id,
        parent_session_id: session.parent_session_id?,
        agent_path: session.agent_path.clone()?,
        nickname: session
            .agent_nickname
            .clone()
            .unwrap_or_else(|| session.session_id.to_string()),
        role: session
            .agent_role
            .clone()
            .unwrap_or_else(|| "default".to_string()),
        status: format!("{:?}", session.status).to_lowercase(),
        last_task_message: None,
    })
}

pub(super) fn emit_subagent_event(
    method: &str,
    event: ServerEvent,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
    latest_completed_agent_messages: &mut HashMap<SessionId, String>,
) {
    match method {
        "turn/started" => {
            if let ServerEvent::TurnStarted(payload) = event {
                let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                    event: SubagentMonitorEvent::TurnStarted {
                        session_id: payload.session_id,
                        turn_id: payload.turn.turn_id,
                    },
                });
            }
        }
        "item/started" => {
            if let ServerEvent::ItemStarted(payload) = event {
                emit_subagent_item_started(payload, event_tx);
            }
        }
        "item/agentMessage/delta" => {
            if let ServerEvent::ItemDelta { payload, .. } = event {
                let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                    event: SubagentMonitorEvent::TextItemDelta {
                        session_id: payload.context.session_id,
                        item_id: payload.context.item_id,
                        kind: TextItemKind::Assistant,
                        delta: payload.delta,
                    },
                });
            }
        }
        "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
            if let ServerEvent::ItemDelta { payload, .. } = event {
                let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                    event: SubagentMonitorEvent::TextItemDelta {
                        session_id: payload.context.session_id,
                        item_id: payload.context.item_id,
                        kind: TextItemKind::Reasoning,
                        delta: payload.delta,
                    },
                });
            }
        }
        "item/commandExecution/outputDelta" => {
            if let ServerEvent::ItemDelta { payload, .. } = event
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload.delta)
            {
                let tool_use_id = value
                    .get("tool_use_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let text = value
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if !tool_use_id.is_empty() {
                    let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                        event: SubagentMonitorEvent::ToolOutputDelta {
                            session_id: payload.context.session_id,
                            tool_use_id: tool_use_id.to_string(),
                            delta: text.to_string(),
                        },
                    });
                }
            }
        }
        "item/completed" => {
            if let ServerEvent::ItemCompleted(payload) = event {
                if let Some(text) = super::completed_agent_message_text(&payload) {
                    latest_completed_agent_messages.insert(payload.context.session_id, text);
                }
                emit_subagent_item_completed(payload, event_tx);
            }
        }
        "turn/completed" => {
            if let ServerEvent::TurnCompleted(payload) = event {
                let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                    event: SubagentMonitorEvent::TurnFinished {
                        session_id: payload.session_id,
                        status: format!("{:?}", payload.turn.status),
                    },
                });
            }
        }
        "turn/failed" => {
            if let ServerEvent::TurnFailed(TurnEventPayload {
                session_id, turn, ..
            }) = event
            {
                let message = latest_completed_agent_messages
                    .remove(&session_id)
                    .unwrap_or_else(|| format!("turn failed with status {:?}", turn.status));
                let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                    event: SubagentMonitorEvent::TurnFailed {
                        session_id,
                        message,
                    },
                });
            }
        }
        "turn/plan/updated" => {
            if let ServerEvent::TurnPlanUpdated(payload) = event {
                let steps = payload
                    .plan
                    .into_iter()
                    .filter_map(|step| {
                        Some(PlanStep {
                            text: step.step,
                            status: super::parse_plan_step_status(&step.status)?,
                        })
                    })
                    .collect::<Vec<_>>();
                let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                    event: SubagentMonitorEvent::PlanUpdated {
                        session_id: payload.session_id,
                        explanation: payload.explanation,
                        steps,
                    },
                });
            }
        }
        "session/status/changed" => {
            if let ServerEvent::SessionStatusChanged(SessionStatusChangedPayload {
                session_id,
                status,
            }) = event
            {
                let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                    event: SubagentMonitorEvent::SessionStatusChanged { session_id, status },
                });
            }
        }
        "session/title/updated"
        | "session/compaction/started"
        | "session/compaction/completed"
        | "session/compaction/failed"
        | "turn/usage/updated"
        | "inputQueue/updated"
        | "search/updated"
        | "search/completed"
        | "search/failed" => {}
        _ => {}
    }
}

fn agent_from_started_event(
    active_session_id: Option<SessionId>,
    event: &ServerEvent,
) -> Option<SubagentMonitorAgent> {
    let ServerEvent::SessionStarted(payload) = event else {
        return None;
    };
    let agent = agent_from_session(&payload.session)?;
    (Some(agent.parent_session_id) == active_session_id).then_some(agent)
}

fn emit_subagent_item_started(
    payload: ItemEventPayload,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) {
    let session_id = payload.context.session_id;
    match payload.item.item_kind {
        ItemKind::AgentMessage => {
            let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::TextItemStarted {
                    session_id: payload.context.session_id,
                    item_id: payload.item.item_id,
                    kind: TextItemKind::Assistant,
                },
            });
        }
        ItemKind::Reasoning => {
            let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::TextItemStarted {
                    session_id: payload.context.session_id,
                    item_id: payload.item.item_id,
                    kind: TextItemKind::Reasoning,
                },
            });
        }
        ItemKind::ToolCall => {
            if let Ok(payload) = serde_json::from_value::<ToolCallPayload>(payload.item.payload) {
                let summary = super::summarize_tool_call(&payload);
                let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                    event: SubagentMonitorEvent::ToolCall {
                        session_id,
                        tool_use_id: payload.tool_call_id,
                        summary,
                    },
                });
            }
        }
        ItemKind::CommandExecution => {
            if let Ok(payload) =
                serde_json::from_value::<devo_server::CommandExecutionPayload>(payload.item.payload)
            {
                let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                    event: SubagentMonitorEvent::ToolCall {
                        session_id,
                        tool_use_id: payload.tool_call_id,
                        summary: payload.command,
                    },
                });
            }
        }
        ItemKind::UserMessage
        | ItemKind::Plan
        | ItemKind::ToolResult
        | ItemKind::FileChange
        | ItemKind::McpToolCall
        | ItemKind::WebSearch
        | ItemKind::ImageView
        | ItemKind::ContextCompaction
        | ItemKind::ApprovalRequest
        | ItemKind::ApprovalDecision => {}
    }
}

fn emit_subagent_item_completed(
    payload: ItemEventPayload,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) {
    let session_id = payload.context.session_id;
    match payload.item {
        ItemEnvelope {
            item_id,
            item_kind: ItemKind::AgentMessage,
            payload,
            ..
        } => {
            if let Some(text) = text_payload(&payload) {
                let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                    event: SubagentMonitorEvent::TextItemCompleted {
                        session_id,
                        item_id: Some(item_id),
                        kind: TextItemKind::Assistant,
                        final_text: text,
                    },
                });
            }
        }
        ItemEnvelope {
            item_id,
            item_kind: ItemKind::Reasoning,
            payload,
            ..
        } => {
            if let Some(text) = text_payload(&payload) {
                let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                    event: SubagentMonitorEvent::TextItemCompleted {
                        session_id,
                        item_id: Some(item_id),
                        kind: TextItemKind::Reasoning,
                        final_text: text,
                    },
                });
            }
        }
        ItemEnvelope {
            item_kind: ItemKind::ToolCall,
            payload,
            ..
        } => {
            if let Ok(payload) = serde_json::from_value::<ToolCallPayload>(payload) {
                let summary = super::summarize_tool_call_update(&payload);
                let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                    event: SubagentMonitorEvent::ToolCallUpdated {
                        session_id,
                        tool_use_id: payload.tool_call_id,
                        summary,
                    },
                });
            }
        }
        ItemEnvelope {
            item_kind: ItemKind::ToolResult,
            payload,
            ..
        } => {
            if let Ok(payload) = serde_json::from_value::<ToolResultPayload>(payload) {
                let title = if payload.summary.is_empty() {
                    super::summarize_tool_result_title(
                        payload.tool_name.as_deref(),
                        payload.is_error,
                    )
                } else {
                    payload.summary
                };
                let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                    event: SubagentMonitorEvent::ToolResult {
                        session_id,
                        tool_use_id: payload.tool_call_id,
                        title,
                        preview: payload
                            .display_content
                            .unwrap_or_else(|| super::render_json_value_text(&payload.content)),
                        is_error: payload.is_error,
                    },
                });
            }
        }
        ItemEnvelope {
            item_kind: ItemKind::CommandExecution,
            payload,
            ..
        } => {
            if let Ok(payload) =
                serde_json::from_value::<devo_server::CommandExecutionPayload>(payload)
            {
                let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                    event: SubagentMonitorEvent::ToolResult {
                        session_id,
                        tool_use_id: payload.tool_call_id,
                        title: payload.command,
                        preview: payload
                            .output
                            .as_ref()
                            .map(super::render_json_value_text)
                            .unwrap_or_default(),
                        is_error: payload.is_error,
                    },
                });
            }
        }
        ItemEnvelope {
            item_kind:
                ItemKind::UserMessage
                | ItemKind::Plan
                | ItemKind::FileChange
                | ItemKind::McpToolCall
                | ItemKind::WebSearch
                | ItemKind::ImageView
                | ItemKind::ContextCompaction
                | ItemKind::ApprovalRequest
                | ItemKind::ApprovalDecision,
            ..
        } => {}
    }
}

fn text_payload(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}
