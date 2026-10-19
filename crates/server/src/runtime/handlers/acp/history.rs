use super::*;

use devo_protocol::SessionHistoryEntry;
use devo_protocol::native::item::{Item, PlanStepStatus, UserInput};

const DEVO_TURN_DURATION_MS_META: &str = "devo/turnDurationMs";

pub(super) fn acp_update_from_history_entry(
    index: usize,
    entry: &SessionHistoryEntry,
    parent_message_id: Option<&str>,
) -> Option<AcpSessionUpdate> {
    let meta = history_meta(index, parent_message_id);
    match entry {
        SessionHistoryEntry::Item { item } => {
            acp_update_from_native_history_item(index, item, meta)
        }
        SessionHistoryEntry::TurnSummary {
            title: _,
            body,
            duration_ms,
            ..
        } => {
            let mut meta = meta;
            if let Some(duration_secs) = *duration_ms {
                meta.insert(
                    DEVO_TURN_DURATION_MS_META.to_string(),
                    serde_json::json!(duration_secs.saturating_mul(1_000)),
                );
            }
            Some(AcpSessionUpdate::AgentThoughtChunk {
                content: AcpContentBlock::text(body.clone()),
                message_id: Some(format!("history-{index}")),
                meta: Some(meta),
            })
        }
        SessionHistoryEntry::Error { title, body } => {
            let text = if body.is_empty() {
                title.clone()
            } else {
                body.clone()
            };
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: format!("history-{index}"),
                title: Some(title.clone()),
                kind: None,
                status: Some(AcpToolCallStatus::Failed),
                raw_input: None,
                raw_output: None,
                content: Some(vec![AcpToolCallContent::content(AcpContentBlock::text(
                    text,
                ))]),
                locations: Some(Vec::new()),
                meta: Some(meta),
            })
        }
    }
}

fn acp_update_from_native_history_item(
    index: usize,
    item: &Item,
    meta: AcpMeta,
) -> Option<AcpSessionUpdate> {
    let message_id = Some(format!("history-{index}"));
    match item {
        Item::UserMessage { content, .. } => {
            let text = user_text_from_content(content);
            Some(AcpSessionUpdate::UserMessageChunk {
                content: AcpContentBlock::text(text),
                message_id,
                meta: Some(history_meta(index, None)),
            })
        }
        Item::AssistantMessage { text, .. } => Some(AcpSessionUpdate::AgentMessageChunk {
            content: AcpContentBlock::text(text.clone()),
            message_id,
            meta: Some(meta),
        }),
        Item::HostedToolCall { output, .. } => {
            let content = output
                .as_ref()
                .map(|value| match value {
                    serde_json::Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            Some(AcpSessionUpdate::AgentMessageChunk {
                content: AcpContentBlock::text(content),
                message_id,
                meta: Some(meta),
            })
        }
        Item::Reasoning { text, .. } => Some(AcpSessionUpdate::AgentThoughtChunk {
            content: AcpContentBlock::text(text.clone()),
            message_id,
            meta: Some(meta),
        }),
        Item::Plan { entries, .. } => Some(AcpSessionUpdate::Plan {
            entries: entries
                .iter()
                .map(|entry| AcpPlanEntry {
                    content: entry.step.clone(),
                    priority: AcpPlanEntryPriority::Medium,
                    status: match entry.status {
                        PlanStepStatus::Completed => AcpPlanEntryStatus::Completed,
                        PlanStepStatus::InProgress => AcpPlanEntryStatus::InProgress,
                        PlanStepStatus::Pending => AcpPlanEntryStatus::Pending,
                    },
                    meta: None,
                })
                .collect(),
            meta: Some(meta),
        }),
        Item::ToolCall {
            call_id,
            tool_name,
            input,
            ..
        } => {
            let parameters = input.clone().unwrap_or(serde_json::Value::Null);
            Some(AcpSessionUpdate::ToolCall {
                tool_call_id: if call_id.is_empty() {
                    format!("history-{index}")
                } else {
                    call_id.clone()
                },
                title: tool_name.clone(),
                kind: Some(AcpToolKind::Other),
                status: Some(AcpToolCallStatus::Completed),
                raw_input: Some(parameters),
                raw_output: None,
                content: Vec::new(),
                locations: Vec::new(),
                meta: Some(meta),
            })
        }
        Item::ToolResult {
            call_id,
            output,
            display_content,
            is_error,
            ..
        } => {
            let text = display_content.clone().unwrap_or_else(|| match output {
                serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            });
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: if call_id.is_empty() {
                    format!("history-{index}")
                } else {
                    call_id.clone()
                },
                title: Some("Tool result".to_string()),
                kind: None,
                status: Some(if *is_error {
                    AcpToolCallStatus::Failed
                } else {
                    AcpToolCallStatus::Completed
                }),
                raw_input: None,
                raw_output: Some(output.clone()),
                content: Some(vec![AcpToolCallContent::content(AcpContentBlock::text(
                    text,
                ))]),
                locations: Some(Vec::new()),
                meta: Some(meta),
            })
        }
        Item::CommandExecution {
            call_id,
            command,
            input,
            output,
            is_error,
            ..
        } => {
            let text = output
                .as_ref()
                .map(|value| match value {
                    serde_json::Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: if call_id.is_empty() {
                    format!("history-{index}")
                } else {
                    call_id.clone()
                },
                title: Some(command.clone()),
                kind: Some(AcpToolKind::Execute),
                status: Some(if *is_error {
                    AcpToolCallStatus::Failed
                } else {
                    AcpToolCallStatus::Completed
                }),
                raw_input: input.clone(),
                raw_output: output.clone(),
                content: Some(vec![AcpToolCallContent::content(AcpContentBlock::text(
                    text,
                ))]),
                locations: Some(Vec::new()),
                meta: Some(meta),
            })
        }
        Item::ContextCompaction { .. } => None,
        Item::Warning { code, message, .. } => {
            // Live terminal failures are Native Warning items; ACP keeps the
            // prior Error-row projection shape for adapter clients.
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: format!("history-{index}"),
                title: Some(code.clone()),
                kind: None,
                status: Some(AcpToolCallStatus::Failed),
                raw_input: None,
                raw_output: None,
                content: Some(vec![AcpToolCallContent::content(AcpContentBlock::text(
                    message.clone(),
                ))]),
                locations: Some(Vec::new()),
                meta: Some(meta),
            })
        }
        _ => None,
    }
}

