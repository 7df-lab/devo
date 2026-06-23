//! Inline live-list rendering for active direct sub-agents.

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

use crate::line_truncation::truncate_line_with_ellipsis_if_overflow;

pub(super) const MAX_VISIBLE_SUBAGENTS: usize = 3;

pub(super) struct SubagentLiveListRow {
    pub(super) session_id: SessionId,
    pub(super) name: String,
    pub(super) status: String,
    pub(super) preview: String,
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
    let mut lines = Vec::new();
    for row in &rows[visible_start..visible_end] {
        let is_selected = focused && selected == Some(row.session_id);
        lines.push(title_line(row, is_selected, accent));
        lines.push(preview_line(row));
    }

    let lines = lines
        .into_iter()
        .take(usize::from(area.height))
        .map(|line| truncate_line_with_ellipsis_if_overflow(line, usize::from(area.width)))
        .collect::<Vec<_>>();
    Paragraph::new(lines).render(area, buf);
}

fn visible_window_start(rows: &[SubagentLiveListRow], selected: Option<SessionId>) -> usize {
    if rows.len() <= MAX_VISIBLE_SUBAGENTS {
        return 0;
    }

    let selected_index = selected
        .and_then(|selected| rows.iter().position(|row| row.session_id == selected))
        .unwrap_or(0);
    selected_index
        .saturating_add(1)
        .saturating_sub(MAX_VISIBLE_SUBAGENTS)
        .min(rows.len().saturating_sub(MAX_VISIBLE_SUBAGENTS))
}

fn title_line(row: &SubagentLiveListRow, selected: bool, accent: Color) -> Line<'static> {
    let selection_marker = if selected {
        Span::styled("›", Style::default().fg(accent).bold())
    } else {
        Span::raw(" ")
    };
    let name_style = if selected {
        Style::default().fg(Color::White).bold()
    } else {
        Style::default().bold()
    };

    Line::from(vec![
        Span::raw("  "),
        selection_marker,
        Span::raw(" "),
        Span::styled("●", status_marker_style(&row.status)),
        Span::raw(" "),
        Span::styled(row.name.clone(), name_style),
        Span::raw(": "),
        Span::styled(row.status.clone(), status_text_style(&row.status)),
    ])
}

fn preview_line(row: &SubagentLiveListRow) -> Line<'static> {
    Line::from(vec![
        Span::raw("      ").dim(),
        Span::raw("> ").dim(),
        Span::styled(
            row.preview.clone(),
            Style::default().fg(Color::Rgb(176, 184, 196)),
        ),
    ])
}

fn status_marker_style(status: &str) -> Style {
    match status.to_ascii_lowercase().as_str() {
        "idle" => Style::default().fg(Color::Rgb(120, 220, 160)).bold(),
        "waiting_client" => Style::default().fg(Color::Rgb(210, 150, 60)).bold(),
        _ => Style::default().fg(Color::Rgb(106, 200, 255)).bold(),
    }
}

fn status_text_style(status: &str) -> Style {
    match status.to_ascii_lowercase().as_str() {
        "running" | "active_turn" => Style::default().fg(Color::Rgb(106, 200, 255)).bold(),
        "idle" => Style::default().fg(Color::Rgb(120, 220, 160)).bold(),
        "waiting_client" => Style::default().fg(Color::Rgb(210, 150, 60)).bold(),
        _ => Style::default().fg(Color::Rgb(160, 163, 168)),
    }
}
