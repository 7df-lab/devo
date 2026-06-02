//! Active assistant/reasoning text stream lifecycle for `ChatWidget`.
//!
//! This module owns the ordering, live-cell synchronization, and final commit
//! behavior for streaming text items while `ChatWidget` keeps the actual state.

use std::time::Instant;

use devo_core::ItemId;
use ratatui::text::Span;

use crate::events::TextItemKind;
use crate::history_cell;
use crate::markdown::append_markdown;
use crate::streaming::commit_tick::CommitTickScope;
use crate::streaming::commit_tick::run_commit_tick;
use crate::streaming::controller::StreamController;

use super::ChatWidget;
use super::DotStatus;

pub(super) struct ActiveTextItem {
    pub(super) item_id: ActiveTextItemId,
    pub(super) kind: TextItemKind,
    pub(super) status: DotStatus,
    pub(super) stream_controller: Option<StreamController>,
    raw_text: String,
    pub(super) cell: Option<history_cell::AgentMessageCell>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActiveTextItemId {
    Server(ItemId),
    Legacy(TextItemKind),
}

impl ActiveTextItemId {
    fn log_label(self) -> String {
        match self {
            Self::Server(item_id) => item_id.to_string(),
            Self::Legacy(kind) => format!("legacy-{kind:?}"),
        }
    }
}

impl ChatWidget {
    pub(super) fn commit_active_streams(&mut self, status: DotStatus) {
        tracing::debug!(
            status = ?status,
            active_items = ?self.active_text_item_log_order(),
            "committing all active text items"
        );
        while !self.active_text_items.is_empty() {
            self.commit_text_item_at(0, status);
        }
    }

    pub(super) fn start_text_item(&mut self, item_id: ActiveTextItemId, kind: TextItemKind) {
        if self
            .active_text_items
            .iter()
            .any(|item| item.item_id == item_id)
        {
            return;
        }

        let stream_controller = match kind {
            TextItemKind::Assistant => Some(StreamController::new(None, &self.session.cwd)),
            TextItemKind::Reasoning => None,
        };
        let insert_index = self.active_text_item_insert_index(kind);
        tracing::debug!(
            item_id = %item_id.log_label(),
            kind = ?kind,
            insert_index,
            before = ?self.active_text_item_log_order(),
            "starting active text item"
        );
        self.active_text_items.insert(
            insert_index,
            ActiveTextItem {
                item_id,
                kind,
                status: DotStatus::Pending,
                stream_controller,
                raw_text: String::new(),
                cell: None,
            },
        );
        tracing::trace!(
            after = ?self.active_text_item_log_order(),
            "active text item order after start"
        );
        self.stream_chunking_policy.reset();
    }

    pub(super) fn push_text_item_delta(
        &mut self,
        item_id: ActiveTextItemId,
        kind: TextItemKind,
        delta: &str,
    ) {
        let index = self.ensure_text_item(item_id, kind);
        tracing::debug!(
            item_id = %item_id.log_label(),
            kind = ?kind,
            delta_len = delta.len(),
            active_items = ?self.active_text_item_log_order(),
            "received active text item delta"
        );
        match kind {
            TextItemKind::Assistant => {
                if let Some(controller) = self.active_text_items[index].stream_controller.as_mut() {
                    controller.push(delta);
                }
            }
            TextItemKind::Reasoning => {
                self.active_text_items[index].raw_text.push_str(delta);
            }
        }
        self.sync_text_item_cell(index);
        self.frame_requester.schedule_frame();
    }