fn user_text_from_content(content: &[UserInput]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            UserInput::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn history_meta(index: usize, parent_message_id: Option<&str>) -> AcpMeta {
    let mut meta = AcpMeta::new();
    meta.insert(
        DEVO_HISTORY_INDEX_META.to_string(),
        serde_json::json!(index),
    );
    if let Some(parent_message_id) = parent_message_id {
        meta.insert(
            DEVO_PARENT_MESSAGE_ID_META.to_string(),
            serde_json::Value::String(parent_message_id.to_string()),
        );
    }
    meta
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn history_user_updates_include_stable_order_metadata_without_parent() {
        let entry = SessionHistoryEntry::item(Item::UserMessage {
            client_user_message_id: None,
            content: vec![UserInput::Text {
                text: "hello".into(),
            }],
            entry: Default::default(),
        });

        let update = acp_update_from_history_entry(3, &entry, None).expect("history update");

        let AcpSessionUpdate::UserMessageChunk {
            message_id, meta, ..
        } = update
        else {
            panic!("expected user message chunk");
        };
        assert_eq!(message_id, Some("history-3".to_string()));
        assert_eq!(meta, Some(history_meta(3, None)));
    }

    #[test]
    fn history_tool_updates_include_stable_order_and_parent_metadata() {
        let entry = SessionHistoryEntry::item(Item::ToolCall {
            call_id: "read-real-a".into(),
            tool_name: "Read".into(),
            source: devo_protocol::native::item::ToolSource::Builtin,
            server_name: None,
            input: None,
        });

        let update =
            acp_update_from_history_entry(4, &entry, Some("history-0")).expect("history update");

        let AcpSessionUpdate::ToolCall {
            tool_call_id, meta, ..
        } = update
        else {
            panic!("expected tool call");
        };
        assert_eq!(tool_call_id, "read-real-a");
        assert_eq!(meta, Some(history_meta(4, Some("history-0"))));
    }

    #[test]
    fn history_turn_summary_includes_duration_metadata() {
        let entry = SessionHistoryEntry::TurnSummary {
            title: "gpt-5".into(),
            body: String::new(),
            duration_ms: Some(42),
            collaboration_mode: Default::default(),
        };

        let update =
            acp_update_from_history_entry(5, &entry, Some("history-0")).expect("history update");

        let AcpSessionUpdate::AgentThoughtChunk {
            message_id, meta, ..
        } = update
        else {
            panic!("expected agent thought chunk");
        };
        let mut expected = history_meta(5, Some("history-0"));
        expected.insert(
            DEVO_TURN_DURATION_MS_META.to_string(),
            serde_json::json!(42_000_u64),
        );
        assert_eq!(message_id, Some("history-5".to_string()));
        assert_eq!(meta, Some(expected));
    }
}
