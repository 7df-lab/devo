//! Renders and formats unified-exec background session summary text.
//!
//! This module provides one canonical summary string so the bottom pane can
//! either render a dedicated footer row or reuse the same text inline in the
//! status row without duplicating copy/grammar logic.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::v2::live_wrap::take_prefix_by_width;
use crate::v2::render::renderable::Renderable;

/// Tracks active unified-exec processes and renders a compact summary.
pub(crate) struct UnifiedExecFooter {
    processes: Vec<String>,
}

impl UnifiedExecFooter {
    pub(crate) fn new() -> Self {
        Self {
            processes: Vec::new(),
        }
    }

    pub(crate) fn set_processes(&mut self, processes: Vec<String>) -> bool {
        if self.processes == processes {
            return false;
        }
        self.processes = processes;
        true
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.processes.is_empty()
    }

    /// Returns the unindented summary text used by both footer and status-row rendering.
    ///
    /// The returned string intentionally omits leading spaces and separators so
    /// callers can choose layout-specific framing (inline separator vs. row
    /// indentation). Returning `None` means there is nothing to surface.
    pub(crate) fn summary_text(&self) -> Option<String> {
        if self.processes.is_empty() {
            return None;
        }

        let count = self.processes.len();
        let plural = if count == 1 { "" } else { "s" };
        Some(format!(
            "{count} background terminal{plural} running · /ps to view · /stop to close"
        ))
    }

    fn render_lines(&self, width: u16) -> Vec<Line<'static>> {
        if width < 4 {
            return Vec::new();
        }
        let Some(summary) = self.summary_text() else {
            return Vec::new();
        };
        let message = format!("  {summary}");
        let (truncated, _, _) = take_prefix_by_width(&message, width as usize);
        vec![Line::from(truncated.dim())]
    }
}

impl Renderable for UnifiedExecFooter {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        Paragraph::new(self.render_lines(area.width)).render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.render_lines(width).len() as u16
    }
}
