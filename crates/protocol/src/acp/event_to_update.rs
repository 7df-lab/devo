use std::path::PathBuf;

use chrono::Utc;

use super::ACP_SESSION_UPDATE_METHOD;
use super::AcpMeta;
use super::DEVO_ACTIVITY_AT_META;
use super::DEVO_ITEM_ID_META;
use super::DEVO_ORIGINAL_EVENT_META;
use super::DEVO_ORIGINAL_METHOD_META;
use super::DEVO_SESSION_META;
use super::DEVO_TURN_ID_META;
use super::DEVO_TURN_USAGE_META;
use super::content::*;
use super::session_update::*;
use crate::native::event::ServerNotification;
use crate::native::item::FileChangeEntry;
use crate::native::item::FileChangeKind;
use crate::native::item::ItemEnvelope;
use crate::native::item::PlanEntry;
use crate::native::item::PlanStepStatus;
use crate::native::notification_bus::notification_legacy_session_id;
use crate::native::wire_projector::wire_from_server_notification;

/// Project a Native bus [`ServerNotification`] into an ACP `session/update`.
///
/// Item lifecycle reuses the envelope ACP projectors. Other Native-covered
/// lifecycle notifications embed the Native wire params under
/// `_meta.devo/originalEvent` so ACP adapters keep a recoverable method name.
pub fn acp_notification_from_server_notification(
    notification: &ServerNotification,
) -> (String, serde_json::Value) {
    let (method, params) = wire_from_server_notification(notification);
    let Some(session_id) = notification_legacy_session_id(notification) else {
        return (method, params);
    };
    let (update, meta) = match notification {
        ServerNotification::ItemStarted { item } => match acp_update_from_item_started(item) {
            Some(update) => {
                let meta = should_preserve_original_tool_envelope(item, /*completed*/ false)
                    .then(|| original_notification_meta(&method, notification));
                (update, meta)
            }
            None => (
                AcpSessionUpdate::SessionInfoUpdate {
                    title: None,
                    updated_at: None,
                    meta: None,
                },
                Some(original_notification_meta(&method, notification)),
            ),
        },
        ServerNotification::ItemCompleted { item } => match acp_update_from_item_completed(item) {
            Some(update) => {
                let meta = should_preserve_original_tool_envelope(item, /*completed*/ true)
                    .then(|| original_notification_meta(&method, notification));
                (update, meta)
            }
            None => (
                AcpSessionUpdate::SessionInfoUpdate {
                    title: None,
                    updated_at: None,
                    meta: None,
                },
                Some(original_notification_meta(&method, notification)),
            ),
        },
        ServerNotification::ItemUpdated { item } => match acp_update_from_item_updated(item) {
            Some(update) => (update, None),
            None => (
                AcpSessionUpdate::SessionInfoUpdate {
                    title: None,
                    updated_at: None,
                    meta: None,
                },
                Some(original_notification_meta(&method, notification)),
            ),
        },
        ServerNotification::SessionCreated { session }
        | ServerNotification::SessionMetadataUpdated { session } => {
            let mut meta = AcpMeta::new();
            meta.insert(
                DEVO_SESSION_META.to_string(),
                serde_json::to_value(session.as_ref()).expect("serialize native session"),
            );
            (
                AcpSessionUpdate::SessionInfoUpdate {
                    title: session.title.clone(),
                    updated_at: Some(session.last_activity_at.to_rfc3339()),
                    meta: Some(meta),
                },
                None,
            )
        }
        ServerNotification::ItemAssistantMessageDelta(delta) => (
            AcpSessionUpdate::AgentMessageChunk {
                content: AcpContentBlock::text(delta.delta.clone()),
                message_id: Some(delta.item_id.as_str().to_string()),
                meta: Some(acp_delta_meta(delta)),
            },
            None,
        ),
        ServerNotification::ItemReasoningDelta(delta) => (
            AcpSessionUpdate::AgentThoughtChunk {
                content: AcpContentBlock::text(delta.delta.clone()),
                message_id: Some(delta.item_id.as_str().to_string()),
                meta: Some(acp_delta_meta(delta)),
            },
            None,
        ),
        ServerNotification::TurnUsageUpdated {
            usage,
            session_totals,
            context_window,
            ..
        } => {
            let used = session_totals
                .as_ref()
                .map(|totals| totals.input_tokens + totals.output_tokens)
                .unwrap_or(usage.query.input_tokens + usage.query.output_tokens);
            let mut meta = AcpMeta::new();
            meta.insert(DEVO_TURN_USAGE_META.to_string(), params.clone());
            (
                AcpSessionUpdate::UsageUpdate {
                    used,
                    size: context_window.unwrap_or_else(|| used.max(1)),
                    cost: None,
                    meta: Some(meta),
                },
                None,
            )
        }
        ServerNotification::ToolCallStatusUpdated {
            turn_id,
            tool_call_id,
            status,
            ..
        } => (
            AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: tool_call_id.clone(),
                title: None,
                kind: None,
                status: acp_tool_call_status_from_str(status.as_str()),
                raw_input: None,
                raw_output: None,
                content: None,
                locations: None,
                meta: Some(acp_activity_meta_from_turn_id_str(turn_id.as_str())),
            },
            None,
        ),
        _ => (
            AcpSessionUpdate::SessionInfoUpdate {
                title: None,
                updated_at: None,
                meta: None,
            },
            Some(original_notification_meta(&method, notification)),
        ),
    };
    (
        ACP_SESSION_UPDATE_METHOD.to_string(),
        serde_json::to_value(AcpSessionNotification {
            session_id,
            update,
            meta,
        })
        .expect("serialize ACP session update"),
    )
}

