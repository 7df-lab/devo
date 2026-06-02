//! Session header, status summary, and shared line-formatting helpers for `ChatWidget`.
//!
//! The chat widget owns the session state, while this module keeps header refresh,
//! token summary text, and small shared rendering helpers out of the root file.

use std::time::Instant;

use devo_protocol::Model;
use devo_protocol::ProviderWireApi;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use crate::history_cell;
use crate::history_cell::HistoryCell;
use crate::startup_header::STARTUP_HEADER_ANIMATION_INTERVAL;

use super::ChatWidget;
use super::DotStatus;

impl ChatWidget {
    pub(super) fn is_blank_line(line: &Line<'_>) -> bool {
        line.spans.iter().all(|span| span.content.trim().is_empty())
    }

    pub(super) fn build_header_box(
        cwd: &std::path::Path,
        model: Option<&Model>,
        thinking_selection: Option<&str>,
        is_first_run: bool,
        startup_tooltip_override: Option<String>,
        accent_color: Color,
        mascot_frame_index: usize,
    ) -> Box<dyn HistoryCell> {
        let model = model.cloned().unwrap_or_else(|| Model {
            slug: "unknown".to_string(),
            display_name: "unknown".to_string(),
            provider: ProviderWireApi::OpenAIChatCompletions,
            ..Model::default()
        });
        Box::new(history_cell::new_session_info(
            cwd,
            &model.slug,
            model.slug.clone(),
            model.display_name.clone(),
            model.thinking_capability.clone(),
            model
                .resolve_thinking_selection(thinking_selection)
                .effective_reasoning_effort,
            model.thinking_implementation.clone(),
            is_first_run,
            startup_tooltip_override,
            /*show_fast_status*/ false,
            accent_color,
            mascot_frame_index,
        ))
    }

    pub(super) fn trim_trailing_blank_lines(lines: &mut Vec<Line<'static>>) {
        while lines
            .last()
            .is_some_and(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
        {
            lines.pop();
        }
    }

    pub(super) fn completed_dot_prefix() -> Line<'static> {
        Line::from(vec![
            Span::styled("▌", Style::default().fg(Color::Rgb(120, 220, 160))),
            " ".into(),
        ])
    }

