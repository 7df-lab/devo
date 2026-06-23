use std::path::Path;
use std::path::PathBuf;

use ratatui::prelude::*;
use ratatui::style::Color;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

use crate::history_cell::HistoryCell;
use crate::history_cell::collapse_consecutive_blank_lines;
use crate::markdown::append_markdown;
use crate::render::line_utils::prefix_lines;
use crate::style::user_message_style;
use crate::ui_consts::LIVE_PREFIX_COLS;
use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_lines;

#[derive(Debug)]
pub(crate) struct ResearchArtifactCell {
    markdown_source: String,
    cwd: PathBuf,
}

impl ResearchArtifactCell {
    pub(crate) fn new(
        _title: impl Into<String>,
        markdown_source: impl Into<String>,
        cwd: &Path,
    ) -> Self {
        let markdown_source = strip_leading_title(markdown_source.into());
        Self {
            markdown_source,
            cwd: cwd.to_path_buf(),
        }
    }

    fn content_lines(&self) -> Vec<Line<'static>> {
        let style = user_message_style();
        let mut lines = Vec::new();
        append_markdown(
            &self.markdown_source,
            /*width*/ None,
            Some(self.cwd.as_path()),
            &mut lines,
        );
        let mut lines = collapse_consecutive_blank_lines(lines);
        patch_lines_style(&mut lines, style);
        lines
    }

    fn block_prefix_style() -> Style {
        user_message_style().fg(Color::Cyan)
    }

    fn blank_prefixed_line() -> Line<'static> {
        let style = user_message_style();
        Line::from(Span::styled("  ", style)).style(style)
    }

    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        let wrap_width = width
            .saturating_sub(
                LIVE_PREFIX_COLS + 1, /* keep a one-column right margin for wrapping */
            )
            .max(1);
        let content_lines = self.content_lines();
        if !content_lines.iter().any(|line| !line_is_blank(line)) {
            return Vec::new();
        }
        let content_lines = adaptive_wrap_lines(
            content_lines,
            RtOptions::new(usize::from(wrap_width))
                .wrap_algorithm(textwrap::WrapAlgorithm::FirstFit),
        );

        let mut lines = vec![Self::blank_prefixed_line()];
        lines.extend(prefix_lines(
            content_lines,
            Span::styled("▌ ", Self::block_prefix_style()),
            Span::styled("  ", user_message_style()),
        ));
        lines.push(Self::blank_prefixed_line());
        pad_lines_to_width(&mut lines, usize::from(width), user_message_style());
        lines
    }
}

impl HistoryCell for ResearchArtifactCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.lines(width)
    }

    fn desired_height(&self, width: u16) -> u16 {
        let lines = self.lines(width);
        if lines.is_empty() {
            return 0;
        }
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .line_count(width)
            .try_into()
            .unwrap_or(0)
    }
}

fn strip_leading_title(markdown_source: String) -> String {
    let trimmed = markdown_source.trim_start();
    let Some(rest) = trimmed.strip_prefix("### ") else {
        return markdown_source;
    };
    let Some((_heading, body)) = rest.split_once('\n') else {
        return String::new();
    };
    body.trim_start_matches(['\r', '\n']).to_string()
}

fn patch_lines_style(lines: &mut [Line<'static>], style: Style) {
    for line in lines {
        line.style = line.style.patch(style);
        for span in &mut line.spans {
            span.style = span.style.patch(style);
        }
    }
}

fn line_is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.trim().is_empty())
}

fn pad_lines_to_width(lines: &mut [Line<'static>], width: usize, style: Style) {
    if width == 0 {
        return;
    }
    for line in lines {
        let padding = width.saturating_sub(line.width());
        if padding > 0 {
            line.spans.push(Span::styled(" ".repeat(padding), style));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn renders_markdown_inside_block_cell() {
        let cell = ResearchArtifactCell::new(
            "Research",
            "### Finding\n\n- **first** item\n- second item",
            Path::new("."),
        );

        let rows = trimmed_plain_rows(cell.display_lines(80));

        assert_eq!(vec!["", "▌ - first item", "  - second item", "",], rows);
    }

    #[test]
    fn renders_body_without_generic_title_when_body_has_no_heading() {
        let cell = ResearchArtifactCell::new("Research", "partial finding", Path::new("."));

        let rows = trimmed_plain_rows(cell.display_lines(80));

        assert_eq!(vec!["", "▌ partial finding", ""], rows);
    }

    #[test]
    fn strips_research_title_heading() {
        let cell = ResearchArtifactCell::new(
            "Research",
            "### Research Brief\n\nUnderstand the current behavior.",
            Path::new("."),
        );

        let rows = trimmed_plain_rows(cell.display_lines(80));

        assert_eq!(vec!["", "▌ Understand the current behavior.", ""], rows);
    }

    #[test]
    fn title_only_artifact_is_not_visible() {
        let cell = ResearchArtifactCell::new("Research", "### Compressed Finding", Path::new("."));

        assert_eq!(
            Vec::<String>::new(),
            trimmed_plain_rows(cell.display_lines(80))
        );
        assert_eq!(0, cell.desired_height(80));
    }

    #[test]
    fn pads_each_line_to_viewport_width() {
        let cell = ResearchArtifactCell::new("Research", "partial finding", Path::new("."));

        let rows = cell.display_lines(24);

        assert!(rows.iter().all(|line| line.width() == 24));
    }

    #[test]
    fn renders_only_one_gutter_marker() {
        let cell = ResearchArtifactCell::new(
            "Research",
            "### Finding\n\nfirst line\nsecond line",
            Path::new("."),
        );

        let marker_count = trimmed_plain_rows(cell.display_lines(80))
            .iter()
            .filter(|row| row.contains('▌'))
            .count();

        assert_eq!(1, marker_count);
    }

    fn trimmed_plain_rows(lines: Vec<Line<'static>>) -> Vec<String> {
        lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }
}
