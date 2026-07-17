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
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use leaf_core::{Doc, InlineKind, View};

use crate::style::{CODE_BG, CODE_BORDER, CODE_INSET, wysiwyg_lines};
use crate::{App, ConflictPrompt, ContextMenu, DirtyAction, DirtyPrompt, MENU_ITEMS, TextPrompt};

pub fn render(f: &mut Frame, doc: &mut Doc, breadcrumb: &str, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(1),    // body
        Constraint::Length(2), // footer
    ])
    .split(f.area());

    render_header(f, chunks[0], doc, breadcrumb);
    render_body(f, chunks[1], doc, app);
    render_footer(f, chunks[2], doc, app.dirty_prompt.as_ref(), app.conflict.as_ref());

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

fn render_body(f: &mut Frame, area: Rect, doc: &mut Doc, app: &mut App) {
    let sel = doc.selection();

    // Reserve the rightmost column for the scrollbar so it doesn't paint over
    // a line's last visible character; everything below reads `content_area`
    // instead of `area` for exactly that reason (the WYSIWYG soft-wrap width,
    // the mouse hit-test geometry, the horizontal source follow).
    let [content_area, scrollbar_area] =
        Layout::horizontal([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let width = content_area.width as usize;
    let height = content_area.height as usize;

    // The WYSIWYG map must be built before we read the caret position (which
    // rides it). Code lines don't wrap — the map keeps them full length — so a
    // long one scrolls inside its box (below) rather than folding.
    if doc.view == View::Wysiwyg {
        doc.build_visual(width);
    }
    let (caret_row, caret_col) = doc.caret_pos();

    // The code block the caret is in, and how far it's scrolled sideways to keep
    // the caret a column clear of the box's right border. A box is only as wide
    // as its widest line (like a table), so the scroll runway is that block's own
    // inner width, not the whole editor's. Every other code block shows from its
    // first column; only this one scrolls. Stashed on `app` so `handle_mouse`
    // can undo the shift on a click.
    let caret_cb = (doc.view == View::Wysiwyg)
        .then(|| doc.vmap.code_blocks.iter().find(|c| c.rows_span.contains(&caret_row)))
        .flatten();
    let caret_span = caret_cb.map(|c| c.rows_span.clone());
    let code_inner_w = caret_cb
        .map(|c| code_box_width(doc, &c.rows_span, width).saturating_sub(CODE_INSET + 1))
        .unwrap_or(0);
    let code_scroll = match &caret_span {
        Some(_) if code_inner_w > 0 && caret_col >= code_inner_w => caret_col + 1 - code_inner_w,
        _ => 0,
    };
    app.code_scroll_x = code_scroll;
    app.code_caret_span = caret_span.clone();

    // Build the view's lines. A code row is drawn inset for its box and, if it's
    // the caret's block, scrolled by `code_scroll`.
    let lines = match doc.view {
        View::Source => build_lines(&doc.source, sel),
        View::Wysiwyg => {
            let code_shift = |r: usize| -> Option<usize> {
                doc.vmap.code_blocks.iter().find(|c| c.rows_span.contains(&r)).map(|c| {
                    if caret_span.as_ref().is_some_and(|s| *s == c.rows_span) {
                        code_scroll
                    } else {
                        0
                    }
                })
            };
            wysiwyg_lines(&doc.vmap, sel, code_shift)
        }
    };
    let line_count = lines.len();
    doc.follow_caret(caret_row, height, line_count);

    // Stash geometry for mouse hit-testing.
    doc.body_origin = (content_area.x, content_area.y);
    doc.body_height = content_area.height;

    // The source view splits on '\n' alone and can run a line past the right
    // edge, so it needs a horizontal follow; the WYSIWYG view scrolls only its
    // code blocks (above), everything else already fits `width`.
    let scroll_x = &mut app.scroll_x;
    match doc.view {
        View::Source => follow_caret_x(scroll_x, caret_col, width),
        View::Wysiwyg => *scroll_x = 0,
    }
    let scroll_x = *scroll_x;

    let para = Paragraph::new(lines).scroll((doc.scroll as u16, scroll_x as u16));
    f.render_widget(para, content_area);

    // Each code block's box: a tinted, bordered panel patched *over* the code
    // rows the paragraph just drew. A `Block` only sets the background and draws
    // its border — it leaves the code glyphs underneath untouched — so the fill
    // slides behind the text and the border sits in the inset column reserved
    // for it. Drawn after the paragraph precisely so the border lands on top of
    // the throwaway edge columns.
    if doc.view == View::Wysiwyg {
        for cb in &doc.vmap.code_blocks {
            let box_w = code_box_width(doc, &cb.rows_span, width);
            if let Some((rect, borders)) =
                code_box(&cb.rows_span, doc.vmap.rows.len(), content_area, doc.scroll, box_w)
            {
                let mut block = Block::default()
                    .borders(borders)
                    .border_style(Style::default().fg(CODE_BORDER).bg(CODE_BG))
                    .style(Style::default().bg(CODE_BG));
                // The language rides the top border as a small label, the way a
                // titled panel names itself — shown only when that border is.
                if let Some(lang) = &cb.lang {
                    if borders.contains(Borders::TOP) {
                        block = block.title(Line::from(Span::styled(
                            format!(" {lang} "),
                            Style::default().fg(Color::Gray).bg(CODE_BG),
                        )));
                    }
                }
                f.render_widget(block, rect);
            }
        }
    }

    // A thumb-only affordance (no `<`/`>` end glyphs — there's no click target
    // for them without a wired-up mouse handler, and a bare thumb over a track
    // is enough to show how much is above/below without implying it's a
    // button). `ScrollbarState`'s content length is the same line count
    // `follow_caret` above was just clamped against, so the two can't disagree
    // about where the bottom of the document is.
    let mut sb_state = ScrollbarState::new(line_count).position(doc.scroll);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None);
    f.render_stateful_widget(scrollbar, scrollbar_area, &mut sb_state);

    // Draw the real terminal caret (only when it's within the viewport). A code
    // row is inset and scrolled, so its caret column is measured from inside the
    // box; every other row measures from the content edge.
    let in_caret_code = caret_span.as_ref().is_some_and(|s| s.contains(&caret_row));
    let caret_x = if in_caret_code {
        let vis = caret_col.saturating_sub(code_scroll);
        (code_inner_w == 0 || vis < code_inner_w)
            .then(|| content_area.x + (CODE_INSET + vis) as u16)
    } else {
        let col_visible = caret_col >= scroll_x && (width == 0 || caret_col < scroll_x + width);
        col_visible.then(|| content_area.x + (caret_col - scroll_x) as u16)
    };
    if let Some(x) = caret_x {
        if caret_row >= doc.scroll && (height == 0 || caret_row < doc.scroll + height) {
            let y = content_area.y + (caret_row - doc.scroll) as u16;
            f.set_cursor_position(Position::new(x, y));
        }
    }
}

/// The width of a code block's box: its widest line plus the two border
/// columns, capped at the content width. Sizes the box to its content the way a
/// table's columns size to their cells, so a short snippet doesn't stretch a
/// bar across the whole editor. A block wider than the surface is capped and
/// scrolls (see the caret follow above).
fn code_box_width(doc: &Doc, span: &std::ops::Range<usize>, avail: usize) -> usize {
    let content = span.clone().map(|r| doc.vmap.row_width(r)).max().unwrap_or(0);
    (content + CODE_INSET + 1).min(avail).max(CODE_INSET + 1)
}

/// The on-screen rectangle and border edges of a code block's box, or `None`
/// when it's scrolled entirely out of view. `span` is the block's code rows;
/// the box grows one row up and one down into the blank separators around it to
/// carry its top and bottom border, and is `box_w` columns wide. A border whose
/// real edge is scrolled past the viewport is dropped rather than drawn as a
/// false rule at the viewport's edge, and a block flush against the document
/// start or end simply has no separator there to border.
fn code_box(
    span: &std::ops::Range<usize>,
    row_count: usize,
    content: Rect,
    scroll: usize,
    box_w: usize,
) -> Option<(Rect, Borders)> {
    let has_top = span.start > 0;
    let has_bottom = span.end < row_count;
    // Box rows, inclusive, in map-row coordinates.
    let top_vr = if has_top { span.start - 1 } else { span.start };
    let bottom_vr = if has_bottom { span.end } else { span.end.saturating_sub(1) };

    // Map-row → screen-y (relative to the content top), as signed so a box above
    // the viewport is caught rather than wrapping around.
    let cy = content.y as i32;
    let y_of = |vr: usize| cy + vr as i32 - scroll as i32;
    let box_top = y_of(top_vr);
    let box_bottom = y_of(bottom_vr); // inclusive
    let view_top = cy;
    let view_bottom = cy + content.height as i32 - 1;

    let vis_top = box_top.max(view_top);
    let vis_bottom = box_bottom.min(view_bottom);
    if vis_bottom < vis_top {
        return None;
    }

    let mut borders = Borders::LEFT | Borders::RIGHT;
    if has_top && box_top >= view_top {
        borders |= Borders::TOP;
    }
    if has_bottom && box_bottom <= view_bottom {
        borders |= Borders::BOTTOM;
    }
    let rect = Rect {
        x: content.x,
        y: vis_top as u16,
        width: (box_w as u16).min(content.width).max(1),
        height: (vis_bottom - vis_top + 1) as u16,
    };
    Some((rect, borders))
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

fn render_footer(
    f: &mut Frame,
    area: Rect,
    doc: &mut Doc,
    dirty_prompt: Option<&DirtyPrompt>,
    conflict: Option<&ConflictPrompt>,
) {
    let dim = Style::default().fg(Color::DarkGray);
    let key = Style::default().fg(Color::Cyan);

    // Both prompts take over the whole footer until answered, so an
    // accidental Ctrl+Q/⌥s/^S on a document with something at stake can't
    // lose or clobber work to a stray keystroke.
    if let Some(prompt) = dirty_prompt {
        let verb = match prompt.action {
            DirtyAction::Quit => "quit",
            DirtyAction::New => "start a new document",
        };
        let lines = choice_prompt_lines(
            &format!("Unsaved changes — {verb}?"),
            &["Save", "Discard", "Cancel"],
            prompt.selected,
            key,
            dim,
        );
        f.render_widget(Paragraph::new(lines), area);
        return;
    }
    if let Some(prompt) = conflict {
        let lines = choice_prompt_lines(
            "File changed on disk since it was opened",
            &["Overwrite", "Reload", "Cancel"],
            prompt.selected,
            key,
            dim,
        );
        f.render_widget(Paragraph::new(lines), area);
        return;
    }

    let line1 = if let Some(msg) = &doc.status {
        Line::from(Span::styled(msg.clone(), Style::default().fg(Color::Yellow)))
    } else {
        format_state_line(doc, key)
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
        Span::styled("⌥v ", key),
        Span::styled("paste plain   ", dim),
        Span::styled("⌥w ", key),
        Span::styled("view   ", dim),
        Span::styled("^s/⌥s ", key),
        Span::styled("save·save as   ", dim),
        Span::styled("^q ", key),
        Span::styled("quit", dim),
    ]);
    f.render_widget(Paragraph::new(vec![line1, line2]), area);
}

/// Line 1's normal (no-status) content: what's active at the caret, read live
/// off `Doc::active_inline_marks`/`current_heading_level` every frame — the
/// same thing a mouse-driven toolbar shows by lighting up a button, as text
/// since there are no buttons here to light. Only what's actually *on* is
/// listed (a status line states facts; a row of every possible mark grayed
/// out beside it is the toolbar this deliberately isn't), so a bare caret in
/// plain body text prints a dim placeholder rather than leaving the line
/// looking broken or blank.
fn format_state_line(doc: &mut Doc, key: Style) -> Line<'static> {
    let heading = doc.current_heading_level();
    let marks = doc.active_inline_marks();
    let mut spans = Vec::new();
    if let Some(level) = heading {
        spans.push(Span::styled(format!("H{level}"), key.add_modifier(Modifier::BOLD)));
    }
    for kind in marks.iter() {
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(mark_label(kind), mark_style(kind)));
    }
    if spans.is_empty() {
        return Line::from(Span::styled("plain text", Style::default().fg(Color::DarkGray)));
    }
    Line::from(spans)
}

