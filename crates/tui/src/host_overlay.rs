//! Host-side lifecycle helpers for alternate-screen overlays.

use crate::ansi_escape::ansi_escape_line;
use anyhow::Result;
use devo_core::SessionId;
use ratatui::style::Stylize;
use ratatui::text::Line;

use crate::chatwidget::ChatWidget;
use crate::pager_overlay::Overlay;
use crate::pager_overlay::TranscriptOverlay;
use crate::tui::Tui;
use crate::tui::TuiEvent;

#[derive(Debug, Default)]
pub(crate) struct OverlayState {
    overlay: Option<Overlay>,
    transcript_source: Option<TranscriptSource>,
}

#[derive(Clone, Copy, Debug)]
enum TranscriptSource {
    Parent,
    Subagent(SessionId),
}

impl OverlayState {
    pub(crate) fn is_active(&self) -> bool {
        self.overlay.is_some()
    }

    pub(crate) fn handle_tui_event(
        &mut self,
        tui_event: TuiEvent,
        tui: &mut Tui,
        chat_widget: &mut ChatWidget,
    ) -> Result<()> {
        let Some(overlay) = self.overlay.as_mut() else {
            return Ok(());
        };

        if matches!(tui_event, TuiEvent::Draw)
            && let Overlay::Transcript(transcript) = overlay
        {
            match self.transcript_source.unwrap_or(TranscriptSource::Parent) {
                TranscriptSource::Parent => sync_transcript_overlay(transcript, tui, chat_widget)?,
                TranscriptSource::Subagent(session_id) => {
                    sync_subagent_transcript_overlay(transcript, tui, chat_widget, session_id)?;
                }
            }
        }

        overlay.handle_event(tui, tui_event)?;
        if overlay.is_done() {
            self.overlay = None;
            self.transcript_source = None;
            tui.leave_alt_screen()?;
            tui.frame_requester().schedule_frame();
        } else if let Overlay::Transcript(transcript) = overlay
            && transcript.is_scrolled_to_bottom()
            && transcript_source_live_tail_animation(
                self.transcript_source.unwrap_or(TranscriptSource::Parent),
                chat_widget,
            )
        {
            tui.frame_requester()
                .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
        }

        Ok(())
    }

    pub(crate) fn open_transcript(
        &mut self,
        tui: &mut Tui,
        chat_widget: &ChatWidget,
    ) -> Result<()> {
        let width = tui.terminal.size()?.width.max(1);
        tui.enter_alt_screen()?;
        self.overlay = Some(Overlay::new_transcript(
            chat_widget.transcript_overlay_cells(width),
            width,
        ));
        self.transcript_source = Some(TranscriptSource::Parent);
        tui.frame_requester().schedule_frame();
        Ok(())
    }

    pub(crate) fn open_subagent_transcript(
        &mut self,
        tui: &mut Tui,
        chat_widget: &mut ChatWidget,
        session_id: SessionId,
    ) -> Result<()> {
        let width = tui.terminal.size()?.width.max(1);
        let Some(cells) = chat_widget.subagent_transcript_overlay_cells(session_id, width) else {
            chat_widget.set_status_message("No active sub-agent");
            tui.frame_requester().schedule_frame();
            return Ok(());
        };
        let title = chat_widget
            .subagent_overlay_title(session_id)
            .unwrap_or_else(|| "Sub-agent".to_string());
        tui.enter_alt_screen()?;
        self.overlay = Some(Overlay::new_transcript_with_title(cells, width, title));
        self.transcript_source = Some(TranscriptSource::Subagent(session_id));
        tui.frame_requester().schedule_frame();
        Ok(())
    }

    pub(crate) fn transcript_mut(&mut self) -> Option<&mut TranscriptOverlay> {
        match self.overlay.as_mut() {
            Some(Overlay::Transcript(overlay)) => Some(overlay),
            _ => None,
        }
    }

    pub(crate) fn parent_transcript(&self) -> Option<&TranscriptOverlay> {
        if !matches!(self.transcript_source, Some(TranscriptSource::Parent)) {
            return None;
        }
        match self.overlay.as_ref() {
            Some(Overlay::Transcript(overlay)) => Some(overlay),
            Some(Overlay::Static(_)) | None => None,
        }
    }

    pub(crate) fn close(&mut self, tui: &mut Tui) -> Result<()> {
        self.overlay = None;
        self.transcript_source = None;
        tui.leave_alt_screen()?;
        tui.frame_requester().schedule_frame();
        Ok(())
    }

    pub(crate) fn open_diff(
        &mut self,
        tui: &mut Tui,
        chat_widget: &mut ChatWidget,
        text: String,
    ) -> Result<()> {
        tui.enter_alt_screen()?;
        self.overlay = Some(Overlay::new_static_with_lines(
            diff_overlay_lines(&text),
            "D I F F".to_string(),
        ));
        self.transcript_source = None;
        chat_widget.set_status_message("Diff shown");
        tui.frame_requester().schedule_frame();
        Ok(())
    }
}

fn transcript_source_live_tail_animation(
    source: TranscriptSource,
    chat_widget: &ChatWidget,
) -> bool {
    match source {
        TranscriptSource::Parent => chat_widget
            .transcript_overlay_live_tail_key()
            .is_some_and(|key| key.animation_tick.is_some()),
        TranscriptSource::Subagent(session_id) => chat_widget
            .subagent_transcript_overlay_live_tail_key(session_id)
            .is_some_and(|key| key.animation_tick.is_some()),
    }
}

fn sync_transcript_overlay(
    transcript: &mut TranscriptOverlay,
    tui: &mut Tui,
    chat_widget: &ChatWidget,
) -> Result<()> {
    let width = tui.terminal.size()?.width.max(1);
    let cell_count = chat_widget.transcript_overlay_cell_count();
    if transcript.needs_committed_cells_sync(width, cell_count) {
        transcript.replace_committed_cells(width, chat_widget.transcript_overlay_cells(width));
    }
    let live_tail_key = chat_widget.transcript_overlay_live_tail_key();
    transcript.sync_live_tail(width, live_tail_key, |tail_width| {
        chat_widget.transcript_overlay_live_tail_lines(tail_width)
    });
    Ok(())
}

fn sync_subagent_transcript_overlay(
    transcript: &mut TranscriptOverlay,
    tui: &mut Tui,
    chat_widget: &ChatWidget,
    session_id: SessionId,
) -> Result<()> {
    let width = tui.terminal.size()?.width.max(1);
    let cell_count = chat_widget
        .subagent_transcript_overlay_cell_count(session_id)
        .unwrap_or(0);
    if transcript.needs_committed_cells_sync(width, cell_count) {
        transcript.replace_committed_cells(
            width,
            chat_widget
                .subagent_transcript_overlay_cells(session_id, width)
                .unwrap_or_default(),
        );
    }
    let live_tail_key = chat_widget.subagent_transcript_overlay_live_tail_key(session_id);
    transcript.sync_live_tail(width, live_tail_key, |tail_width| {
        chat_widget.subagent_transcript_overlay_live_tail_lines(session_id, tail_width)
    });
    Ok(())
}

fn diff_overlay_lines(text: &str) -> Vec<Line<'static>> {
    if text.trim().is_empty() {
        vec!["No changes detected.".italic().into()]
    } else {
        text.lines().map(ansi_escape_line).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn diff_overlay_lines_render_empty_diff_message() {
        let lines = diff_overlay_lines("");
        assert_eq!(1, lines.len());
        let text = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!("No changes detected.", text);
    }
}
