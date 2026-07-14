//! Rendering: a source view with the selection highlighted, a real terminal
//! caret, a live AST breadcrumb in the header, and a toolbar-hint footer.
//!
//! This is the *source* view — the honest first milestone. A WYSIWYG view that
//! hides the markup (mdfried-style rasterized headings, inline images) is the
//! next step, and its hard part is mapping the caret between rendered and
//! source coordinates; the byte-offset model here is what that maps onto.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use leaf_core::{Doc, View};

use crate::style::wysiwyg_lines;

pub fn render(f: &mut Frame, doc: &mut Doc, breadcrumb: &str) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(1),    // body
        Constraint::Length(2), // footer
    ])
    .split(f.area());

    render_header(f, chunks[0], doc, breadcrumb);
    render_body(f, chunks[1], doc);
    render_footer(f, chunks[2], doc);
}

fn render_header(f: &mut Frame, area: Rect, doc: &Doc, breadcrumb: &str) {
    let dim = Style::default().fg(Color::DarkGray);
    let mut spans = vec![
        Span::styled(
            "leaf ",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        ),
        Span::styled("▸ ", dim),
        Span::raw(doc.file_name()),
        Span::styled(format!("  [{}]", doc.format_name()), dim),
    ];
    if doc.dirty {
        spans.push(Span::styled(
            "  ● modified",
            Style::default().fg(Color::Yellow),
        ));
    }
    spans.push(Span::styled(format!("  ⌥w {}", doc.view_name()), dim));
    if !breadcrumb.is_empty() {
        spans.push(Span::styled(format!("   {breadcrumb}"), Style::default().fg(Color::Cyan)));
    }
    f.render_widget(Line::from(spans), area);
}

fn render_body(f: &mut Frame, area: Rect, doc: &mut Doc) {
    let sel = doc.selection();

    // Build the view's lines. The WYSIWYG map must be built before we read the
    // caret position (which rides it).
    let lines = match doc.view {
        View::Source => build_lines(&doc.source, sel),
        View::Wysiwyg => {
            doc.build_visual(area.width as usize);
            wysiwyg_lines(&doc.vmap, sel)
        }
    };
    let (caret_row, caret_col) = doc.caret_pos();
    let height = area.height as usize;

    // Keep the caret row on screen.
    if caret_row < doc.scroll {
        doc.scroll = caret_row;
    } else if height > 0 && caret_row >= doc.scroll + height {
        doc.scroll = caret_row + 1 - height;
    }

    // Stash geometry for mouse hit-testing.
    doc.body_origin = (area.x, area.y);
    doc.body_height = area.height;

    let para = Paragraph::new(lines).scroll((doc.scroll as u16, 0));
    f.render_widget(para, area);

    // Draw the real terminal caret (only when it's within the viewport).
    if caret_row >= doc.scroll && (height == 0 || caret_row < doc.scroll + height) {
        let x = area.x + caret_col as u16;
        let y = area.y + (caret_row - doc.scroll) as u16;
        f.set_cursor_position(Position::new(x, y));
    }
}

fn render_footer(f: &mut Frame, area: Rect, doc: &Doc) {
    let dim = Style::default().fg(Color::DarkGray);
    let key = Style::default().fg(Color::Cyan);

    let line1 = if let Some(msg) = &doc.status {
        Line::from(Span::styled(msg.clone(), Style::default().fg(Color::Yellow)))
    } else {
        Line::from(vec![
            Span::styled("⌥b/i/c ", key),
            Span::styled("bold·italic·code   ", dim),
            Span::styled("⌥m ", key),
            Span::styled("mark   ", dim),
            Span::styled("⌥1-6/0 ", key),
            Span::styled("heading/body", dim),
        ])
    };
    let line2 = Line::from(vec![
        Span::styled("type ", key),
        Span::styled("to edit   ", dim),
        Span::styled("⇧+move ", key),
        Span::styled("select   ", dim),
        Span::styled("⌥w ", key),
        Span::styled("view   ", dim),
        Span::styled("^s ", key),
        Span::styled("save   ", dim),
        Span::styled("^q ", key),
        Span::styled("quit", dim),
    ]);
    f.render_widget(Paragraph::new(vec![line1, line2]), area);
}

/// Split `source` into styled lines, drawing any part of the `[start, end)`
/// selection reversed.
fn build_lines(source: &str, sel: Option<(usize, usize)>) -> Vec<Line<'static>> {
    let hl = Style::default().add_modifier(Modifier::REVERSED);
    let (sel_start, sel_end) = sel.unwrap_or((0, 0));
    let mut lines = Vec::new();
    let mut byte = 0usize;

    for raw in source.split('\n') {
        let line_start = byte;
        let line_end = line_start + raw.len();

        // Overlap of the selection with this line, in line-local byte coords.
        let a = sel_start.clamp(line_start, line_end) - line_start;
        let b = sel_end.clamp(line_start, line_end) - line_start;

        let mut spans = Vec::new();
        if a < b {
            push(&mut spans, &raw[..a], Style::default());
            push(&mut spans, &raw[a..b], hl);
            push(&mut spans, &raw[b..], Style::default());
        } else {
            push(&mut spans, raw, Style::default());
        }
        if spans.is_empty() {
            spans.push(Span::raw(""));
        }
        lines.push(Line::from(spans));
        byte = line_end + 1; // skip the '\n' that `split` consumed
    }
    lines
}

fn push(spans: &mut Vec<Span<'static>>, text: &str, style: Style) {
    if !text.is_empty() {
        spans.push(Span::styled(text.to_string(), style));
    }
}