fn original_notification_meta(method: &str, notification: &ServerNotification) -> AcpMeta {
    let mut meta = AcpMeta::new();
    meta.insert(
        DEVO_ORIGINAL_METHOD_META.to_string(),
        serde_json::Value::String(method.to_string()),
    );
    // Store Native wire params (not the tagged ServerNotification) so ACP
    // unwrap helpers can rebuild a method+params envelope without a dual bus.
    let (_, params) = wire_from_server_notification(notification);
    meta.insert(DEVO_ORIGINAL_EVENT_META.to_string(), params);
    meta
}

fn should_preserve_original_tool_envelope(envelope: &ItemEnvelope, completed: bool) -> bool {
    if completed {
        matches!(
            &envelope.item,
            crate::native::item::Item::ToolCall { .. }
                | crate::native::item::Item::ToolResult { .. }
                | crate::native::item::Item::CommandExecution { .. }
                | crate::native::item::Item::FileChange { .. }
        )
    } else {
        matches!(
            &envelope.item,
            crate::native::item::Item::ToolCall { .. }
                | crate::native::item::Item::CommandExecution { .. }
        )
    }
}

/// Unwrap Native-bus originals embedded by [`acp_notification_from_server_notification`].
pub fn original_notification_wire_from_acp(
    notification: &AcpSessionNotification,
) -> Option<(String, serde_json::Value)> {
    let meta = notification.meta.as_ref()?;
    let method = meta.get(DEVO_ORIGINAL_METHOD_META)?.as_str()?.to_string();
    let params = meta.get(DEVO_ORIGINAL_EVENT_META)?.clone();
    Some((method, params))
}

fn add_activity_at(meta: &mut AcpMeta) {
    meta.insert(
        DEVO_ACTIVITY_AT_META.to_string(),
        serde_json::Value::String(Utc::now().to_rfc3339()),
    );
}

fn acp_activity_meta_from_envelope(envelope: &ItemEnvelope) -> AcpMeta {
    let mut meta = AcpMeta::new();
    meta.insert(
        DEVO_TURN_ID_META.to_string(),
        serde_json::Value::String(envelope.turn_id.as_str().to_string()),
    );
    meta.insert(
        DEVO_ITEM_ID_META.to_string(),
        serde_json::Value::String(envelope.id.as_str().to_string()),
    );
    add_activity_at(&mut meta);
    meta
}

fn acp_activity_meta_from_turn_id_str(turn_id: &str) -> AcpMeta {
    let mut meta = AcpMeta::new();
    meta.insert(
        DEVO_TURN_ID_META.to_string(),
        serde_json::Value::String(turn_id.to_string()),
    );
    add_activity_at(&mut meta);
    meta
}

