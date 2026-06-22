use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ViewportAnchor {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

pub(crate) fn render_lines_with_anchor(
    lines: Vec<Line<'static>>,
    anchor: Option<ViewportAnchor>,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let paragraph = Paragraph::new(Text::from(lines.clone())).wrap(Wrap { trim: false });
    let total_rows = paragraph.line_count(area.width);
    let visible_rows = area.height as usize;
    let Some(anchor) = anchor else {
        paragraph.render(area, buf);
        return;
    };
    if total_rows <= visible_rows {
        paragraph.render(area, buf);
        return;
    }

    let scroll_offset =
        scroll_offset_for_anchor(&lines, anchor, area.width, visible_rows, total_rows);
    if scroll_offset == 0 {
        paragraph.render(area, buf);
        return;
    }

    render_scrolled_paragraph(paragraph, total_rows, scroll_offset, area, buf);
}

pub(crate) fn render_lines_with_fixed_footer(
    body_lines: Vec<Line<'static>>,
    footer_lines: Vec<Line<'static>>,
    anchor: Option<ViewportAnchor>,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let footer_paragraph =
        Paragraph::new(Text::from(footer_lines.clone())).wrap(Wrap { trim: false });
    let footer_rows = footer_paragraph.line_count(area.width);
    let footer_height = u16::try_from(footer_rows)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let body_height = area.height.saturating_sub(footer_height);

    if body_height > 0 {
        render_lines_with_anchor(
            body_lines,
            anchor,
            Rect {
                height: body_height,
                ..area
            },
            buf,
        );
    }

    let footer_area = Rect {
        y: area.y.saturating_add(body_height),
        height: footer_height,
        ..area
    };
    render_footer_paragraph(footer_paragraph, footer_rows, footer_area, buf);
}

fn scroll_offset_for_anchor(
    lines: &[Line<'static>],
    anchor: ViewportAnchor,
    width: u16,
    visible_rows: usize,
    total_rows: usize,
) -> usize {
    if visible_rows == 0 {
        return 0;
    }

    let anchor_start = anchor.start.min(lines.len());
    let anchor_end = anchor.end.max(anchor_start).min(lines.len());
    let anchor_top = wrapped_row_count(&lines[..anchor_start], width);
    let anchor_rows = wrapped_row_count(&lines[anchor_start..anchor_end], width);
    if anchor_rows == 0 {
        return 0;
    }

    let max_scroll = total_rows.saturating_sub(visible_rows);
    if anchor_rows >= visible_rows {
        return anchor_top.min(max_scroll);
    }

    anchor_top
        .saturating_add(anchor_rows)
        .saturating_sub(visible_rows)
        .min(max_scroll)
}

fn wrapped_row_count(lines: &[Line<'static>], width: u16) -> usize {
    if lines.is_empty() || width == 0 {
        return 0;
    }

    Paragraph::new(Text::from(lines.to_vec()))
        .wrap(Wrap { trim: false })
        .line_count(width)
}

fn render_footer_paragraph(
    paragraph: Paragraph<'_>,
    total_rows: usize,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.height == 0 {
        return;
    }

    let visible_rows = area.height as usize;
    if total_rows <= visible_rows {
        paragraph.render(area, buf);
        return;
    }

    let scroll_offset = total_rows.saturating_sub(visible_rows);
    render_scrolled_paragraph(paragraph, total_rows, scroll_offset, area, buf);
}

fn render_scrolled_paragraph(
    paragraph: Paragraph<'_>,
    total_rows: usize,
    scroll_offset: usize,
    area: Rect,
    buf: &mut Buffer,
) {
    let visible_rows = area.height as usize;
    let tall_height = total_rows.min(scroll_offset.saturating_add(visible_rows));
    let tall_height = u16::try_from(tall_height).unwrap_or(u16::MAX);
    let scroll_offset = u16::try_from(scroll_offset).unwrap_or(u16::MAX);
    let mut tall_buf = Buffer::empty(Rect::new(0, 0, area.width, tall_height));

    paragraph.render(*tall_buf.area(), &mut tall_buf);

    let copy_height = area
        .height
        .min(tall_buf.area().height.saturating_sub(scroll_offset));
    for y in 0..copy_height {
        let source_y = y.saturating_add(scroll_offset);
        for x in 0..area.width {
            buf[(area.x + x, area.y + y)] = tall_buf[(x, source_y)].clone();
        }
    }
}
