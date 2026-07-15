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
    widgets::{Clear, Paragraph},
};

use leaf_core::{Doc, View};

use crate::style::wysiwyg_lines;
use crate::{App, ContextMenu, MENU_ITEMS, TextPrompt};

pub fn render(f: &mut Frame, doc: &mut Doc, breadcrumb: &str, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(1),    // body
        Constraint::Length(2), // footer
    ])
    .split(f.area());

    render_header(f, chunks[0], doc, breadcrumb);
    render_body(f, chunks[1], doc, &mut app.scroll_x);
    render_footer(f, chunks[2], doc, app.confirm_quit);

    if let Some(menu) = &mut app.context_menu {
        render_context_menu(f, f.area(), menu);
    }
    if let Some(prompt) = &app.text_prompt {
        render_text_prompt(f, f.area(), prompt);
    }
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

fn render_body(f: &mut Frame, area: Rect, doc: &mut Doc, scroll_x: &mut usize) {
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
    doc.follow_caret(caret_row, height, lines.len());

    // Stash geometry for mouse hit-testing.
    doc.body_origin = (area.x, area.y);
    doc.body_height = area.height;

    // The WYSIWYG view already soft-wraps at `area.width` (`build_visual`
    // above), so every column it produces is on screen. Only the source view
    // splits on '\n' alone and can run a line past the right edge, so it's the
    // only one that needs a horizontal follow — the vertical one `doc.scroll`
    // gets from `follow_caret`, but kept in the frontend since it doesn't
    // affect the row/col ↔ offset mapping leaf-core owns.
    let width = area.width as usize;
    match doc.view {
        View::Source => follow_caret_x(scroll_x, caret_col, width),
        View::Wysiwyg => *scroll_x = 0,
    }

    let para = Paragraph::new(lines).scroll((doc.scroll as u16, *scroll_x as u16));
    f.render_widget(para, area);

    // Draw the real terminal caret (only when it's within the viewport).
    let col_visible = caret_col >= *scroll_x && (width == 0 || caret_col < *scroll_x + width);
    if col_visible && caret_row >= doc.scroll && (height == 0 || caret_row < doc.scroll + height) {
        let x = area.x + (caret_col - *scroll_x) as u16;
        let y = area.y + (caret_row - doc.scroll) as u16;
        f.set_cursor_position(Position::new(x, y));
    }
}

/// Horizontal analogue of `Doc::follow_caret`: keeps the caret's column
/// on screen in the source view. Unlike the vertical axis there's no
/// horizontal scroll wheel to fight — nothing else ever moves `scroll_x` — so
/// this can just chase the caret on every frame instead of only on caret
/// moves.
fn follow_caret_x(scroll_x: &mut usize, caret_col: usize, width: usize) {
    if width == 0 {
        return;
    }
    if caret_col < *scroll_x {
        *scroll_x = caret_col;
    } else if caret_col >= *scroll_x + width {
        *scroll_x = caret_col + 1 - width;
    }
}

fn render_footer(f: &mut Frame, area: Rect, doc: &Doc, confirm_quit: bool) {
    let dim = Style::default().fg(Color::DarkGray);
    let key = Style::default().fg(Color::Cyan);

    // The quit-confirmation prompt takes over both footer lines until the
    // user answers, so an accidental Ctrl+Q on a dirty document can't lose
    // work to a stray keystroke.
    if confirm_quit {
        let warn = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
        let line1 = Line::from(Span::styled(
            "Unsaved changes — quit without saving?",
            warn,
        ));
        let line2 = Line::from(vec![
            Span::styled("y ", key),
            Span::styled("quit   ", dim),
            Span::styled("n/esc ", key),
            Span::styled("cancel", dim),
        ]);
        f.render_widget(Paragraph::new(vec![line1, line2]), area);
        return;
    }

    let line1 = if let Some(msg) = &doc.status {
        Line::from(Span::styled(msg.clone(), Style::default().fg(Color::Yellow)))
    } else {
        Line::from(vec![
            Span::styled("⌥b/i/c ", key),
            Span::styled("bold·italic·code   ", dim),
            Span::styled("⌥m ", key),
            Span::styled("mark   ", dim),
            Span::styled("⌥1-6/0 ", key),
            Span::styled("heading/body   ", dim),
            Span::styled("⌥7/8/9 ", key),
            Span::styled("numbered·bulleted·quote   ", dim),
            Span::styled("⌥k ", key),
            Span::styled("link", dim),
        ])
    };
    let line2 = Line::from(vec![
        Span::styled("type ", key),
        Span::styled("to edit   ", dim),
        Span::styled("⇧+move ", key),
        Span::styled("select   ", dim),
        Span::styled("⌥←/→/⌫/⌦ ", key),
        Span::styled("word   ", dim),
        Span::styled("^a/c/x/v ", key),
        Span::styled("all·copy·cut·paste   ", dim),
        Span::styled("⌥w ", key),
        Span::styled("view   ", dim),
        Span::styled("^s ", key),
        Span::styled("save   ", dim),
        Span::styled("^q ", key),
        Span::styled("quit", dim),
    ]);
    f.render_widget(Paragraph::new(vec![line1, line2]), area);
}

