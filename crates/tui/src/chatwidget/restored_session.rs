//! Restored-session transcript reconstruction for `ChatWidget`.
//!
//! Session resume can provide rich protocol history or older transcript items;
//! this module rebuilds the visible history cells for those restored sessions.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::events::TranscriptItem;
use crate::exec_cell::ExecCell;
use crate::exec_cell::new_active_exec_command;
use crate::history_cell;
use crate::tool_result_cell::ToolResultCell;
use devo_protocol::SessionHistoryItem;
use devo_protocol::SessionHistoryMetadata;
use devo_protocol::SessionPlanStepStatus;
use ratatui::text::Line;

use super::ChatWidget;
use super::DotStatus;

impl ChatWidget {
    pub(super) fn rebuild_restored_session_history(
        &mut self,
        history_items: Vec<TranscriptItem>,
        loaded_item_count: u64,
        session_id: &str,
        title: Option<&str>,
    ) {
        self.history.clear();
        self.next_history_flush_index = 0;

        tracing::trace!(
            session_id,
            loaded_item_count,
            restored_items = history_items.len(),
            restored_preview = ?history_items
                .iter()
                .take(10)
                .map(|item| (format!("{:?}", item.kind), item.title.clone()))
                .collect::<Vec<_>>(),
            synthetic_header_inserted = true,
            "rebuilding restored session transcript"
        );

        let loaded_any_history = !history_items.is_empty();
        for item in &history_items {
            self.add_transcript_item_without_redraw(item.clone());
        }

        if !loaded_any_history {
            self.add_history_entry_without_redraw(Box::new(history_cell::new_info_event(
                format!(
                    "switched to {session_id}; title: {}; loaded items: {loaded_item_count}",
                    title.unwrap_or("(untitled)")
                ),
                None,
            )));
        }
        self.frame_requester.schedule_frame();
    }

