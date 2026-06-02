//! Resume-session browser rendering and navigation for `ChatWidget`.
//!
//! The chat widget owns resume-browser state while this module keeps the
//! popup-style list rendering and key handling separate from the main surface.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Block;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::events::SessionListEntry;

use super::ChatWidget;

#[derive(Debug, Clone)]
pub(super) struct ResumeBrowserState {
    pub(super) sessions: Vec<SessionListEntry>,
    pub(super) selection: usize,
    pub(super) scroll_offset: usize,
}

impl ChatWidget {
    pub(super) fn open_resume_browser(&mut self, sessions: Vec<SessionListEntry>) {
        self.resume_browser_loading = false;
        let selection = sessions
            .iter()
            .position(|session| session.is_active)
            .unwrap_or(0);
        self.resume_browser = Some(ResumeBrowserState {
            sessions,
            selection,
            scroll_offset: 0,
        });
        self.set_status_message("Resume session");
    }

    pub(super) fn handle_resume_browser_key_event(&mut self, key: KeyEvent) {
        if !matches!(key.kind, KeyEventKind::Press) {
            return;
        }
        let Some(browser) = self.resume_browser.as_mut() else {
            return;
        };
        let page_step = Self::resume_browser_visible_capacity(
            self.resume_browser_last_height.get(),
            !browser.sessions.is_empty(),
        )
        .max(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.resume_browser = None;
                self.resume_browser_loading = false;
                self.set_status_message("Ready");
                self.frame_requester.schedule_frame();
            }
            KeyCode::Up => {
                if browser.sessions.is_empty() {
                    browser.selection = 0;
                } else if browser.selection > 0 {
                    browser.selection -= 1;
                }
                self.ensure_resume_selection_visible(u16::MAX);
                self.frame_requester.schedule_frame();
            }
            KeyCode::Down => {
                if browser.sessions.is_empty() {
                    browser.selection = 0;
                } else if browser.selection + 1 < browser.sessions.len() {
                    browser.selection += 1;
                }
                self.ensure_resume_selection_visible(u16::MAX);
                self.frame_requester.schedule_frame();
            }
            KeyCode::PageUp => {
                if browser.sessions.is_empty() {
                    browser.selection = 0;
                } else {
                    browser.selection = browser.selection.saturating_sub(page_step);
                }
                self.ensure_resume_selection_visible(u16::MAX);
                self.frame_requester.schedule_frame();
            }
            KeyCode::PageDown => {
                if browser.sessions.is_empty() {
                    browser.selection = 0;
                } else {
                    browser.selection = browser
                        .selection
                        .saturating_add(page_step)
                        .min(browser.sessions.len().saturating_sub(1));
                }
                self.ensure_resume_selection_visible(u16::MAX);
                self.frame_requester.schedule_frame();
            }
            KeyCode::Home => {
                browser.selection = 0;
                self.ensure_resume_selection_visible(u16::MAX);
                self.frame_requester.schedule_frame();
            }
            KeyCode::End => {
                if browser.sessions.is_empty() {
                    browser.selection = 0;
                } else {
                    browser.selection = browser.sessions.len().saturating_sub(1);
                }
                self.ensure_resume_selection_visible(u16::MAX);
                self.frame_requester.schedule_frame();
            }
            KeyCode::Enter => {
                if let Some(selected) = browser.sessions.get(browser.selection) {
                    let session_id = selected.session_id;
                    self.resume_browser = None;
                    self.clear_for_session_switch();
                    self.app_event_tx
                        .send(AppEvent::Command(AppCommand::switch_session(session_id)));
                }
            }
            KeyCode::Backspace
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Char(_)
            | KeyCode::F(_)
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Delete
            | KeyCode::Insert
            | KeyCode::Null
            | KeyCode::CapsLock
            | KeyCode::ScrollLock
            | KeyCode::NumLock
            | KeyCode::PrintScreen
            | KeyCode::Pause
            | KeyCode::Menu
            | KeyCode::KeypadBegin
            | KeyCode::Media(_)
            | KeyCode::Modifier(_) => {}
        }
    }

    pub(crate) fn is_resume_browser_open(&self) -> bool {
        self.resume_browser_loading || self.resume_browser.is_some()
    }

    fn resume_browser_entry_height() -> usize {
        1
    }

    fn resume_browser_chrome_height(has_sessions: bool) -> usize {
        if has_sessions { 7 } else { 6 }
    }

    fn resume_browser_visible_capacity(area_height: u16, has_sessions: bool) -> usize {
        area_height.saturating_sub(Self::resume_browser_chrome_height(has_sessions) as u16) as usize
    }

    fn resume_browser_window(
        sessions_len: usize,
        selection: usize,
        requested_offset: usize,
        area_height: u16,
    ) -> (usize, usize, bool, bool) {
        if sessions_len == 0 {
            return (0, 0, false, false);
        }
        let list_window = Self::resume_browser_visible_capacity(area_height, true);
        if list_window == 0 {
            return (selection.min(sessions_len.saturating_sub(1)), 0, true, true);
        }

        let selection = selection.min(sessions_len.saturating_sub(1));
        let mut start = requested_offset.min(sessions_len.saturating_sub(1));
        let mut slots = list_window;

        loop {
            if slots == 0 {
                return (selection, 0, start > 0, selection + 1 < sessions_len);
            }
            let end = (start + slots).min(sessions_len);
            let has_above = start > 0;
            let has_below = end < sessions_len;
            let indicator_rows = usize::from(has_above) + usize::from(has_below);
            let session_slots = list_window.saturating_sub(indicator_rows);
            if session_slots == slots {
                let end = (start + session_slots).min(sessions_len);
                let has_above = start > 0;
                let has_below = end < sessions_len;
                return (start, end, has_above, has_below);
            }
            slots = session_slots;
            if selection < start {
                start = selection;
            } else if selection >= start + slots {
                start = selection + 1 - slots;
            }
            start = start.min(sessions_len.saturating_sub(slots.max(1)));
        }
    }

    fn resume_browser_footer_lines(has_sessions: bool) -> Vec<Line<'static>> {
        if has_sessions {
            vec![
                Line::from("↑/↓ select  pgup/pgdn page  home/end jump".dim()),
                Line::from("enter resume  q back".dim()),
            ]
        } else {
            vec![Line::from("q back".dim())]
        }
    }

    fn resume_browser_progress_label(
        selection: usize,
        sessions_len: usize,
        rendered_start: usize,
        area_height: u16,
    ) -> String {
        if sessions_len == 0 {
            return " 0 / 0 · 100% ".to_string();
        }
        let position = selection.saturating_add(1);
        let total = sessions_len;
        let capacity = Self::resume_browser_visible_capacity(area_height, true);
        let max_scroll = sessions_len.saturating_sub(capacity.max(1));
        let percent = if max_scroll == 0 {
            100
        } else {
            ((rendered_start.min(max_scroll) as f32 / max_scroll as f32) * 100.0).round() as usize
        };
        format!(" {position} / {total} · {percent}% ")
    }

    fn ensure_resume_selection_visible(&mut self, area_height: u16) {
        let Some(browser) = self.resume_browser.as_mut() else {
            return;
        };
        if browser.sessions.is_empty() {
            browser.selection = 0;
            browser.scroll_offset = 0;
            return;
        }

        let selection = browser
            .selection
            .min(browser.sessions.len().saturating_sub(1));
        browser.selection = selection;
        let capacity = Self::resume_browser_visible_capacity(area_height, true);
        if capacity == 0 {
            browser.scroll_offset = selection;
            return;
        }

        if selection < browser.scroll_offset {
            browser.scroll_offset = selection;
        } else {
            let selection_bottom = selection + Self::resume_browser_entry_height();
            let viewport_bottom = browser.scroll_offset + capacity;
            if selection_bottom > viewport_bottom {
                browser.scroll_offset = selection_bottom.saturating_sub(capacity);
            }
        }

        let max_offset = browser.sessions.len().saturating_sub(capacity);
        browser.scroll_offset = browser.scroll_offset.min(max_offset);
    }

    pub(super) fn render_resume_browser_if_open(&self, area: Rect, buf: &mut Buffer) -> bool {
        if self.resume_browser_loading {
            let lines = vec![
                Line::from("Resume Session".bold()),
                Line::from("Loading saved sessions...".dim()),
                Line::from(""),
                Line::from("Please wait.".dim()),
            ];
            Paragraph::new(Text::from(lines))
                .block(Block::default().title("Devo Sessions"))
                .wrap(Wrap { trim: false })
                .render(area, buf);
            return true;
        }

        let Some(browser) = &self.resume_browser else {
            return false;
        };

        self.resume_browser_last_height.set(area.height);
        Block::default().style(Style::default()).render(area, buf);
        let (scroll_offset, end, has_above, has_below) = Self::resume_browser_window(
            browser.sessions.len(),
            browser.selection,
            browser.scroll_offset,
            area.height,
        );
        let title_width = browser
            .sessions
            .iter()
            .map(|session| unicode_width::UnicodeWidthStr::width(session.title.as_str()))
            .max()
            .unwrap_or(5)
            .clamp(5, 48);
        let progress = Self::resume_browser_progress_label(
            browser.selection,
            browser.sessions.len(),
            scroll_offset,
            area.height,
        );
        let mut lines = vec![Line::from(vec![
            Span::styled("Resume Session", Style::default().bold()),
            Span::raw(" "),
            Span::styled(progress, Style::default().dim()),
        ])];
        if browser.sessions.is_empty() {
            lines.push(Line::from("No saved sessions found.".dim()));
        } else {
            lines.push(
                Line::from(format!(
                    "  {:title_width$}  {:<36}  {}",
                    "Title",
                    "Session ID",
                    "Updated",
                    title_width = title_width
                ))
                .dim(),
            );
            lines.push(
                Line::from(format!(
                    "  {}  {}  {}",
                    "-".repeat(title_width),
                    "-".repeat(36),
                    "-".repeat(23)
                ))
                .dim(),
            );
            if has_above {
                lines.push(Line::from("  ↑ more").dim());
            }
            for (index, session) in browser
                .sessions
                .iter()
                .enumerate()
                .skip(scroll_offset)
                .take(end.saturating_sub(scroll_offset))
            {
                let is_selected = index == browser.selection;
                let marker = if session.is_active {
                    "●"
                } else if is_selected {
                    ">"
                } else {
                    " "
                };
                let display_title = Self::pad_display_text(
                    &Self::truncate_display_text(&session.title, title_width),
                    title_width,
                );
                let line = format!(
                    "{marker} {}  {:<16}  {}",
                    display_title, session.session_id, session.updated_at
                );
                lines.push(if is_selected {
                    Line::from(line).bold()
                } else if session.is_active {
                    Line::from(line).style(Style::default().fg(self.active_accent_color()))
                } else {
                    Line::from(line)
                });
            }
            if has_below {
                lines.push(Line::from("  ↓ more").dim());
            }
        }
        lines.extend(Self::resume_browser_footer_lines(
            !browser.sessions.is_empty(),
        ));
        Paragraph::new(Text::from(lines))
            .block(Block::default().title("Devo Sessions"))
            .wrap(Wrap { trim: false })
            .render(area, buf);
        true
    }

    #[cfg(test)]
    pub(crate) fn resume_browser_selection_for_test(&self) -> Option<usize> {
        self.resume_browser
            .as_ref()
            .map(|browser| browser.selection)
    }

    #[cfg(test)]
    pub(crate) fn resume_browser_scroll_offset_for_test(&self) -> Option<usize> {
        self.resume_browser
            .as_ref()
            .map(|browser| browser.scroll_offset)
    }

    #[cfg(test)]
    pub(crate) fn open_resume_browser_for_test(&mut self, sessions: Vec<SessionListEntry>) {
        self.open_resume_browser(sessions);
    }
}