fn acp_delta_meta(delta: &crate::native::event::ItemDelta) -> AcpMeta {
    let mut meta = AcpMeta::new();
    meta.insert(
        DEVO_ITEM_ID_META.to_string(),
        serde_json::Value::String(delta.item_id.as_str().to_string()),
    );
    add_activity_at(&mut meta);
    meta
}

fn acp_update_from_item_updated(envelope: &ItemEnvelope) -> Option<AcpSessionUpdate> {
    match &envelope.item {
        crate::native::item::Item::Plan { entries, .. } => Some(AcpSessionUpdate::Plan {
            entries: entries.iter().map(acp_plan_entry_from_native).collect(),
            meta: None,
        }),
        _ => None,
    }
}

pub(crate) fn acp_update_from_item_started(envelope: &ItemEnvelope) -> Option<AcpSessionUpdate> {
    let meta = Some(acp_activity_meta_from_envelope(envelope));
    acp_update_from_native_item_started(&envelope.item, meta)
}

fn acp_update_from_native_item_started(
    native: &crate::native::item::Item,
    meta: Option<AcpMeta>,
) -> Option<AcpSessionUpdate> {
    use crate::native::item::Item;
    match native {
        Item::ToolCall {
            call_id,
            tool_name,
            input,
            ..
        } => {
            let parameters = input.clone().unwrap_or(serde_json::Value::Null);
            Some(AcpSessionUpdate::ToolCall {
                tool_call_id: call_id.clone(),
                title: tool_title(tool_name.as_str(), &parameters),
                kind: Some(tool_kind_from_name(tool_name.as_str())),
                status: Some(AcpToolCallStatus::Pending),
                locations: tool_locations_from_value(&parameters),
                raw_input: Some(parameters),
                raw_output: None,
                content: Vec::new(),
                meta,
            })
        }
        Item::CommandExecution {
            call_id,
            command,
            input,
            ..
        } => Some(AcpSessionUpdate::ToolCall {
            tool_call_id: call_id.clone(),
            title: command.clone(),
            kind: Some(AcpToolKind::Execute),
            status: Some(AcpToolCallStatus::Pending),
            locations: input
                .as_ref()
                .map(tool_locations_from_value)
                .unwrap_or_default(),
            raw_input: input.clone(),
            raw_output: None,
            content: Vec::new(),
            meta,
        }),
        _ => None,
    }
}

pub(crate) fn acp_update_from_item_completed(envelope: &ItemEnvelope) -> Option<AcpSessionUpdate> {
    let meta = Some(acp_activity_meta_from_envelope(envelope));
    acp_update_from_native_item_completed(envelope, &envelope.item, meta)
}