/// The word a mark reads as in the footer — the toolbar-button name, not the
/// AST node kind `Doc::toggle`/`active_inline_marks` traffic in.
fn mark_label(kind: InlineKind) -> &'static str {
    match kind {
        InlineKind::Strong => "Bold",
        InlineKind::Emph => "Italic",
        InlineKind::Verbatim => "Code",
        InlineKind::Mark => "Mark",
        InlineKind::Superscript => "Superscript",
        InlineKind::Subscript => "Subscript",
        InlineKind::Insert => "Underline",
        InlineKind::Delete => "Strikethrough",
    }
}

/// Style a mark's footer label the way the WYSIWYG view itself renders that
/// mark (see leaf-core's `wysiwyg::Builder::inline`), so the footer reads as a
/// mirror of the caret's actual formatting instead of an unrelated palette.
fn mark_style(kind: InlineKind) -> Style {
    let base = Style::default();
    match kind {
        InlineKind::Strong => base.add_modifier(Modifier::BOLD),
        InlineKind::Emph => base.add_modifier(Modifier::ITALIC),
        InlineKind::Verbatim => base.fg(Color::Green),
        InlineKind::Mark => base.bg(Color::Yellow).fg(Color::Black),
        InlineKind::Insert => base.add_modifier(Modifier::UNDERLINED),
        InlineKind::Delete => base.add_modifier(Modifier::CROSSED_OUT),
        InlineKind::Superscript | InlineKind::Subscript => base.fg(Color::Cyan),
    }
}

