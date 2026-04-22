use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::key_hint;
use crate::render::renderable::Renderable;
use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_lines;

/// Widget that displays pending steers plus follow-up messages held while a turn is in progress.
///
/// The widget renders pending steers first, then rejected steers that will be
/// resubmitted at end of turn, then ordinary queued user messages. Pending
/// steers explain that they will be submitted after the next tool/result
/// boundary unless the user presses Esc to interrupt and send them
/// immediately. The edit hint at the bottom only appears when there are actual
/// queued user messages to pop back into the composer. Because some terminals
/// intercept certain modifier-key combinations, the displayed binding is
/// configurable via [`set_edit_binding`](Self::set_edit_binding).
pub(crate) struct PendingInputPreview {
    pub pending_steers: Vec<String>,
    pub rejected_steers: Vec<String>,
    pub queued_messages: Vec<String>,
    /// Key combination rendered in the hint line.  Defaults to Alt+Up but may
    /// be overridden for terminals where that chord is unavailable.
    edit_binding: key_hint::KeyBinding,
}

const PREVIEW_LINE_LIMIT: usize = 3;

impl PendingInputPreview {
    pub(crate) fn new() -> Self {
        Self {
            pending_steers: Vec::new(),
            rejected_steers: Vec::new(),
            queued_messages: Vec::new(),
            edit_binding: key_hint::alt(KeyCode::Up),
        }
    }

    /// Replace the keybinding shown in the hint line at the bottom of the
    /// queued-messages list.  The caller is responsible for also wiring the
    /// corresponding key event handler.
    pub(crate) fn set_edit_binding(&mut self, binding: key_hint::KeyBinding) {
        self.edit_binding = binding;
    }

    fn push_truncated_preview_lines(
        lines: &mut Vec<Line<'static>>,
        wrapped: Vec<Line<'static>>,
        overflow_line: Line<'static>,
    ) {
        let wrapped_len = wrapped.len();
        lines.extend(wrapped.into_iter().take(PREVIEW_LINE_LIMIT));
        if wrapped_len > PREVIEW_LINE_LIMIT {
            lines.push(overflow_line);
        }
    }

    fn push_section_header(lines: &mut Vec<Line<'static>>, width: u16, header: Line<'static>) {
        let mut spans = vec!["• ".dim()];
        spans.extend(header.spans);
        lines.extend(adaptive_wrap_lines(
            std::iter::once(Line::from(spans)),
            RtOptions::new(width as usize).subsequent_indent(Line::from("  ".dim())),
        ));
    }

    fn as_renderable(&self, width: u16) -> Box<dyn Renderable> {
        if (self.pending_steers.is_empty()
            && self.rejected_steers.is_empty()
            && self.queued_messages.is_empty())
            || width < 4
        {
            return Box::new(());
        }

        let mut lines = vec![];

        if !self.pending_steers.is_empty() {
            Self::push_section_header(
                &mut lines,
                width,
                Line::from(vec![
                    "Messages to be submitted after next tool call".into(),
                    " (press ".dim(),
                    key_hint::plain(KeyCode::Esc).into(),
                    " to interrupt and send immediately)".dim(),
                ]),
            );

            for steer in &self.pending_steers {
                let wrapped = adaptive_wrap_lines(
                    steer.lines().map(|line| Line::from(line.dim())),
                    RtOptions::new(width as usize)
                        .initial_indent(Line::from("  ↳ ".dim()))
                        .subsequent_indent(Line::from("    ")),
                );
                Self::push_truncated_preview_lines(&mut lines, wrapped, Line::from("    …".dim()));
            }
        }

        if !self.rejected_steers.is_empty() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            Self::push_section_header(
                &mut lines,
                width,
                "Messages to be submitted at end of turn".into(),
            );

            for steer in &self.rejected_steers {
                let wrapped = adaptive_wrap_lines(
                    steer.lines().map(|line| Line::from(line.dim())),
                    RtOptions::new(width as usize)
                        .initial_indent(Line::from("  ↳ ".dim()))
                        .subsequent_indent(Line::from("    ")),
                );
                Self::push_truncated_preview_lines(&mut lines, wrapped, Line::from("    …".dim()));
            }
        }

        if !self.queued_messages.is_empty() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            Self::push_section_header(&mut lines, width, "Queued follow-up messages".into());

            for message in &self.queued_messages {
                let wrapped = adaptive_wrap_lines(
                    message.lines().map(|line| Line::from(line.dim().italic())),
                    RtOptions::new(width as usize)
                        .initial_indent(Line::from("  ↳ ".dim()))
                        .subsequent_indent(Line::from("    ")),
                );
                Self::push_truncated_preview_lines(
                    &mut lines,
                    wrapped,
                    Line::from("    …".dim().italic()),
                );
            }
        }

        if !self.queued_messages.is_empty() {
            lines.push(
                Line::from(vec![
                    "    ".into(),
                    self.edit_binding.into(),
                    " edit last queued message".into(),
                ])
                .dim(),
            );
        }

        Paragraph::new(lines).into()
    }
}

impl Renderable for PendingInputPreview {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        self.as_renderable(area.width).render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.as_renderable(width).desired_height(width)
    }
}
