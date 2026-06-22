use super::*;

pub(super) fn acp_update_from_history_item(
    index: usize,
    item: &SessionHistoryItem,
) -> Option<AcpSessionUpdate> {
    if let Some(SessionHistoryMetadata::PlanUpdate { steps, .. }) = &item.metadata {
        return Some(AcpSessionUpdate::Plan {
            entries: steps
                .iter()
                .map(|step| AcpPlanEntry {
                    content: step.text.clone(),
                    priority: AcpPlanEntryPriority::Medium,
                    status: match step.status {
                        SessionPlanStepStatus::Completed => AcpPlanEntryStatus::Completed,
                        SessionPlanStepStatus::InProgress => AcpPlanEntryStatus::InProgress,
                        SessionPlanStepStatus::Pending | SessionPlanStepStatus::Cancelled => {
                            AcpPlanEntryStatus::Pending
                        }
                    },
                })
                .collect(),
            meta: None,
        });
    }
    let content = AcpContentBlock::text(item.body.clone());
    let message_id = Some(format!("history-{index}"));
    match item.kind {
        SessionHistoryItemKind::User => Some(AcpSessionUpdate::UserMessageChunk {
            content,
            message_id,
            meta: None,
        }),
        SessionHistoryItemKind::Assistant => Some(AcpSessionUpdate::AgentMessageChunk {
            content,
            message_id,
            meta: None,
        }),
        SessionHistoryItemKind::Reasoning | SessionHistoryItemKind::TurnSummary => {
            Some(AcpSessionUpdate::AgentThoughtChunk {
                content,
                message_id,
                meta: None,
            })
        }
        SessionHistoryItemKind::ToolCall => {
            let tool_call_id = history_tool_call_id(index, item);
            Some(AcpSessionUpdate::ToolCall {
                tool_call_id,
                title: item.title.clone(),
                kind: AcpToolKind::Other,
                status: AcpToolCallStatus::Completed,
                raw_input: item.tool_io.as_ref().map(|tool_io| tool_io.input.clone()),
                raw_output: item
                    .tool_io
                    .as_ref()
                    .and_then(|tool_io| tool_io.output.clone()),
                content: Vec::new(),
                locations: Vec::new(),
                meta: None,
            })
        }
        SessionHistoryItemKind::ToolResult
        | SessionHistoryItemKind::CommandExecution
        | SessionHistoryItemKind::Error => {
            let tool_call_id = history_tool_call_id(index, item);
            let text = if item.body.is_empty() {
                item.title.clone()
            } else {
                item.body.clone()
            };
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id,
                title: Some(item.title.clone()),
                kind: None,
                status: Some(if item.kind == SessionHistoryItemKind::Error {
                    AcpToolCallStatus::Failed
                } else {
                    AcpToolCallStatus::Completed
                }),
                raw_input: item.tool_io.as_ref().map(|tool_io| tool_io.input.clone()),
                raw_output: item
                    .tool_io
                    .as_ref()
                    .and_then(|tool_io| tool_io.output.clone()),
                content: vec![AcpToolCallContent::Content {
                    content: AcpContentBlock::text(text),
                }],
                locations: Vec::new(),
                meta: None,
            })
        }
    }
}

fn history_tool_call_id(index: usize, item: &SessionHistoryItem) -> String {
    item.tool_call_id
        .clone()
        .unwrap_or_else(|| format!("history-{index}"))
}