/// Shared two-line rendering for `dirty_prompt` and `conflict`: a warning
/// line naming what's at stake, then `items` laid out with `selected`
/// reversed — arrow-key highlighted the same way the context menu is, plus a
/// first-letter mnemonic per item (the caller's key handling and this must
/// agree on what those letters are; there's only three items either prompt
/// ever has, so they're spelled out in the label rather than derived).
fn choice_prompt_lines(message: &str, items: &[&str], selected: usize, key: Style, dim: Style) -> Vec<Line<'static>> {
    let warn = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    for (i, label) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", dim));
        }
        let style = if i == selected { key.add_modifier(Modifier::REVERSED) } else { key };
        let mnemonic = label.chars().next().unwrap_or(' ').to_ascii_lowercase();
        spans.push(Span::styled(format!(" {label} ({mnemonic}) "), style));
    }
    vec![Line::from(Span::styled(message.to_string(), warn)), Line::from(spans)]
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

#[cfg(test)]
mod code_render_tests {
    use super::*;
    use crate::App;
    use leaf_core::Doc;
    use ratatui::{Terminal, backend::TestBackend};

    fn render_to_lines(name: &str, src: &str, w: u16, h: u16) -> Vec<String> {
        let mut p = std::env::temp_dir();
        p.push(format!("leaf_code_render_{name}.md"));
        std::fs::write(&p, src).unwrap();
        let mut doc = Doc::open(p).unwrap();
        let mut app = App::default();
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render(f, &mut doc, "", &mut app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()
            })
            .collect()
    }

    #[test]
    fn a_code_block_draws_a_content_width_box_with_its_language_label() {
        let src = "text\n\n```rust\nlet x = 1;\n```\n\nafter\n";
        let lines = render_to_lines("labeled", src, 40, 12);
        let joined = lines.join("\n");
        // The box is bordered and carries the language on its top edge.
        assert!(joined.contains("rust"), "language label missing:\n{joined}");
        assert!(lines.iter().any(|l| l.contains('┌') && l.contains('┐')), "no top border:\n{joined}");
        assert!(lines.iter().any(|l| l.contains('└') && l.contains('┘')), "no bottom border:\n{joined}");
        // Content width, not full width: the `let x = 1;` box is far short of 40.
        let top = lines.iter().find(|l| l.contains('┌')).unwrap();
        let border_cols = top.chars().filter(|&c| c == '─' || c == '┌' || c == '┐').count();
        assert!(border_cols < 30, "box should hug its content, got {border_cols} border cols:\n{joined}");
        // No leftover code gutter, and the code itself is inside the box.
        assert!(!joined.contains('▏'), "old gutter still drawn:\n{joined}");
        assert!(joined.contains("let x = 1;"), "code text missing:\n{joined}");
    }

    #[test]
    fn a_bare_fence_gets_a_box_but_no_label() {
        let src = "text\n\n```\nplain code\n```\n\nafter\n";
        let lines = render_to_lines("bare", src, 40, 12);
        let joined = lines.join("\n");
        assert!(lines.iter().any(|l| l.contains('┌')), "no box:\n{joined}");
        assert!(joined.contains("plain code"), "code missing:\n{joined}");
    }
}
