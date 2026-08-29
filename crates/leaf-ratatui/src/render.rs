//! Rendering the editing surface: the document body (WYSIWYG or source) with the
//! selection highlighted, code blocks boxed, block images framed and rasterized,
//! a vertical scrollbar, and the real terminal caret. The host draws its own
//! chrome (header/footer/dialogs) around the `Rect` this fills.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
// Only the block-image loop wipes cells before framing a picture.
#[cfg(feature = "images")]
use ratatui::widgets::Clear;

use leaf_core::{Doc, Highlight, HighlightCursor, View};

use crate::EditorState;
use crate::style::{CODE_INSET, Theme, composed, wysiwyg_lines};

/// Render the editing surface into `area`: the document body, its code-block
/// boxes and framed images, the scrollbar, and the terminal caret. Updates
/// `state`'s scroll bookkeeping so [`crate::handle_mouse`] can map a later click
/// back to a source byte.
pub fn render(f: &mut Frame, area: Rect, doc: &mut Doc, state: &mut EditorState) {
    let sel = doc.selection();
    // The palette, copied out up front: the code boxes and image frames below
    // read it while `state` is borrowed mutably for the image cache.
    let theme = *state.theme();

    // Reserve the rightmost column for the scrollbar so it doesn't paint over
    // a line's last visible character; everything below reads `content_area`
    // instead of `area` for exactly that reason (the WYSIWYG soft-wrap width,
    // the mouse hit-test geometry, the horizontal source follow).
    let [content_area, scrollbar_area] =
        Layout::horizontal([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let width = content_area.width as usize;
    let height = content_area.height as usize;

    // The document's directory — what a relative image path resolves against.
    // `Doc::open` stores an absolute path, so this is set for any real file and
    // empty only for an untitled buffer (where a relative image can't resolve).
    #[cfg(feature = "images")]
    let doc_dir = doc
        .path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf());

    // The WYSIWYG map must be built before we read the caret position (which
    // rides it). Code lines don't wrap — the map keeps them full length — so a
    // long one scrolls inside its box (below) rather than folding.
    if doc.view == View::Wysiwyg {
        doc.build_visual(width);
        // With image support: learn which images the document has, decode and
        // measure them, tell core how many rows each reserves, then rebuild at
        // those heights. The second build is a cache hit whenever nothing changed.
        // Without the feature, block images keep core's default reservation and
        // render as the inline `🖼 alt` placeholder — no decode, no rebuild.
        #[cfg(feature = "images")]
        {
            let heights = state.images.reserve(
                &doc.vmap.media,
                doc_dir.as_deref(),
                width as u16,
                height as u16,
            );
            doc.set_media_rows(heights);
            doc.build_visual(width);
        }
    }
    let (caret_row, caret_col) = doc.caret_pos();

    // The code block the caret is in, and how far it's scrolled sideways to keep
    // the caret a column clear of the box's right border. A box is only as wide
    // as its widest line (like a table), so the scroll runway is that block's own
    // inner width, not the whole editor's. Every other code block shows from its
    // first column; only this one scrolls. Stashed on `state` so `handle_mouse`
    // can undo the shift on a click.
    let caret_cb = (doc.view == View::Wysiwyg)
        .then(|| {
            doc.vmap
                .code_blocks
                .iter()
                .find(|c| c.rows_span.contains(&caret_row))
        })
        .flatten();
    let caret_span = caret_cb.map(|c| c.rows_span.clone());
    let code_inner_w = caret_cb
        .map(|c| code_box_width(doc, &c.rows_span, width).saturating_sub(CODE_INSET + 1))
        .unwrap_or(0);
    let code_scroll = match &caret_span {
        Some(_) if code_inner_w > 0 && caret_col >= code_inner_w => caret_col + 1 - code_inner_w,
        _ => 0,
    };
    state.code_scroll_x = code_scroll;
    state.code_caret_span = caret_span.clone();

    // The host-painted ranges (search hits, an annotation layer). Both builders
    // take them by shared reference, alongside the shared borrows of
    // `doc.source` and `doc.vmap` they already take, so this is the list itself
    // rather than a copy of it made every frame.
    let highlights = doc.highlights();

    // Build the view's lines. A code row is drawn inset for its box and, if it's
    // the caret's block, scrolled by `code_scroll`.
    let lines = match doc.view {
        View::Source => build_lines(&doc.source, sel, highlights, &theme),
        View::Wysiwyg => {
            let code_shift = |r: usize| -> Option<usize> {
                doc.vmap
                    .code_blocks
                    .iter()
                    .find(|c| c.rows_span.contains(&r))
                    .map(|c| {
                        if caret_span.as_ref().is_some_and(|s| *s == c.rows_span) {
                            code_scroll
                        } else {
                            0
                        }
                    })
            };
            wysiwyg_lines(&doc.vmap, sel, highlights, &theme, code_shift)
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
    let scroll_x = &mut state.scroll_x;
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
            if let Some((rect, borders)) = code_box(
                &cb.rows_span,
                doc.vmap.rows.len(),
                content_area,
                doc.scroll,
                box_w,
            ) {
                let mut block = Block::default()
                    .borders(borders)
                    .border_style(Style::default().fg(theme.code_border).bg(theme.code_bg))
                    .style(Style::default().bg(theme.code_bg));
                // The language rides the top border as a small label, the way a
                // titled panel names itself — shown only when that border is.
                if let Some(lang) = &cb.lang
                    && borders.contains(Borders::TOP)
                {
                    block = block.title(Line::from(Span::styled(
                        format!(" {lang} "),
                        Style::default().fg(theme.code_label).bg(theme.code_bg),
                    )));
                }
                f.render_widget(block, rect);
            }
        }
    }

    // Each block image: a bordered box around the rows core reserved for it, the
    // raster painted inside when the whole box is on screen. The border is drawn
    // from cells, so it clips cleanly at the viewport edge and stands in as a
    // "picture goes here" placeholder whenever the graphics-protocol raster can't
    // be shown — a remote/unresolved image, or one only partly scrolled into view
    // (a protocol image can't be clipped; see `Images::paint_raster`). Drawn after
    // the paragraph so it covers the `🖼 alt` text core laid down underneath.
    // Only with the `images` feature; without it this loop is gone and core's
    // inline `🖼 alt` text (drawn by the paragraph) is the placeholder.
    #[cfg(feature = "images")]
    if doc.view == View::Wysiwyg {
        for info in &doc.vmap.media {
            let span = &info.rows_span;
            // The picture's cell size when it loaded; `None` for an image that
            // isn't a loadable local file — still framed, just as an empty box.
            let picture = state.images.picture_cells(info, doc_dir.as_deref());
            let box_w = match picture {
                Some((cols, _)) => cols as usize + 2,
                None => (info.alt.chars().count() + 4).clamp(CODE_INSET + 2, width),
            };
            let Some((rect, borders)) =
                code_box(span, doc.vmap.rows.len(), content_area, doc.scroll, box_w)
            else {
                continue;
            };
            // The frame, captioned with the alt text the way a code box is
            // captioned with its language (only where the top border is drawn).
            let mut block = Block::default()
                .borders(borders)
                .border_style(Style::default().fg(theme.image_border));
            if borders.contains(Borders::TOP) {
                let caption = if info.alt.is_empty() {
                    " 🖼 image ".to_string()
                } else {
                    format!(" 🖼 {} ", info.alt)
                };
                block = block.title(Line::from(Span::styled(
                    caption,
                    Style::default().fg(theme.image_border),
                )));
            }
            // Wipe the interior so core's `🖼 alt` text (drawn by the paragraph)
            // doesn't show through the frame — the caption already names it, and a
            // painted raster or a bare placeholder box is what belongs inside.
            let inner = block.inner(rect);
            f.render_widget(Clear, inner);
            f.render_widget(block, rect);

            // Paint the raster only when the whole reserved span is on screen, so
            // its size is fixed (see `Images::paint_raster`). Its rows sit inside
            // the box's side borders, at the box's own vertical position; anything
            // less leaves the empty framed box as the placeholder.
            let fully_visible = span.start >= doc.scroll && span.end <= doc.scroll + height;
            if fully_visible && let Some((cols, rows)) = picture {
                let interior = Rect {
                    x: rect.x + 1,
                    y: content_area.y + (span.start - doc.scroll) as u16,
                    width: cols.min(rect.width.saturating_sub(2)),
                    height: rows.min(rect.height),
                };
                if interior.width > 0 && interior.height > 0 {
                    state
                        .images
                        .paint_raster(f, info, doc_dir.as_deref(), interior);
                }
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
    if let Some(x) = caret_x
        && caret_row >= doc.scroll
        && (height == 0 || caret_row < doc.scroll + height)
    {
        let y = content_area.y + (caret_row - doc.scroll) as u16;
        f.set_cursor_position(Position::new(x, y));
    }
}

/// The width of a code block's box: its widest line plus the two border
/// columns, capped at the content width. Sizes the box to its content the way a
/// table's columns size to their cells, so a short snippet doesn't stretch a
/// bar across the whole editor. A block wider than the surface is capped and
/// scrolls (see the caret follow above).
fn code_box_width(doc: &Doc, span: &std::ops::Range<usize>, avail: usize) -> usize {
    let content = span
        .clone()
        .map(|r| doc.vmap.row_width(r))
        .max()
        .unwrap_or(0);
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
    let bottom_vr = if has_bottom {
        span.end
    } else {
        span.end.saturating_sub(1)
    };

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

/// Split `source` into styled lines, drawing any part of the `[start, end)`
/// selection reversed.
fn build_lines(
    source: &str,
    sel: Option<(usize, usize)>,
    highlights: &[Highlight],
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut byte = 0usize;
    // Reused across lines rather than allocated per line: a document is a few
    // thousand of them and this is rebuilt every frame.
    let mut cuts: Vec<usize> = Vec::new();
    // The runs are visited in ascending source order, so one cursor answers the
    // whole view — see `HighlightCursor`.
    let mut covering = HighlightCursor::new(highlights);

    for raw in source.split('\n') {
        let line_start = byte;
        let line_end = line_start + raw.len();

        // Where the styling can change within this line: its two ends, plus
        // every selection and highlight edge falling inside it, in line-local
        // byte coordinates. This used to be a three-way split around the
        // selection alone, which had nowhere to put a second overlapping range
        // — and search hits are exactly that.
        cuts.clear();
        cuts.push(0);
        cuts.push(raw.len());
        let mut cut = |at: usize| {
            if at > line_start && at < line_end {
                cuts.push(at - line_start);
            }
        };
        if let Some((s, e)) = sel {
            cut(s);
            cut(e);
        }
        for h in highlights {
            cut(h.start);
            cut(h.end);
        }
        cuts.sort_unstable();
        cuts.dedup();

        let mut spans = Vec::new();
        for pair in cuts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            // Styled by what covers the run's first byte: no edge falls strictly
            // inside a run, by construction, so one probe answers for all of it.
            push(
                &mut spans,
                &raw[a..b],
                composed(Style::default(), line_start + a, sel, &mut covering, theme),
            );
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
    use ratatui::style::Modifier;

    /// The source view's own painter, which used to split each line three ways
    /// around the selection and had nowhere to put a second, overlapping range.
    #[test]
    fn the_source_view_paints_highlights_and_the_selection_together() {
        let theme = Theme::dark();
        let painted = vec![Highlight {
            start: 4,
            end: 7,
            id: "hit".into(),
            color: None,
            marker: None,
        }];
        // Selection over the second half of the highlight, so the line has to
        // come apart into four runs rather than three.
        let lines = build_lines("one two three", Some((6, 9)), &painted, &theme);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "one two three", "the line still reads whole");

        let washed: String = lines[0]
            .spans
            .iter()
            .filter(|s| s.style.bg == Some(theme.highlight_bg))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(washed, "two");
        let reversed: String = lines[0]
            .spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(reversed, "o t");
    }

    /// Highlight ranges are document-wide byte offsets, so a painter that
    /// forgot to clamp them per line would wash the wrong words further down.
    #[test]
    fn a_highlight_lands_on_the_line_it_actually_covers() {
        let theme = Theme::dark();
        let source = "alpha\nbravo\ncharlie";
        let painted = vec![Highlight {
            start: 6,
            end: 11, // "bravo", on the second line
            id: "hit".into(),
            color: None,
            marker: None,
        }];
        let lines = build_lines(source, None, &painted, &theme);
        let washed = |i: usize| -> String {
            lines[i]
                .spans
                .iter()
                .filter(|s| s.style.bg == Some(theme.highlight_bg))
                .map(|s| s.content.as_ref())
                .collect()
        };
        assert_eq!(washed(0), "");
        assert_eq!(washed(1), "bravo");
        assert_eq!(washed(2), "");
    }

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
    use crate::EditorState;
    use crate::style::Theme;
    use leaf_core::{ColorScheme, Doc};
    use ratatui::{Terminal, backend::TestBackend};

    /// Draw `src` into an off-screen buffer of `w`×`h`. `scheme` pins the
    /// palette so a test doesn't inherit the developer's own `COLORFGBG`.
    fn render_to_buffer(
        name: &str,
        src: &str,
        w: u16,
        h: u16,
        scheme: ColorScheme,
    ) -> ratatui::buffer::Buffer {
        let mut p = std::env::temp_dir();
        p.push(format!("leaf_ratatui_code_render_{name}.md"));
        std::fs::write(&p, src).unwrap();
        let mut doc = Doc::open(p).unwrap();
        let mut state = EditorState::new();
        state.set_color_scheme(scheme);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render(f, f.area(), &mut doc, &mut state))
            .unwrap();
        term.backend().buffer().clone()
    }

    fn render_to_lines(name: &str, src: &str, w: u16, h: u16) -> Vec<String> {
        let buf = render_to_buffer(name, src, w, h, ColorScheme::Dark);
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
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
        assert!(
            lines.iter().any(|l| l.contains('┌') && l.contains('┐')),
            "no top border:\n{joined}"
        );
        assert!(
            lines.iter().any(|l| l.contains('└') && l.contains('┘')),
            "no bottom border:\n{joined}"
        );
        // Content width, not full width: the `let x = 1;` box is far short of 40.
        let top = lines.iter().find(|l| l.contains('┌')).unwrap();
        let border_cols = top
            .chars()
            .filter(|&c| c == '─' || c == '┌' || c == '┐')
            .count();
        assert!(
            border_cols < 30,
            "box should hug its content, got {border_cols} border cols:\n{joined}"
        );
        // No leftover code gutter, and the code itself is inside the box.
        assert!(!joined.contains('▏'), "old gutter still drawn:\n{joined}");
        assert!(
            joined.contains("let x = 1;"),
            "code text missing:\n{joined}"
        );
    }

    #[test]
    fn a_bare_fence_gets_a_box_but_no_label() {
        let src = "text\n\n```\nplain code\n```\n\nafter\n";
        let lines = render_to_lines("bare", src, 40, 12);
        let joined = lines.join("\n");
        assert!(lines.iter().any(|l| l.contains('┌')), "no box:\n{joined}");
        assert!(joined.contains("plain code"), "code missing:\n{joined}");
    }

    /// The reported bug: a code block was filled with a fixed near-black grey
    /// whatever the terminal looked like, so on a light terminal it landed as a
    /// dark slab across the page. Each scheme must fill its box — every cell of
    /// it, border and label included — with *its own* tint.
    #[test]
    fn a_code_box_is_filled_with_the_active_schemes_tint() {
        for scheme in [ColorScheme::Dark, ColorScheme::Light] {
            let expected = Theme::for_scheme(scheme).code_bg;
            let name = format!("tint_{scheme:?}");
            let buf = render_to_buffer(
                &name,
                "text\n\n```rust\nlet x = 1;\n```\n\nafter\n",
                40,
                12,
                scheme,
            );
            // The box's rows: the top border down to the bottom border.
            let row_of = |ch: char| {
                (0..buf.area.height)
                    .find(|&y| (0..buf.area.width).any(|x| buf[(x, y)].symbol() == ch.to_string()))
                    .unwrap_or_else(|| panic!("no {ch} drawn for {scheme:?}"))
            };
            let (top, bottom) = (row_of('┌'), row_of('└'));
            assert!(top < bottom, "box rows inverted for {scheme:?}");
            for y in top..=bottom {
                for x in 0..buf.area.width {
                    let cell = &buf[(x, y)];
                    // Only the box itself is tinted; it hugs its content, so the
                    // page to its right keeps the terminal's own background.
                    if cell.bg == ratatui::style::Color::Reset {
                        continue;
                    }
                    assert_eq!(
                        cell.bg,
                        expected,
                        "{scheme:?}: cell ({x},{y}) {:?} is filled {:?}, not the scheme's {expected:?}",
                        cell.symbol(),
                        cell.bg
                    );
                }
            }
        }
    }

    /// …and the two schemes really do differ, so the test above can't pass by
    /// painting the same slab twice.
    #[test]
    fn the_two_schemes_paint_different_code_fills() {
        assert_ne!(Theme::dark().code_bg, Theme::light().code_bg);
        assert_ne!(Theme::dark().code_border, Theme::light().code_border);
    }
}