    pub(super) fn complete_text_item(
        &mut self,
        item_id: ActiveTextItemId,
        kind: TextItemKind,
        final_text: String,
    ) {
        let index = self.ensure_text_item(item_id, kind);
        tracing::debug!(
            item_id = %item_id.log_label(),
            kind = ?kind,
            final_text_len = final_text.len(),
            active_items = ?self.active_text_item_log_order(),
            "completed active text item"
        );
        self.active_text_items[index].status = DotStatus::Completed;
        if !final_text.trim().is_empty() {
            self.active_text_items[index].raw_text = final_text;
        }
        self.sync_text_item_cell(index);
        self.commit_completed_text_items();
        if matches!(item_id, ActiveTextItemId::Server(_)) && kind == TextItemKind::Assistant {
            self.committed_server_assistant_in_turn = true;
        }
    }

    fn ensure_text_item(&mut self, item_id: ActiveTextItemId, kind: TextItemKind) -> usize {
        if let Some(index) = self
            .active_text_items
            .iter()
            .position(|item| item.item_id == item_id)
        {
            return index;
        }

        self.start_text_item(item_id, kind);
        self.active_text_items
            .iter()
            .position(|item| item.item_id == item_id)
            .unwrap_or_else(|| self.active_text_items.len().saturating_sub(1))
    }

    pub(super) fn has_server_active_item(&self, kind: TextItemKind) -> bool {
        self.active_text_items
            .iter()
            .any(|item| matches!(item.item_id, ActiveTextItemId::Server(_)) && item.kind == kind)
    }

    fn commit_text_item_at(&mut self, index: usize, status: DotStatus) {
        if index >= self.active_text_items.len() {
            return;
        }

        let mut item = self.active_text_items.remove(index);
        tracing::debug!(
            item_id = %item.item_id.log_label(),
            kind = ?item.kind,
            status = ?status,
            remaining = ?self.active_text_item_log_order(),
            "committing active text item"
        );
        match item.kind {
            TextItemKind::Assistant => {
                if let Some(controller) = item.stream_controller.as_mut() {
                    let (_cell, source) = controller.finalize();
                    if let Some(source) = source {
                        self.add_assistant_markdown_source(source, status);
                    } else if !item.raw_text.trim().is_empty() {
                        self.add_markdown_history_with_status_without_redraw(
                            "Assistant",
                            &item.raw_text,
                            status,
                        );
                    }
                } else if !item.raw_text.trim().is_empty() {
                    self.add_markdown_history_with_status_without_redraw(
                        "Assistant",
                        &item.raw_text,
                        status,
                    );
                }
            }
            TextItemKind::Reasoning => {
                if !item.raw_text.trim().is_empty() {
                    self.add_markdown_history_with_status("Reasoning", &item.raw_text, status);
                }
            }
        }
        self.stream_chunking_policy.reset();
    }

    fn add_assistant_markdown_source(&mut self, source: String, status: DotStatus) {
        if source.trim().is_empty() {
            return;
        }

        self.add_history_entry_without_redraw(Box::new(history_cell::AgentMarkdownCell::new(
            source,
            &self.session.cwd,
            self.dot_prefix(status),
            "  ",
        )));
    }

    fn active_text_item_insert_index(&self, kind: TextItemKind) -> usize {
        match kind {
            TextItemKind::Reasoning => self
                .active_text_items
                .iter()
                .position(|item| item.kind == TextItemKind::Assistant)
                .unwrap_or(self.active_text_items.len()),
            TextItemKind::Assistant => self.active_text_items.len(),
        }
    }

    fn commit_completed_text_items(&mut self) {
        let mut index = 0;
        while index < self.active_text_items.len() {
            let item = &self.active_text_items[index];
            if item.status != DotStatus::Completed {
                index += 1;
                continue;
            }

            if item.kind == TextItemKind::Assistant
                && self.active_text_items[..index]
                    .iter()
                    .any(|prior| prior.kind == TextItemKind::Reasoning)
            {
                tracing::debug!(
                    item_id = %item.item_id.log_label(),
                    active_items = ?self.active_text_item_log_order(),
                    "deferring assistant commit until prior reasoning item commits"
                );
                index += 1;
                continue;
            }

            self.commit_text_item_at(index, DotStatus::Completed);
        }
    }

