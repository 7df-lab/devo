//! Transcript overlay and live viewport projections for `ChatWidget`.
//!
//! This module converts committed and active history cells into the line
//! snapshots consumed by the Ctrl+T overlay, scrollback drain, and live view.

use ratatui::text::Line;
use ratatui::text::Span;

use crate::history_cell;
use crate::history_cell::HistoryCell;
use crate::history_cell::ScrollbackLine;

use super::ChatWidget;
use super::UserMessage;

/// Snapshot of active-cell state that affects transcript overlay rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ActiveCellTranscriptKey {
    pub(crate) revision: u64,
    pub(crate) is_stream_continuation: bool,
    pub(crate) animation_tick: Option<u64>,
}

/// Snapshot of one committed transcript cell for the Ctrl+T overlay.
#[derive(Clone, Debug)]
pub(crate) struct TranscriptOverlayCell {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) is_stream_continuation: bool,
    pub(crate) user_message: Option<UserMessage>,
    pub(crate) is_selected_user: bool,
}

impl ChatWidget {
    pub(crate) fn active_cell_transcript_key(&self) -> Option<ActiveCellTranscriptKey> {
        let active_cell = self.active_cell.as_ref()?;
        Some(ActiveCellTranscriptKey {
            revision: self.active_cell_revision,
            is_stream_continuation: active_cell.is_stream_continuation(),
            animation_tick: active_cell.transcript_animation_tick(),
        })
    }