    pub(super) fn rebuild_restored_session_history_from_rich_items(
        &mut self,
        history_items: &[SessionHistoryItem],
        loaded_item_count: u64,
        session_id: &str,
        title: Option<&str>,
    ) -> bool {
        self.history.clear();
        self.next_history_flush_index = 0;

        if history_items.is_empty() {
            self.add_history_entry_without_redraw(Box::new(history_cell::new_info_event(
                format!(
                    "switched to {session_id}; title: {}; loaded items: {loaded_item_count}",
                    title.unwrap_or("(untitled)")
                ),
                None,
            )));
            self.frame_requester.schedule_frame();
            return false;
        }

        let mut paired_result_by_call_id = HashMap::new();
        for (index, item) in history_items.iter().enumerate() {
            if matches!(
                item.kind,
                devo_protocol::SessionHistoryItemKind::ToolResult
                    | devo_protocol::SessionHistoryItemKind::Error
            ) && let Some(tool_call_id) = item.tool_call_id.as_deref()
            {
                paired_result_by_call_id
                    .entry(tool_call_id.to_string())
                    .or_insert(index);
            }
        }

        let metadata_owned_ids: HashSet<String> = history_items
            .iter()
            .filter_map(|item| {
                item.tool_call_id
                    .clone()
                    .filter(|_| item.metadata.is_some())
            })
            .collect();
        let mut consumed_indexes = HashSet::new();

        for (index, item) in history_items.iter().enumerate() {
            if consumed_indexes.contains(&index) {
                continue;
            }

            if let Some(metadata) = &item.metadata {
                if let Some(tool_call_id) = item.tool_call_id.as_deref()
                    && let Some(result_index) = paired_result_by_call_id.get(tool_call_id).copied()
                {
                    consumed_indexes.insert(result_index);
                }
                match metadata {
                    SessionHistoryMetadata::PlanUpdate { explanation, steps } => {
                        self.on_plan_updated(
                            explanation.clone(),
                            steps
                                .iter()
                                .map(|step| crate::events::PlanStep {
                                    text: step.text.clone(),
                                    status: match step.status {
                                        SessionPlanStepStatus::Pending => {
                                            crate::events::PlanStepStatus::Pending
                                        }
                                        SessionPlanStepStatus::InProgress => {
                                            crate::events::PlanStepStatus::InProgress
                                        }
                                        SessionPlanStepStatus::Completed => {
                                            crate::events::PlanStepStatus::Completed
                                        }
                                        SessionPlanStepStatus::Cancelled => {
                                            crate::events::PlanStepStatus::Cancelled
                                        }
                                    },
                                })
                                .collect(),
                        );
                    }
                    SessionHistoryMetadata::Edited { changes } => {
                        self.add_history_entry_without_redraw(Box::new(
                            history_cell::new_patch_event(changes.clone(), &self.session.cwd),
                        ));
                    }
                    SessionHistoryMetadata::Explored { actions } => {
                        self.restore_explored_history_item(item, actions.clone());
                    }
                }
                continue;
            }

            if let Some(changes) = Self::edited_changes_from_history_item(item) {
                self.add_history_entry_without_redraw(Box::new(history_cell::new_patch_event(
                    changes,
                    &self.session.cwd,
                )));
                continue;
            }

            if item.kind == devo_protocol::SessionHistoryItemKind::ToolCall
                && let Some(tool_call_id) = item.tool_call_id.as_deref()
            {
                if metadata_owned_ids.contains(tool_call_id) {
                    continue;
                }
                if let Some(result_index) = paired_result_by_call_id.get(tool_call_id).copied() {
                    consumed_indexes.insert(result_index);
                    let result_item = &history_items[result_index];
                    let title_line =
                        (!item.title.is_empty()).then(|| Self::ran_tool_line(&item.title));
                    self.add_history_entry_without_redraw(Box::new(ToolResultCell::new(
                        title_line,
                        result_item.body.clone(),
                        Self::tool_dot_prefix(),
                        Line::from("  "),
                        Self::tool_text_style(),
                        false,
                    )));
                    continue;
                }
            }

            match item.kind {
                devo_protocol::SessionHistoryItemKind::User => {
                    self.add_history_entry_without_redraw(Box::new(history_cell::new_user_prompt(
                        item.body.clone(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        self.active_accent_color(),
                    )));
                }
                devo_protocol::SessionHistoryItemKind::Assistant => {
                    self.add_markdown_history_without_redraw("Assistant", &item.body);
                }
                devo_protocol::SessionHistoryItemKind::Reasoning => {
                    self.add_markdown_history_without_redraw("Reasoning", &item.body);
                }
                devo_protocol::SessionHistoryItemKind::ToolCall => {
                    self.add_history_entry_without_redraw(Box::new(
                        history_cell::AgentMessageCell::new_with_prefix(
                            vec![Self::running_tool_line(&item.title)],
                            self.dot_prefix(DotStatus::Pending),
                            "  ",
                            false,
                        ),
                    ));
                }
                devo_protocol::SessionHistoryItemKind::ToolResult
                | devo_protocol::SessionHistoryItemKind::CommandExecution => {
                    self.add_history_entry_without_redraw(Box::new(ToolResultCell::new(
                        (!item.title.is_empty()).then(|| Self::ran_tool_line(&item.title)),
                        item.body.clone(),
                        Self::tool_dot_prefix(),
                        Line::from("  "),
                        Self::tool_text_style(),
                        false,
                    )));
                }
                devo_protocol::SessionHistoryItemKind::Error => {
                    self.add_history_entry_without_redraw(Box::new(ToolResultCell::new(
                        (!item.title.is_empty()).then(|| Self::ran_tool_line(&item.title)),
                        item.body.clone(),
                        self.failed_dot_prefix(),
                        Line::from("  "),
                        Self::tool_text_style(),
                        false,
                    )));
                }
                devo_protocol::SessionHistoryItemKind::TurnSummary => {
                    self.add_history_entry_without_redraw(Box::new(
                        history_cell::TurnSummaryCell::new(
                            item.title.clone(),
                            item.duration_ms,
                            self.active_accent_color(),
                        ),
                    ));
                }
            }
        }

        self.frame_requester.schedule_frame();
        true
    }

    pub(super) fn edited_changes_from_history_item(
        item: &SessionHistoryItem,
    ) -> Option<HashMap<PathBuf, devo_protocol::protocol::FileChange>> {
        if item.kind != devo_protocol::SessionHistoryItemKind::ToolResult {
            return None;
        }
        let lower_title = item.title.to_ascii_lowercase();
        if !lower_title.contains("apply_patch")
            && !lower_title.contains("write")
            && !item.body.contains("\"files\"")
        {
            return None;
        }
        let value: serde_json::Value = serde_json::from_str(&item.body).ok()?;
        let files = value.get("files")?.as_array()?;
        let diff = value
            .get("diff")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut changes = HashMap::new();
        for file in files {
            let path = PathBuf::from(file.get("path")?.as_str()?);
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
                        .unwrap_or_else(|| "\n".repeat(additions as usize)),
                },
                "delete" => devo_protocol::protocol::FileChange::Delete {
                    content: file
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| "\n".repeat(deletions as usize)),
                },
                "update" | "move" => devo_protocol::protocol::FileChange::Update {
                    unified_diff: diff.clone(),
                    move_path: file
                        .get("movePath")
                        .or_else(|| file.get("move_path"))
                        .and_then(serde_json::Value::as_str)
                        .map(PathBuf::from),
                },
                _ => continue,
            };
            changes.insert(path, change);
        }
        (!changes.is_empty()).then_some(changes)
    }

    pub(super) fn restore_explored_history_item(
        &mut self,
        item: &SessionHistoryItem,
        actions: Vec<devo_protocol::parse_command::ParsedCommand>,
    ) {
        let command = item.title.clone();
        let command_tokens = crate::exec_command::split_command_string(&command);
        if let Some(cell) = self
            .history
            .last_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
            && let Some(grouped) = cell.with_added_call(
                item.tool_call_id
                    .clone()
                    .unwrap_or_else(|| "restored".to_string()),
                command_tokens.clone(),
                actions.clone(),
                devo_protocol::protocol::ExecCommandSource::Agent,
                None,
            )
        {
            *cell = grouped;
            return;
        }

        let exec = new_active_exec_command(
            item.tool_call_id
                .clone()
                .unwrap_or_else(|| "restored".to_string()),
            command_tokens,
            actions,
            devo_protocol::protocol::ExecCommandSource::Agent,
            None,
            false,
        );
        self.add_history_entry_without_redraw(Box::new(exec));
    }
}