/// The right-click menu: Cut / Copy / Paste / Select All, anchored at the
/// click and nudged back onto `screen` if it wouldn't otherwise fit (the
/// terminal equivalent of the GUI menu's `snap_to_window`). Stashes the rect
/// it painted at back onto `menu` so `main::menu_item_at` can hit-test clicks
/// against the exact geometry drawn here, the same way `doc.body_origin`
/// carries the body's geometry back out to `handle_mouse`.
fn render_context_menu(f: &mut Frame, screen: Rect, menu: &mut ContextMenu) {
    let width = MENU_ITEMS.iter().map(|(label, _)| label.len()).max().unwrap_or(0) as u16 + 4;
    let height = MENU_ITEMS.len() as u16;
    let (anchor_x, anchor_y) = menu.anchor;
    let x = anchor_x.min(screen.width.saturating_sub(width));
    let y = anchor_y.min(screen.height.saturating_sub(height));
    let rect = Rect { x, y, width, height };
    menu.rect = Some(rect);

    let base = Style::default().bg(Color::DarkGray).fg(Color::White);
    let lines: Vec<Line<'static>> = MENU_ITEMS
        .iter()
        .enumerate()
        .map(|(i, (label, _))| {
            let style = if i == menu.selected {
                base.add_modifier(Modifier::REVERSED)
            } else {
                base
            };
            Line::from(Span::styled(format!(" {label:<pad$}", pad = width as usize - 1), style))
        })
        .collect();

    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).style(base), rect);
}

/// The single-line input: a label row, a value row, and an Enter/Esc hint,
/// centered over `screen` — there's no click anchor to hang it off the way
/// the context menu has, and nothing in it is clickable, so unlike that menu
/// this stashes no rect back for hit-testing. The caret is the real terminal
/// cursor, positioned into the value row exactly the way the document body
/// positions it into the source — one visible caret, one mechanism.
fn render_text_prompt(f: &mut Frame, screen: Rect, prompt: &TextPrompt) {
    let hint = " enter confirm  esc cancel ";
    let content = [prompt.label.chars().count(), prompt.value.chars().count(), hint.chars().count()]
        .into_iter()
        .max()
        .unwrap_or(0) as u16
        + 2;
    let width = content.max(24).min(screen.width.max(1));
    let height = 3u16.min(screen.height.max(1));
    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 2;
    let rect = Rect { x, y, width, height };

    let base = Style::default().bg(Color::DarkGray).fg(Color::White);
    let bold = base.add_modifier(Modifier::BOLD);
    let key = base.fg(Color::Cyan);
    let dim = base.fg(Color::Gray);
    let lines = vec![
        Line::from(Span::styled(format!(" {} ", prompt.label), bold)),
        Line::from(Span::styled(format!(" {} ", prompt.value), base)),
        Line::from(vec![
            Span::styled(" enter ", key),
            Span::styled("confirm  ", dim),
            Span::styled("esc ", key),
            Span::styled("cancel ", dim),
        ]),
    ];

    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).style(base), rect);

    let cursor_x = rect.x + 1 + prompt.value[..prompt.cursor].chars().count() as u16;
    if rect.height >= 2 && cursor_x < rect.x + rect.width {
        f.set_cursor_position(Position::new(cursor_x, rect.y + 1));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_caret_x_scrolls_right_just_far_enough_to_reveal_the_caret() {
        let mut scroll_x = 0;
        follow_caret_x(&mut scroll_x, 50, 20);
        assert_eq!(scroll_x, 31); // caret_col + 1 - width
    }

    #[test]
    fn follow_caret_x_scrolls_left_when_the_caret_moves_before_the_offset() {
        let mut scroll_x = 30;
        follow_caret_x(&mut scroll_x, 5, 20);
        assert_eq!(scroll_x, 5);
    }

    #[test]
    fn follow_caret_x_leaves_scroll_alone_when_the_caret_is_already_visible() {
        let mut scroll_x = 10;
        follow_caret_x(&mut scroll_x, 15, 20);
        assert_eq!(scroll_x, 10);
    }
}