    pub(crate) fn active_cell_transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.active_cell
            .as_ref()
            .map(|cell| cell.transcript_lines(width))
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn active_cell_display_lines_for_test(&self, width: u16) -> Vec<Line<'static>> {
        self.active_cell
            .as_ref()
            .map(|cell| cell.display_lines(width))
            .unwrap_or_default()
    }

    pub(crate) fn transcript_overlay_cell_count(&self) -> usize {
        self.history.len()
    }

    pub(crate) fn transcript_overlay_cells(&self, width: u16) -> Vec<TranscriptOverlayCell> {
        let width = width.max(1);
        self.history
            .iter()
            .map(|cell| {
                let user_message = cell
                    .as_any()
                    .downcast_ref::<history_cell::UserHistoryCell>()
                    .map(|user| UserMessage {
                        text: user.message.clone(),
                        local_images: user
                            .local_image_paths
                            .iter()
                            .cloned()
                            .map(|path| crate::bottom_pane::LocalImageAttachment {
                                path,
                                placeholder: String::new(),
                            })
                            .collect(),
                        remote_image_urls: user.remote_image_urls.clone(),
                        text_elements: user.text_elements.clone(),
                        mention_bindings: Vec::new(),
                    });
                TranscriptOverlayCell {
                    lines: cell.transcript_lines(width),
                    is_stream_continuation: cell.is_stream_continuation(),
                    user_message,
                    is_selected_user: false,
                }
            })
            .collect()
    }

    pub(crate) fn transcript_overlay_live_tail_key(&self) -> Option<ActiveCellTranscriptKey> {
        if !self.transcript_overlay_has_live_tail() {
            return None;
        }

        let active_cell = self.active_cell.as_ref();
        Some(ActiveCellTranscriptKey {
            revision: self.active_cell_revision,
            is_stream_continuation: active_cell.is_some_and(|cell| cell.is_stream_continuation()),
            animation_tick: active_cell.and_then(|cell| cell.transcript_animation_tick()),
        })
    }

    pub(crate) fn transcript_overlay_live_tail_lines(
        &self,
        width: u16,
    ) -> Option<Vec<Line<'static>>> {
        self.transcript_overlay_has_live_tail()
            .then(|| self.live_transcript_lines(width.max(1)))
    }

    pub(crate) fn transcript_overlay_lines(&self, width: u16) -> Vec<Line<'static>> {
        let width = width.max(1);
        let mut lines = Vec::new();
        for cell in &self.history {
            Self::extend_lines_with_separator(&mut lines, cell.transcript_lines(width));
        }
        Self::extend_lines_with_separator(&mut lines, self.live_transcript_lines(width));
        Self::trim_trailing_blank_lines(&mut lines);
        lines
    }

    pub(crate) fn transcript_overlay_has_live_tail(&self) -> bool {
        self.active_cell.is_some()
            || !self.active_text_items.is_empty()
            || !self.active_tool_calls.is_empty()
            || !self.pending_tool_calls.is_empty()
    }

    pub(crate) fn active_viewport_lines_for_test(&self, width: u16) -> Vec<Line<'static>> {
        self.active_viewport_lines(width)
    }

    pub(super) fn active_viewport_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if let Some(cell) = &self.active_cell {
            Self::extend_lines_with_separator(&mut lines, cell.display_lines(width));
        }
        for item in &self.active_text_items {
            if let Some(cell) = &item.cell {
                Self::extend_lines_with_separator(&mut lines, cell.display_lines(width));
            }
        }
        // Pending tool calls are shown with a pending (cyan) dot until their results arrive.
        for pending in &self.pending_tool_calls {
            let pending_lines = if let Some(start_time) = pending.start_time {
                vec![Line::from(vec![
                    crate::exec_cell::spinner(Some(start_time), true),
                    " ".into(),
                    Span::styled(pending.title.clone(), Self::tool_text_style()),
                ])]
            } else {
                pending.lines.clone()
            };
            Self::extend_lines_with_separator(
                &mut lines,
                history_cell::AgentMessageCell::new_with_prefix(
                    pending_lines,
                    Self::pending_dot_prefix(),
                    "  ",
                    false,
                )
                .display_lines(width),
            );
        }
        Self::trim_trailing_blank_lines(&mut lines);
        lines
    }

    fn live_transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if let Some(cell) = &self.active_cell {
            Self::extend_lines_with_separator(&mut lines, cell.transcript_lines(width));
        }
        for item in &self.active_text_items {
            if let Some(cell) = &item.cell {
                Self::extend_lines_with_separator(&mut lines, cell.transcript_lines(width));
            }
        }
        let mut tool_calls = self.active_tool_calls.values().collect::<Vec<_>>();
        tool_calls.sort_by(|left, right| left.tool_use_id.cmp(&right.tool_use_id));
        for tool_call in tool_calls {
            Self::extend_lines_with_separator(
                &mut lines,
                history_cell::AgentMessageCell::new_with_prefix(
                    tool_call.lines.clone(),
                    Self::pending_dot_prefix(),
                    "  ",
                    false,
                )
                .transcript_lines(width),
            );
        }
        for pending in &self.pending_tool_calls {
            let pending_lines = if let Some(start_time) = pending.start_time {
                vec![Line::from(vec![
                    crate::exec_cell::spinner(Some(start_time), true),
                    " ".into(),
                    Span::styled(pending.title.clone(), Self::tool_text_style()),
                ])]
            } else {
                pending.lines.clone()
            };
            Self::extend_lines_with_separator(
                &mut lines,
                history_cell::AgentMessageCell::new_with_prefix(
                    pending_lines,
                    Self::pending_dot_prefix(),
                    "  ",
                    false,
                )
                .transcript_lines(width),
            );
        }
        Self::trim_trailing_blank_lines(&mut lines);
        lines
    }

    fn extend_lines_with_separator(target: &mut Vec<Line<'static>>, mut next: Vec<Line<'static>>) {
        if next.is_empty() {
            return;
        }

        let should_insert_separator = !target.is_empty()
            && target.last().is_some_and(|line| !Self::is_blank_line(line))
            && next.first().is_some_and(|line| !Self::is_blank_line(line));
        if should_insert_separator {
            target.push(Line::from(""));
        }
        target.append(&mut next);
    }

    pub(crate) fn drain_scrollback_lines(&mut self, width: u16) -> Vec<ScrollbackLine> {
        let width = width.max(1);
        let mut lines = Vec::new();
        for (index, cell) in self
            .history
            .iter()
            .skip(self.next_history_flush_index)
            .enumerate()
        {
            let cell_lines = cell.display_lines(width);
            let should_insert_separator = index > 0
                && !cell_lines.is_empty()
                && !lines.is_empty()
                && lines
                    .last()
                    .is_some_and(|line: &ScrollbackLine| !Self::is_blank_line(&line.line))
                && cell_lines
                    .first()
                    .is_some_and(|line| !Self::is_blank_line(line));
            if should_insert_separator {
                lines.push(ScrollbackLine::new(Line::from("")));
            }
            lines.extend(cell_lines.into_iter().map(ScrollbackLine::new));
        }
        self.next_history_flush_index = self.history.len();
        if !lines.is_empty() {
            lines.push(ScrollbackLine::new(Line::from("")));
        }
        lines
    }
}