fn acp_update_from_native_item_completed(
    envelope: &ItemEnvelope,
    native: &crate::native::item::Item,
    meta: Option<AcpMeta>,
) -> Option<AcpSessionUpdate> {
    use crate::native::item::{Item, UserInput};
    match native {
        Item::UserMessage { content, .. } => {
            let text = content
                .iter()
                .filter_map(|part| match part {
                    UserInput::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            Some(AcpSessionUpdate::UserMessageChunk {
                content: AcpContentBlock::text(text),
                message_id: Some(envelope.id.as_str().to_string()),
                meta,
            })
        }
        Item::ToolResult {
            call_id,
            output,
            display_content,
            is_error,
            ..
        } => Some(AcpSessionUpdate::ToolCallUpdate {
            tool_call_id: call_id.clone(),
            title: Some("Tool result".to_string()),
            kind: None,
            status: Some(if *is_error {
                AcpToolCallStatus::Failed
            } else {
                AcpToolCallStatus::Completed
            }),
            raw_input: None,
            raw_output: Some(output.clone()),
            locations: Some(Vec::new()),
            content: Some(tool_result_content(display_content.clone(), output.clone())),
            meta,
        }),
        Item::CommandExecution {
            call_id,
            command,
            input,
            output,
            is_error,
            ..
        } => {
            let content = output
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .map(|text| vec![AcpToolCallContent::content(AcpContentBlock::text(text))]);
            Some(AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: call_id.clone(),
                title: Some(command.clone()),
                kind: Some(AcpToolKind::Execute),
                status: Some(if *is_error {
                    AcpToolCallStatus::Failed
                } else {
                    AcpToolCallStatus::Completed
                }),
                raw_input: input.clone(),
                raw_output: output.clone(),
                content,
                locations: None,
                meta,
            })
        }
        Item::FileChange {
            call_id, changes, ..
        } => Some(AcpSessionUpdate::ToolCallUpdate {
            tool_call_id: call_id.clone(),
            title: None,
            kind: Some(AcpToolKind::Edit),
            status: Some(AcpToolCallStatus::Completed),
            raw_input: None,
            raw_output: Some(serde_json::to_value(changes).unwrap_or(serde_json::Value::Null)),
            content: Some(native_file_change_tool_content(changes)),
            locations: Some(native_file_change_locations(changes)),
            meta,
        }),
        _ => None,
    }
}

fn native_file_change_tool_content(changes: &[FileChangeEntry]) -> Vec<AcpToolCallContent> {
    changes
        .iter()
        .map(|entry| match &entry.change {
            FileChangeKind::Add { content } if entry.path.is_absolute() => {
                AcpToolCallContent::Diff {
                    path: entry.path.clone(),
                    old_text: None,
                    new_text: content.clone(),
                    meta: None,
                }
            }
            FileChangeKind::Delete { content } if entry.path.is_absolute() => {
                AcpToolCallContent::Diff {
                    path: entry.path.clone(),
                    old_text: Some(content.clone()),
                    new_text: String::new(),
                    meta: None,
                }
            }
            FileChangeKind::Update { unified_diff, .. } if entry.path.is_absolute() => {
                AcpToolCallContent::content(AcpContentBlock::text(unified_diff.clone()))
            }
            FileChangeKind::Add { content } | FileChangeKind::Delete { content } => {
                AcpToolCallContent::content(AcpContentBlock::text(content.clone()))
            }
            FileChangeKind::Update { unified_diff, .. } => {
                AcpToolCallContent::content(AcpContentBlock::text(unified_diff.clone()))
            }
        })
        .collect()
}

fn native_file_change_locations(changes: &[FileChangeEntry]) -> Vec<AcpToolCallLocation> {
    changes
        .iter()
        .filter_map(|entry| {
            entry.path.is_absolute().then_some(AcpToolCallLocation {
                path: entry.path.clone(),
                line: None,
                meta: None,
            })
        })
        .collect()
}

fn acp_plan_entry_from_native(entry: &PlanEntry) -> AcpPlanEntry {
    AcpPlanEntry {
        content: entry.step.clone(),
        priority: AcpPlanEntryPriority::Medium,
        status: match entry.status {
            PlanStepStatus::Completed => AcpPlanEntryStatus::Completed,
            PlanStepStatus::InProgress => AcpPlanEntryStatus::InProgress,
            PlanStepStatus::Pending => AcpPlanEntryStatus::Pending,
        },
        meta: None,
    }
}

fn tool_title(tool_name: &str, parameters: &serde_json::Value) -> String {
    if let Some(command) = parameters
        .get("command")
        .or_else(|| parameters.get("cmd"))
        .and_then(serde_json::Value::as_str)
    {
        return command.to_string();
    }
    if matches!(
        tool_name,
        "webfetch" | "web_fetch" | "web-fetch" | "fetch_url" | "fetch-url"
    ) && let Some(url) = parameters
        .get("url")
        .and_then(serde_json::Value::as_str)
        .filter(|url| !url.is_empty())
    {
        return format!("web_fetch: {url}");
    }
    if matches!(tool_name, "web_search" | "websearch" | "web-search")
        && let Some(query) = parameters
            .get("query")
            .and_then(serde_json::Value::as_str)
            .filter(|query| !query.is_empty())
    {
        return format!("web_search: {query}");
    }
    if tool_name == "spawn_agent"
        && let Some(message) = parameters
            .get("message")
            .and_then(serde_json::Value::as_str)
            .filter(|message| !message.is_empty())
    {
        return format!("spawn_agent: {message}");
    }
    tool_name.to_string()
}

fn tool_kind_from_name(tool_name: &str) -> AcpToolKind {
    match tool_name {
        "read" | "grep" | "glob" | "lsp" => AcpToolKind::Read,
        "apply_patch" | "edit" | "write" => AcpToolKind::Edit,
        "bash" | "shell_command" | "exec_command" => AcpToolKind::Execute,
        "web_search" | "websearch" | "web_fetch" | "webfetch" | "web-fetch" | "fetch_url"
        | "fetch-url" | "websearch_query" => AcpToolKind::Fetch,
        "agent" => AcpToolKind::Think,
        _ => AcpToolKind::Other,
    }
}

fn acp_tool_call_status_from_str(status: &str) -> Option<AcpToolCallStatus> {
    Some(match status {
        "pending" => AcpToolCallStatus::Pending,
        "in_progress" => AcpToolCallStatus::InProgress,
        "completed" => AcpToolCallStatus::Completed,
        "failed" => AcpToolCallStatus::Failed,
        // ACP v1 has no cancelled tool-call status. Cancellation is represented
        // by the surrounding session/turn lifecycle, so do not emit an invalid
        // enum value to ACP clients.
        "cancelled" => return None,
        _ => return None,
    })
}

fn tool_locations_from_value(value: &serde_json::Value) -> Vec<AcpToolCallLocation> {
    let mut locations = Vec::new();
    for key in ["path", "filePath", "file_path"] {
        if let Some(path) = value.get(key).and_then(serde_json::Value::as_str) {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                locations.push(AcpToolCallLocation {
                    path,
                    line: value
                        .get("line")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|line| u32::try_from(line).ok()),
                    meta: None,
                });
            }
        }
    }
    for key in ["paths", "files"] {
        if let Some(items) = value.get(key).and_then(serde_json::Value::as_array) {
            for item in items {
                if let Some(path) = item.as_str() {
                    let path = PathBuf::from(path);
                    if path.is_absolute() {
                        locations.push(AcpToolCallLocation {
                            path,
                            line: None,
                            meta: None,
                        });
                    }
                } else {
                    push_location_from_object(item, &mut locations);
                }
            }
        }
    }
    locations
}

