//! Inline live-list rendering for active direct sub-agents.

use devo_core::ItemId;
use devo_core::SessionId;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

use super::session_header::COMPLETED_COLOR;
use super::session_header::MUTED_COLOR;
use super::session_header::PREVIEW_COLOR;
use super::session_header::REASONING_ACCENT_COLOR;
use super::session_header::RUNNING_COLOR;
use crate::line_truncation::truncate_line_with_ellipsis_if_overflow;
use crate::ui_consts::LIVE_PREFIX_COLS;

pub(super) const MAX_VISIBLE_SUBAGENTS: usize = 3;

pub(super) struct SubagentLiveListRow {
    pub(super) key: SubagentLiveListRowKey,
    pub(super) name: String,
    pub(super) status: String,
    pub(super) preview: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SubagentLiveListRowKey {
    Session(SessionId),
    Research(ItemId),
}

impl SubagentLiveListRowKey {
    fn session_id(self) -> Option<SessionId> {
        match self {
            Self::Session(session_id) => Some(session_id),
            Self::Research(_) => None,
        }
    }
}

pub(super) fn desired_height(row_count: usize) -> u16 {
    u16::try_from(row_count.min(MAX_VISIBLE_SUBAGENTS).saturating_mul(2)).unwrap_or(u16::MAX)
}

pub(super) fn render(
    area: Rect,
    buf: &mut Buffer,
    rows: &[SubagentLiveListRow],
    selected: Option<SessionId>,
    focused: bool,
    accent: Color,
) {
    if area.is_empty() || rows.is_empty() {
        return;
    }

    let visible_start = visible_window_start(rows, selected);
    let visible_end = rows.len().min(visible_start + MAX_VISIBLE_SUBAGENTS);
    let visible_rows = rows[visible_start..visible_end]
        .iter()
        .map(|row| format!("{}:{}:{}", row.name, row.status, row.preview))
        .collect::<Vec<_>>()
        .join(" | ");
    tracing::debug!(
        target: "devo_tui::subagent",
        row_count = rows.len(),
        visible_start,
        visible_end,
        ?selected,
        focused,
        visible_rows,
        "rendering subagent live list"
    );
    let mut lines = Vec::new();
    for row in &rows[visible_start..visible_end] {
        let is_selected = focused && selected == row.key.session_id();
        lines.push(truncate_line_with_ellipsis_if_overflow(
            title_line(row, is_selected, accent),
            usize::from(area.width),
        ));
        lines.push(preview_line(row, usize::from(area.width)));
    }

    let lines = lines
        .into_iter()
        .take(usize::from(area.height))
        .collect::<Vec<_>>();
    Paragraph::new(lines).render(area, buf);
}

fn visible_window_start(rows: &[SubagentLiveListRow], selected: Option<SessionId>) -> usize {
    if rows.len() <= MAX_VISIBLE_SUBAGENTS {
        return 0;
    }

    let selected_index = selected
        .and_then(|selected| {
            rows.iter()
                .position(|row| row.key.session_id() == Some(selected))
        })
        .unwrap_or(0);
    selected_index
        .saturating_add(1)
        .saturating_sub(MAX_VISIBLE_SUBAGENTS)
        .min(rows.len().saturating_sub(MAX_VISIBLE_SUBAGENTS))
}

fn title_line(row: &SubagentLiveListRow, selected: bool, accent: Color) -> Line<'static> {
    let selection_marker = if selected {
        Span::styled("› ", Style::default().fg(accent).bold())
    } else {
        Span::raw("")
    };
    let name_style = if selected {
        Style::default().fg(Color::White).bold()
    } else {
        Style::default().bold()
    };

    Line::from(vec![
        Span::raw(" ".repeat(usize::from(LIVE_PREFIX_COLS))),
        selection_marker,
        Span::styled("●", status_marker_style(&row.status)),
        Span::raw(" "),
        Span::styled(row.name.clone(), name_style),
        Span::raw(": "),
        Span::styled(row.status.clone(), status_text_style(&row.status)),
    ])
}

fn preview_line(row: &SubagentLiveListRow, max_width: usize) -> Line<'static> {
    let prefix = format!("{}> ", " ".repeat(usize::from(LIVE_PREFIX_COLS)));
    let preview_width = max_width.saturating_sub(UnicodeWidthStr::width(prefix.as_str()));
    Line::from(vec![
        Span::raw(prefix).dim(),
        Span::styled(
            truncate_preview_tail(&row.preview, preview_width),
            Style::default().fg(PREVIEW_COLOR),
        ),
    ])
}

fn truncate_preview_tail(preview: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(preview) <= max_width {
        return preview.to_string();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    format!("…{}", take_suffix_by_width(preview, max_width - 1))
}

fn take_suffix_by_width(text: &str, max_width: usize) -> String {
    let mut used = 0usize;
    let mut suffix = Vec::new();
    for ch in text.chars().rev() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used.saturating_add(width) > max_width {
            break;
        }
        used = used.saturating_add(width);
        suffix.push(ch);
    }
    suffix.into_iter().rev().collect()
}

fn status_marker_style(status: &str) -> Style {
    match status.to_ascii_lowercase().as_str() {
        "idle" => Style::default().fg(COMPLETED_COLOR).bold(),
        "waiting_client" => Style::default().fg(REASONING_ACCENT_COLOR).bold(),
        _ => Style::default().fg(RUNNING_COLOR).bold(),
    }
}

fn status_text_style(status: &str) -> Style {
    match status.to_ascii_lowercase().as_str() {
        "running" | "active_turn" => Style::default().fg(RUNNING_COLOR).bold(),
        "idle" => Style::default().fg(COMPLETED_COLOR).bold(),
        "waiting_client" => Style::default().fg(REASONING_ACCENT_COLOR).bold(),
        _ => Style::default().fg(MUTED_COLOR),
    }
}
