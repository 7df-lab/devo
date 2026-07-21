//! Reasoning-view picker data for the chat widget.
//!
//! Controls whether reasoning content is shown in full or collapsed in the
//! main transcript viewport.

use std::path::Path;

use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

use crate::app_event::AppEvent;
use crate::bottom_pane::list_selection_view::SelectionItem;
use crate::history_cell;
use crate::history_cell::HistoryCell;
use crate::history_cell::ReasoningViewportMode;
use crate::markdown::append_markdown;

/// Live streaming window when reasoning view is collapsed.
pub(super) const COLLAPSED_REASONING_LIVE_LINES: usize = 3;

pub(super) fn reasoning_view_items(collapse_reasoning: bool) -> Vec<SelectionItem> {
    [(true, "Collapsed"), (false, "Full")]
        .into_iter()
        .map(|(collapsed, label)| SelectionItem {
            name: label.to_string(),
            description: None,
            is_current: collapsed == collapse_reasoning,
            dismiss_on_select: true,
            actions: vec![Box::new(move |app_event_tx| {
                app_event_tx.send(AppEvent::CollapseReasoningSelected { collapsed });
            })],
            ..Default::default()
        })
        .collect()
}

pub(super) fn reasoning_view_label(collapse_reasoning: bool) -> &'static str {
    if collapse_reasoning {
        "Collapsed"
    } else {
        "Full"
    }
}

/// Build the committed reasoning cell for collapsed mode.
///
/// Short reasoning stays fully visible. Longer reasoning becomes a one-line
/// Thought summary in the main viewport, with the full body kept for Ctrl+T.
pub(super) fn collapsed_reasoning_history_cell(
    content: String,
    cwd: &Path,
    status_heading: &str,
    status_heading_style: Style,
    reasoning_text_style: Style,
    dot_prefix: Line<'static>,
) -> Box<dyn HistoryCell> {
    let mut body_lines = Vec::new();
    append_markdown(&content, /*width*/ None, Some(cwd), &mut body_lines);
    for line in &mut body_lines {
        line.spans = line
            .spans
            .iter()
            .cloned()
            .map(|span| span.patch_style(reasoning_text_style))
            .collect();
    }

    if body_lines.len() <= COLLAPSED_REASONING_LIVE_LINES {
        if let Some(first_line) = body_lines.first_mut() {
            first_line.spans.insert(
                0,
                Span::styled(status_heading.to_string(), status_heading_style),
            );
        }
        body_lines.push(history_cell::reasoning_transcript_hint_line());
        return Box::new(history_cell::AgentMessageCell::new_ai_response_with_prefix(
            body_lines, dot_prefix, "  ", false,
        ));
    }

    Box::new(history_cell::ReasoningSummaryCell::new(
        String::new(),
        content,
        cwd,
        ReasoningViewportMode::Compact,
    ))
}