fn push_location_from_object(value: &serde_json::Value, locations: &mut Vec<AcpToolCallLocation>) {
    let Some(object) = value.as_object() else {
        return;
    };
    let path = object
        .get("path")
        .or_else(|| object.get("filePath"))
        .or_else(|| object.get("file_path"))
        .and_then(serde_json::Value::as_str);
    if let Some(path) = path {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            locations.push(AcpToolCallLocation {
                path,
                line: object
                    .get("line")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|line| u32::try_from(line).ok()),
                meta: None,
            });
        }
    }
}

pub(crate) fn tool_result_content(
    display_content: Option<String>,
    content: serde_json::Value,
) -> Vec<AcpToolCallContent> {
    if let Some(display_content) = display_content {
        return vec![AcpToolCallContent::content(AcpContentBlock::text(
            display_content,
        ))];
    }

    if let Some(content) = acp_tool_content_from_value(&content) {
        return content;
    }

    let text = match content {
        serde_json::Value::String(text) => text,
        other => other.to_string(),
    };
    vec![AcpToolCallContent::content(AcpContentBlock::text(text))]
}

fn acp_tool_content_from_value(value: &serde_json::Value) -> Option<Vec<AcpToolCallContent>> {
    if let Ok(content) = serde_json::from_value::<AcpToolCallContent>(value.clone()) {
        return Some(vec![content]);
    }

    if let Ok(contents) = serde_json::from_value::<Vec<AcpToolCallContent>>(value.clone()) {
        return Some(contents);
    }

    if let Ok(content) = serde_json::from_value::<AcpContentBlock>(value.clone()) {
        return Some(vec![AcpToolCallContent::content(content)]);
    }

    if let Ok(contents) = serde_json::from_value::<Vec<AcpContentBlock>>(value.clone()) {
        return Some(
            contents
                .into_iter()
                .map(AcpToolCallContent::content)
                .collect(),
        );
    }

    let mcp_contents = value.get("content")?;
    if let Ok(contents) = serde_json::from_value::<Vec<AcpToolCallContent>>(mcp_contents.clone()) {
        return Some(contents);
    }
    let contents = serde_json::from_value::<Vec<AcpContentBlock>>(mcp_contents.clone()).ok()?;
    Some(
        contents
            .into_iter()
            .map(AcpToolCallContent::content)
            .collect(),
    )
}