    pub(super) fn pending_dot_prefix() -> Line<'static> {
        Line::from(vec![
            Span::styled("▌", Style::default().fg(Color::Rgb(110, 200, 255))),
            " ".into(),
        ])
    }

    pub(super) fn reasoning_dot_prefix(status: DotStatus) -> Line<'static> {
        let color = match status {
            DotStatus::Pending => Color::Rgb(210, 150, 60),
            DotStatus::Completed => Color::Rgb(120, 220, 160),
            DotStatus::Failed => Color::Rgb(255, 100, 100),
        };
        Line::from(vec![
            Span::styled("▌", Style::default().fg(color)),
            " ".into(),
        ])
    }

    pub(super) fn truncate_display_text(value: &str, max_width: usize) -> String {
        let total_width = unicode_width::UnicodeWidthStr::width(value);
        if total_width <= max_width {
            return value.to_string();
        }
        if max_width == 0 {
            return String::new();
        }
        if max_width <= 3 {
            return ".".repeat(max_width);
        }

        let target_width = max_width.saturating_sub(3);
        let mut rendered = String::new();
        let mut rendered_width = 0usize;
        for ch in value.chars() {
            let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if rendered_width.saturating_add(ch_width) > target_width {
                break;
            }
            rendered.push(ch);
            rendered_width = rendered_width.saturating_add(ch_width);
        }
        rendered.push_str("...");
        rendered
    }

    pub(super) fn pad_display_text(value: &str, target_width: usize) -> String {
        let width = unicode_width::UnicodeWidthStr::width(value);
        if width >= target_width {
            return value.to_string();
        }
        format!("{value}{}", " ".repeat(target_width - width))
    }

    pub(super) fn tool_text_style() -> Style {
        Style::default().fg(Color::Rgb(160, 163, 168))
    }

    pub(super) fn tool_status_running_style() -> Style {
        Style::default().fg(Color::Rgb(106, 200, 255)).bold()
    }

    pub(super) fn tool_status_done_style() -> Style {
        Style::default().fg(Color::Rgb(120, 220, 160)).bold()
    }

    pub(super) fn running_tool_line(title: &str) -> Line<'static> {
        let normalized = title
            .strip_prefix("Running ")
            .or_else(|| title.strip_prefix("Ran "))
            .unwrap_or(title);
        Line::from(vec![
            Span::styled("Running ", Self::tool_status_running_style()),
            Span::styled(normalized.to_string(), Self::tool_text_style()),
        ])
    }

    pub(super) fn ran_tool_line(title: &str) -> Line<'static> {
        let normalized = title
            .strip_prefix("Running ")
            .or_else(|| title.strip_prefix("Ran "))
            .unwrap_or(title);
        Line::from(vec![
            Span::styled("Ran ", Self::tool_status_done_style()),
            Span::styled(normalized.to_string(), Self::tool_text_style()),
        ])
    }

    pub(super) fn tool_dot_prefix() -> Line<'static> {
        Line::from(vec![
            Span::styled("▌", Style::default().fg(Color::Rgb(120, 220, 160))),
            " ".into(),
        ])
    }

    pub(super) fn failed_dot_prefix(&self) -> Line<'static> {
        let error_color = self.active_error_color();
        Line::from(vec![
            Span::styled("▌", Style::default().fg(error_color)),
            " ".into(),
        ])
    }

    pub(super) fn dot_prefix(&self, status: DotStatus) -> Line<'static> {
        match status {
            DotStatus::Pending => Self::pending_dot_prefix(),
            DotStatus::Completed => Self::completed_dot_prefix(),
            DotStatus::Failed => self.failed_dot_prefix(),
        }
    }

    pub(super) fn format_token_count(value: usize) -> String {
        if value >= 1_000_000 {
            format!("{:.1}M", value as f64 / 1_000_000.0)
        } else if value >= 1_000 {
            format!("{:.1}k", value as f64 / 1_000.0)
        } else {
            value.to_string()
        }
    }

    pub(super) fn context_usage(&self) -> Option<(usize, usize, usize)> {
        let model = self.session.model.as_ref()?;
        let total = (model
            .context_window
            .saturating_mul(model.effective_context_window_percent() as u32)
            / 100) as usize;
        let used = self.last_query_input_tokens.min(total);
        let percent = if total == 0 {
            0
        } else {
            used.saturating_mul(100) / total
        };
        Some((used, total, percent))
    }

    pub(super) fn format_compact_token_count(value: usize) -> String {
        if value >= 1_000_000 {
            format!("{:.1}M", value as f64 / 1_000_000.0)
        } else if value >= 1_000 {
            format!("{:.0}k", value as f64 / 1_000.0)
        } else {
            value.to_string()
        }
    }

    pub(super) fn render_progress_bar(used: usize, total: usize, bar_width: usize) -> String {
        if total == 0 {
            return String::new();
        }
        let ratio = (used as f64 / total as f64).clamp(0.0, 1.0);
        let filled = (ratio * bar_width as f64).round() as usize;
        let empty = bar_width.saturating_sub(filled);
        let bar: String = std::iter::repeat_n('▰', filled)
            .chain(std::iter::repeat_n('▱', empty))
            .collect();
        let pct = (ratio * 100.0).round() as usize;
        format!("{bar} {pct}%")
    }

    pub(super) fn percent_of(numerator: usize, denominator: usize) -> usize {
        if denominator == 0 {
            0
        } else {
            (numerator.saturating_mul(100) + denominator / 2) / denominator
        }
    }

    pub(super) fn session_summary_text(&self) -> String {
        let model = self
            .session
            .model
            .as_ref()
            .map(|model| model.slug.as_str())
            .unwrap_or("unknown");
        let thinking = self.thinking_selection.as_deref().unwrap_or("default");
        let cached_input_percent =
            Self::percent_of(self.total_cache_read_tokens, self.total_input_tokens);
        let context = self
            .context_usage()
            .map_or_else(String::new, |(used, total, _percent)| {
                format!(
                    "{} {}/{}",
                    Self::render_progress_bar(used, total, 10),
                    Self::format_compact_token_count(used),
                    Self::format_compact_token_count(total)
                )
            });

        let mut parts: Vec<String> = Vec::new();
        parts.push(format!("{model} {thinking}"));
        parts.push(format!(
            "↑{}",
            Self::format_compact_token_count(self.total_input_tokens)
        ));
        parts.push(format!(
            "(cached {} {}%)",
            Self::format_compact_token_count(self.total_cache_read_tokens),
            cached_input_percent
        ));
        parts.push(format!(
            "↓{}",
            Self::format_compact_token_count(self.total_output_tokens)
        ));
        if !context.is_empty() {
            parts.push(context);
        }
        parts.join("  ")
    }

    pub(super) fn sync_bottom_pane_summary(&mut self) {
        self.bottom_pane
            .set_status_line(Some(Line::from(self.session_summary_text()).dim()));
        self.bottom_pane.set_status_line_enabled(true);
    }

    pub(super) fn push_session_header(
        &mut self,
        is_first_run: bool,
        startup_tooltip_override: Option<String>,
    ) {
        self.history
            .push(self.build_current_header_box(is_first_run, startup_tooltip_override));
    }

    pub(super) fn build_current_header_box(
        &self,
        is_first_run: bool,
        startup_tooltip_override: Option<String>,
    ) -> Box<dyn HistoryCell> {
        let accent = self.active_accent_color();
        Self::build_header_box(
            &self.session.cwd,
            self.session.model.as_ref(),
            self.thinking_selection.as_deref(),
            is_first_run,
            startup_tooltip_override,
            accent,
            self.startup_header_mascot_frame_index,
        )
    }

    pub(super) fn history_has_non_header_content(&self) -> bool {
        self.history.iter().any(|cell| {
            cell.as_any()
                .downcast_ref::<history_cell::SessionInfoCell>()
                .is_none()
        })
    }

    pub(super) fn last_known_width(&self) -> u16 {
        crossterm::terminal::size()
            .map(|(width, _height)| width)
            .unwrap_or(80)
    }

    pub(super) fn refresh_header_box(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let accent = self.active_accent_color();
        self.history[0] = Self::build_header_box(
            &self.session.cwd,
            self.session.model.as_ref(),
            self.thinking_selection.as_deref(),
            /*is_first_run*/ false,
            None,
            accent,
            self.startup_header_mascot_frame_index,
        );
    }

    pub(super) fn advance_startup_header_animation(&mut self) {
        let now = Instant::now();
        if self
            .history
            .first()
            .and_then(|cell| {
                cell.as_any()
                    .downcast_ref::<history_cell::SessionInfoCell>()
            })
            .is_none()
        {
            return;
        }

        self.frame_requester
            .schedule_frame_in(STARTUP_HEADER_ANIMATION_INTERVAL);
        if now < self.startup_header_next_animation_at {
            return;
        }

        self.startup_header_mascot_frame_index = (self.startup_header_mascot_frame_index + 1) % 3;
        self.startup_header_next_animation_at = now + STARTUP_HEADER_ANIMATION_INTERVAL;
        self.refresh_header_box();
    }

    pub(crate) fn current_model(&self) -> Option<&Model> {
        self.session.model.as_ref()
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn current_cwd(&self) -> &std::path::Path {
        &self.session.cwd
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn startup_header_mascot_frame_index(&self) -> usize {
        self.startup_header_mascot_frame_index
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn has_stream_controller(&self) -> bool {
        self.active_text_items
            .iter()
            .any(|item| item.stream_controller.is_some())
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn force_startup_header_animation_due(&mut self) {
        self.startup_header_next_animation_at = Instant::now();
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn force_task_elapsed_seconds(&mut self, secs: u64) {
        self.bottom_pane.set_task_running(true);
        if let Some(status) = self.bottom_pane.status_widget_mut() {
            let now = Instant::now();
            status.pause_timer_at(now);
            let resume_at = now
                .checked_sub(std::time::Duration::from_secs(secs))
                .unwrap_or(now);
            status.resume_timer_at(resume_at);
        }
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn placeholder_text(&self) -> &str {
        self.bottom_pane.placeholder_text()
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fn status_summary_text(&self) -> String {
        self.session_summary_text()
    }

    pub(super) fn reasoning_text_style() -> Style {
        Style::default().dim()
    }

    pub(super) fn reasoning_heading_style() -> Style {
        Style::default().italic().fg(Color::Rgb(210, 150, 60))
    }

    pub(super) fn patch_lines_style(lines: &mut [Line<'static>], style: Style) {
        if style == Style::default() {
            return;
        }

        for line in lines {
            line.spans = line
                .spans
                .drain(..)
                .map(|span| span.patch_style(style))
                .collect();
        }
    }
}
