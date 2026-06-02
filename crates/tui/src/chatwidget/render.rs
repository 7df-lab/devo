//! Layout rendering glue for `ChatWidget`.
//!
//! This module owns only the ratatui `Renderable` implementation so the root
//! chat widget file can focus on state construction and module wiring.

use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::render::renderable::Renderable;

use super::ChatWidget;

impl Renderable for ChatWidget {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if self.render_resume_browser_if_open(area, buf) {
            return;
        }

        if let Some(onboarding) = &self.onboarding {
            onboarding.render(area, buf);
            return;
        }

        let bottom_height = self
            .bottom_pane
            .desired_height(area.width)
            .min(area.height.saturating_sub(1).max(3));
        let [history_area, bottom_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(bottom_height)]).areas(area);

        let viewport_lines = self.active_viewport_lines(history_area.width);
        if !viewport_lines.is_empty() {
            Paragraph::new(Text::from(viewport_lines)).render(history_area, buf);
        }

        self.bottom_pane.render(bottom_area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        if self.onboarding.is_some() {
            return u16::MAX;
        }
        if self.resume_browser.is_some() {
            return u16::MAX;
        }
        let history_height =
            u16::try_from(self.active_viewport_lines(width.max(1)).len()).unwrap_or(u16::MAX);
        history_height
            .saturating_add(self.bottom_pane.desired_height(width))
            .saturating_add(2)
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if let Some(onboarding) = &self.onboarding {
            return onboarding.cursor_pos(area);
        }
        if self.resume_browser.is_some() {
            return None;
        }
        let bottom_height = self
            .bottom_pane
            .desired_height(area.width)
            .min(area.height.saturating_sub(1).max(3));
        let [_, bottom_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(bottom_height)]).areas(area);
        self.bottom_pane.cursor_pos(bottom_area)
    }
}
