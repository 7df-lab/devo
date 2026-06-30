//! Styled Ctrl+X selector for live direct sub-agents.

use devo_core::SessionId;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::BorderType;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::Widget;
use super::session_header::COMPLETED_COLOR;
use super::session_header::FAILED_COLOR;
use super::session_header::MUTED_COLOR;
use super::session_header::REASONING_ACCENT_COLOR;
use super::session_header::RUNNING_COLOR;

pub(super) struct SubagentSelectorAgent<'a> {
    pub(super) session_id: SessionId,
    pub(super) nickname: &'a str,
    pub(super) role: &'a str,
    pub(super) status: String,
    pub(super) task: Option<&'a str>,
    pub(super) agent_path: &'a str,
}

pub(super) fn render(
    area: Rect,
    buf: &mut Buffer,
    agents: &[SubagentSelectorAgent<'_>],
    selected: Option<SessionId>,
    accent: Color,
) {
    Clear.render(area, buf);
    let panel = selector_area(area, agents.len());
    Clear.render(panel, buf);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(86, 96, 112)))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled("Sub-agents", Style::default().fg(accent).bold()),
            Span::raw(" "),
        ]))
        .title_bottom(Line::from(vec![
            Span::raw(" "),
            Span::styled("↑/↓", Style::default().fg(accent)),
            Span::raw(" select  "),
            Span::styled("Enter", Style::default().fg(accent)),
            Span::raw(" open  "),
            Span::styled("q", Style::default().fg(accent)),
            Span::raw(" close "),
        ]));
    let inner = block.inner(panel);
    block.render(panel, buf);
    render_content(inner, buf, agents, selected, accent);
}

fn render_content(
    area: Rect,
    buf: &mut Buffer,
    agents: &[SubagentSelectorAgent<'_>],
    selected: Option<SessionId>,
    accent: Color,
) {
    if area.is_empty() {
        return;
    }

    let mut y = area.y;
    let header = if agents.len() == 1 {
        "1 active child".to_string()
    } else {
        format!("{} active children", agents.len())
    };
    render_line(
        area,
        buf,
        y,
        &Line::from(vec![
            Span::styled(header, Style::default().fg(accent).bold()),
            Span::raw("  "),
            Span::styled("live direct sub-agents", Style::default().dim()),
        ]),
    );
    y = y.saturating_add(2);

    if agents.is_empty() {
        render_line(area, buf, y, &Line::from("No active sub-agents.").dim());
        return;
    }

    for agent in agents {
        if y >= area.bottom() {
            break;
        }
        let is_selected = selected == Some(agent.session_id);
        let row_height = row_height(area, y);
        if row_height == 0 {
            break;
        }
        if is_selected {
            buf.set_style(
                Rect::new(area.x, y, area.width, row_height),
                Style::default().bg(Color::Rgb(28, 38, 52)),
            );
        }
        render_row(area, buf, y, agent, is_selected, accent);
        y = y.saturating_add(row_height).saturating_add(1);
    }
}

fn render_row(
    area: Rect,
    buf: &mut Buffer,
    y: u16,
    agent: &SubagentSelectorAgent<'_>,
    selected: bool,
    accent: Color,
) {
    let marker = if selected { "›" } else { " " };
    let marker_style = if selected {
        Style::default().fg(accent).bold()
    } else {
        Style::default().dim()
    };
    let title_style = if selected {
        Style::default().fg(Color::White).bold()
    } else {
        Style::default().bold()
    };
    render_line(
        area,
        buf,
        y,
        &Line::from(vec![
            Span::styled(marker, marker_style),
            Span::raw(" "),
            Span::styled(agent.nickname.to_string(), title_style),
            Span::raw("  "),
            status_badge(&agent.status),
            Span::raw("  "),
            Span::styled(agent.role.to_string(), Style::default().dim()),
        ]),
    );

    if y.saturating_add(1) < area.bottom() {
        let task = agent
            .task
            .filter(|task| !task.trim().is_empty())
            .unwrap_or("No task message");
        render_line(
            area,
            buf,
            y + 1,
            &Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    task.to_string(),
                    Style::default().fg(Color::Rgb(176, 184, 196)),
                ),
            ]),
        );
    }

    if y.saturating_add(2) < area.bottom() {
        render_line(
            area,
            buf,
            y + 2,
            &Line::from(vec![
                Span::raw("  "),
                Span::styled(agent.agent_path.to_string(), Style::default().dim()),
            ]),
        );
    }
}

fn render_line(area: Rect, buf: &mut Buffer, y: u16, line: &Line<'_>) {
    if y >= area.bottom() || area.width <= 2 {
        return;
    }
    buf.set_line(area.x + 1, y, line, area.width.saturating_sub(2));
}

fn selector_area(area: Rect, live_count: usize) -> Rect {
    if area.width <= 4 || area.height <= 4 {
        return area;
    }

    let horizontal_margin = if area.width > 104 {
        area.width.saturating_sub(96) / 2
    } else if area.width > 8 {
        2
    } else {
        0
    };
    let width = area
        .width
        .saturating_sub(horizontal_margin.saturating_mul(2));

    let max_height = if area.height > 6 {
        area.height - 2
    } else {
        area.height
    };
    let min_height = max_height.min(5);
    let content_height = live_count.saturating_mul(4).saturating_add(5);
    let desired_height = u16::try_from(content_height).unwrap_or(u16::MAX).min(26);
    let height = desired_height.min(max_height).max(min_height);

    Rect::new(
        area.x + horizontal_margin,
        area.y + area.height.saturating_sub(height) / 3,
        width.max(1),
        height.max(1),
    )
}

fn row_height(area: Rect, y: u16) -> u16 {
    area.bottom().saturating_sub(y).min(3)
}

fn status_badge(status: &str) -> Span<'static> {
    Span::styled(format!(" {status} "), status_style(status))
}

fn status_style(status: &str) -> Style {
    match status.to_ascii_lowercase().as_str() {
        "running" | "active_turn" => Style::default().fg(RUNNING_COLOR).bold(),
        "idle" => Style::default().fg(COMPLETED_COLOR).bold(),
        "waiting_client" => Style::default().fg(REASONING_ACCENT_COLOR).bold(),
        "completed" => Style::default().fg(COMPLETED_COLOR),
        "failed" | "interrupted" => Style::default().fg(FAILED_COLOR).bold(),
        "closed" => Style::default().dim(),
        _ => Style::default().fg(MUTED_COLOR),
    }
}