    fn active_text_item_log_order(&self) -> Vec<String> {
        self.active_text_items
            .iter()
            .map(|item| {
                format!(
                    "{:?}:{}:{:?}",
                    item.kind,
                    item.item_id.log_label(),
                    item.status
                )
            })
            .collect()
    }

    pub(super) fn run_stream_commit_tick(&mut self) {
        let now = Instant::now();
        let mut output_cells = Vec::new();
        let mut needs_followup = false;
        let mut changed_indexes = Vec::new();

        for (index, item) in self.active_text_items.iter_mut().enumerate() {
            let Some(controller) = item.stream_controller.as_mut() else {
                continue;
            };
            let output = run_commit_tick(
                &mut self.stream_chunking_policy,
                Some(controller),
                CommitTickScope::AnyMode,
                now,
            );
            if item.kind == TextItemKind::Assistant {
                if !output.cells.is_empty() {
                    changed_indexes.push(index);
                }
                if !output.all_idle {
                    needs_followup = true;
                }
                continue;
            }
            if !output.cells.is_empty() {
                output_cells.extend(output.cells);
                changed_indexes.push(index);
            }
            if !output.all_idle {
                needs_followup = true;
            }
        }

        for cell in output_cells {
            self.add_history_entry_without_redraw(cell);
        }
        for index in changed_indexes {
            self.sync_text_item_cell(index);
        }
        if needs_followup {
            self.frame_requester
                .schedule_frame_in(std::time::Duration::from_millis(16));
        }
        if !self.active_text_items.is_empty() {
            self.frame_requester.schedule_frame();
        }
    }

    fn sync_text_item_cell(&mut self, index: usize) {
        if index >= self.active_text_items.len() {
            return;
        }

        let cell = match self.active_text_items[index].kind {
            TextItemKind::Assistant => self.assistant_active_cell(&self.active_text_items[index]),
            TextItemKind::Reasoning => self.reasoning_active_cell(&self.active_text_items[index]),
        };
        self.active_text_items[index].cell = cell;
        self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
    }

    fn assistant_active_cell(
        &self,
        item: &ActiveTextItem,
    ) -> Option<history_cell::AgentMessageCell> {
        if let Some(controller) = &item.stream_controller {
            let lines = controller.live_lines();
            if lines.iter().any(|line| !Self::is_blank_line(line)) {
                return Some(history_cell::AgentMessageCell::new_ai_response_with_prefix(
                    lines,
                    Self::pending_dot_prefix(),
                    "  ",
                    false,
                ));
            }
        } else if !item.raw_text.trim().is_empty() {
            return Some(self.bulleted_markdown_cell(&item.raw_text, Self::pending_dot_prefix()));
        }
        None
    }

    fn reasoning_active_cell(
        &self,
        item: &ActiveTextItem,
    ) -> Option<history_cell::AgentMessageCell> {
        if item.raw_text.trim().is_empty() {
            return None;
        }

        let mut body_lines = Vec::new();
        append_markdown(
            &item.raw_text,
            None,
            Some(&self.session.cwd),
            &mut body_lines,
        );
        Self::patch_lines_style(&mut body_lines, Self::reasoning_text_style());
        if let Some(first_line) = body_lines.first_mut() {
            first_line.spans.insert(
                0,
                Span::styled("Thinking: ", Self::reasoning_heading_style()),
            );
        }
        Some(history_cell::AgentMessageCell::new_ai_response_with_prefix(
            body_lines,
            Self::reasoning_dot_prefix(item.status),
            "  ",
            false,
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::events::TextItemKind;

    use super::ActiveTextItemId;

    #[test]
    fn legacy_text_item_id_log_label_includes_kind() {
        assert_eq!(
            ActiveTextItemId::Legacy(TextItemKind::Assistant).log_label(),
            "legacy-Assistant"
        );
    }
}
