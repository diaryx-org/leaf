//! Rendering the editing surface: the document body (WYSIWYG or source) with the
//! selection highlighted, code blocks boxed, block images framed and rasterized,
//! a vertical scrollbar, and the real terminal caret. The host draws its own
//! chrome (header/footer/dialogs) around the `Rect` this fills.

use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
// Only the block-image loop wipes cells before framing a picture.
#[cfg(feature = "images")]
use ratatui::widgets::Clear;

#[cfg(feature = "images")]
use leaf_core::VisualMap;
use leaf_core::{Doc, Highlight, HighlightCursor, SourceMap, View};

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
    let scrollbar_width = u16::from(area.width > 0);
    let available_width = area.width.saturating_sub(scrollbar_width);
    let content_width = state
        .line_width
        .unwrap_or(available_width)
        .min(available_width);
    let content_area = Rect {
        x: area.x + available_width.saturating_sub(content_width) / 2,
        width: content_width,
        ..area
    };
    let scrollbar_area = Rect {
        x: area.x + available_width,
        width: scrollbar_width,
        ..area
    };
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
    if doc.view == View::Source {
        // The source view's peer of `build_visual`: the styling for raw markup.
        // Cached on the revision, so this is a no-op on every frame that isn't
        // the first after an edit.
        doc.build_source();
    }
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

    // Oversized headings are a presentation of core's ordinary editable rows.
    // Always restore the canonical map first: the previous frame may have added
    // blank rows beneath inactive H1/H2 blocks, and caret motion must be able to
    // collapse the heading it just entered without changing the document.
    //
    // Only on a terminal with a real graphics protocol, though. The composed
    // heading is text we rasterized; without kitty/iTerm2/sixel ratatui-image
    // would paint it as unicode half-blocks — a blocky picture of letters, worse
    // than the letters themselves. So there we reserve no filler rows and take
    // no raster, and the heading stays the ordinary bold coloured terminal text
    // the theme already gives it.
    // Last frame's painted rasters are stale the moment a new frame starts;
    // the paint loop below re-publishes the ones it actually paints.
    #[cfg(feature = "images")]
    state.heading_rasters.clear();
    #[cfg(feature = "images")]
    let heading_rasters = if doc.view == View::Wysiwyg && state.images.supports_graphics() {
        let key = (doc.revision(), width);
        if let Some((revision, cached_width, base)) = &state.heading_base
            && (*revision, *cached_width) == key
        {
            doc.vmap = base.clone();
        } else {
            state.heading_base = Some((key.0, key.1, doc.vmap.clone()));
        }
        let active_row = doc.vmap.pos_of_offset(doc.caret).0;
        let images = &mut state.images;
        expand_headings(
            &mut doc.vmap,
            doc.caret,
            active_row,
            sel,
            |text, l, rows| images.heading_fits(text, l, (width as u16, rows)),
        )
    } else {
        // Nothing to expand this frame. A base left from a frame that did expand
        // still has to be put back, or its filler rows would outlive the reason
        // for them — a document rebuilt since then is canonical already, which is
        // what the revision/width check asks.
        if let Some((revision, cached_width, base)) = state.heading_base.take()
            && (revision, cached_width) == (doc.revision(), width)
        {
            doc.vmap = base;
        }
        Vec::new()
    };
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
        View::Source => build_lines(&doc.source, &doc.smap, sel, highlights, &theme),
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
    doc.body_width = content_area.width;
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

        // Paint composed H1/H2 blocks last, over the ordinary terminal glyphs.
        // The *active* heading is painted too — its caret (and any selection)
        // is baked into the raster, since nothing drawn from cells can land on
        // top of a graphics-protocol image. Only a heading partly scrolled off
        // screen collapses to ordinary text (a protocol image can't be
        // clipped), and there the real terminal caret still serves.
        //
        // What actually got painted is published to `state.heading_rasters`:
        // those are the headings whose editing UI now lives in pixels, so the
        // caret suppression below and the mouse handler's raster hit-test key
        // off exactly this set — a skipped paint stays ordinary text with the
        // ordinary caret and click mapping.
        for heading in heading_rasters {
            let span = &heading.rows_span;
            let fully_visible = span.start >= doc.scroll && span.end <= doc.scroll + height;
            if !fully_visible {
                continue;
            }
            let rect = Rect {
                x: content_area.x,
                y: content_area.y + (span.start - doc.scroll) as u16,
                width: content_area.width,
                height: (span.end - span.start) as u16,
            };
            if state.images.paint_heading(
                f,
                &heading.text,
                heading.level,
                theme.heading[(heading.level as usize - 1).min(5)],
                rect,
                heading.caret,
                heading.selection,
            ) {
                state.heading_rasters.push(heading);
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
    //
    // …unless a painted heading raster covers the caret's row: that raster
    // carries its own caret in its pixels, and the terminal cursor would sit on
    // top of it as a cell-sized block in the wrong place. Left unset, ratatui
    // keeps the terminal cursor hidden for the frame.
    #[cfg(feature = "images")]
    let caret_in_raster = state
        .heading_rasters
        .iter()
        .any(|h| h.rows_span.contains(&caret_row));
    #[cfg(not(feature = "images"))]
    let caret_in_raster = false;
    let in_caret_code = caret_span.as_ref().is_some_and(|s| s.contains(&caret_row));
    let caret_x = if in_caret_code {
        let vis = caret_col.saturating_sub(code_scroll);
        (code_inner_w == 0 || vis < code_inner_w)
            .then(|| content_area.x + (CODE_INSET + vis) as u16)
    } else {
        let col_visible = caret_col >= scroll_x && (width == 0 || caret_col < scroll_x + width);
        col_visible.then(|| content_area.x + (caret_col - scroll_x) as u16)
    };
    if !caret_in_raster
        && let Some(x) = caret_x
        && caret_row >= doc.scroll
        && (height == 0 || caret_row < doc.scroll + height)
    {
        let y = content_area.y + (caret_row - doc.scroll) as u16;
        f.set_cursor_position(Position::new(x, y));
    }
}

/// A heading block the terminal will paint as a graphics-protocol raster: its
/// projection (level, text), where it sits in the expanded map, the editing UI
/// to bake into the pixels, and the per-character source offsets that turn the
/// raster's hit-test answer back into a caret position. Only the rasters a
/// frame actually painted end up on [`crate::EditorState`] — a skipped one
/// stays ordinary text with the ordinary click mapping.
#[cfg(feature = "images")]
pub(crate) struct HeadingRaster {
    pub(crate) level: u8,
    pub(crate) text: String,
    pub(crate) rows_span: std::ops::Range<usize>,
    /// Caret byte offset into `text`, when the caret sits in this heading.
    pub(crate) caret: Option<usize>,
    /// Selected byte range of `text`, when the selection touches this heading.
    pub(crate) selection: Option<(usize, usize)>,
    /// The source offset of each character of `text`, in order — the reverse
    /// of the projection, so a hit-tested character index maps straight to the
    /// offset `Doc::place_caret` wants, with no trip through display columns
    /// (whose per-cluster widths this module has no business re-deriving).
    pub(crate) srcs: Vec<usize>,
    /// Where a hit past the last character lands: the heading's trailing edge.
    pub(crate) end_src: usize,
}

/// Insert presentation-only blank rows after H1/H2 blocks and shift every
/// structural side table by the same row-boundary map. The source/caret stops
/// themselves are unchanged; filler rows are decoration and therefore mouse
/// clicks on their lower half resolve to the heading's trailing stop.
///
/// The active heading expands like any other — its raster carries the caret —
/// so each returned [`HeadingRaster`] also maps the document's caret and
/// selection into byte offsets of its own text, where the rasterizer needs
/// them.
///
/// Two kinds of heading are left alone (no filler, no raster): one with no
/// visible glyphs, where the ordinary empty row is the honest rendering, and
/// one `fits` refuses — a title too long for the single layout line a raster
/// box holds, whose tail would be culled from the pixels: undrawn, unclickable,
/// and with nowhere truthful for its caret to stand. Those stay ordinary
/// (fully visible, fully editable) terminal text.
#[cfg(feature = "images")]
fn expand_headings(
    vmap: &mut VisualMap,
    caret: usize,
    active_row: usize,
    selection: Option<(usize, usize)>,
    mut fits: impl FnMut(&str, u8, u16) -> bool,
) -> Vec<HeadingRaster> {
    let old = std::mem::take(&mut vmap.rows);
    let mut boundary = vec![0usize; old.len() + 1];
    let mut rows = Vec::with_capacity(old.len() + 8);
    let mut rasters = Vec::new();
    let mut i = 0;
    while i < old.len() {
        boundary[i] = rows.len();
        let Some(level @ 1..=2) = old[i].heading else {
            rows.push(old[i].clone());
            i += 1;
            continue;
        };
        let start = i;
        while i < old.len() && old[i].heading == Some(level) {
            boundary[i] = rows.len();
            rows.push(old[i].clone());
            i += 1;
        }
        // The heading's projected text, built glyph by glyph alongside the two
        // mappings between it and the source: `srcs`/`byte_starts` record each
        // character's source offset and text-byte position, so a source offset
        // maps into the text (for the caret and selection the rasterizer
        // paints) and a hit-tested character index maps back out (for the
        // mouse). One snap-forward rule serves every direction: an offset lands
        // at the first glyph at-or-past it (offsets between glyphs — hidden
        // markup, say — snap forward), or at the end of the text when every
        // glyph lies before it.
        let mut text = String::new();
        let mut srcs = Vec::new();
        let mut byte_starts = Vec::new();
        for row in &old[start..i] {
            for g in &row.glyphs {
                srcs.push(g.src);
                byte_starts.push(text.len());
                text.push(g.ch);
            }
        }
        let target: usize = if level == 1 { 3 } else { 2 };
        if text.is_empty() || !fits(&text, level, target as u16) {
            continue;
        }
        let byte_of = |src: usize| {
            srcs.iter()
                .position(|&s| s >= src)
                .map_or(text.len(), |j| byte_starts[j])
        };
        let caret_b = (start..i).contains(&active_row).then(|| byte_of(caret));
        let heading_sel = selection.and_then(|(s, e)| {
            let (s, e) = (byte_of(s), byte_of(e));
            (s < e).then_some((s, e))
        });
        let end_src = old[start..i]
            .iter()
            .map(|row| row.end_src)
            .max()
            .unwrap_or(old[start].end_src);
        while rows.len() - boundary[start] < target {
            let mut filler = old[i - 1].clone();
            filler.glyphs.clear();
            filler.decoration = true;
            filler.code = false;
            filler.code_lang = None;
            filler.media = None;
            filler.task = None;
            filler.directive = false;
            filler.directive_label = None;
            filler.leaf_directive = None;
            filler.boundary = None;
            rows.push(filler);
        }
        rasters.push(HeadingRaster {
            level,
            text,
            rows_span: boundary[start]..rows.len(),
            caret: caret_b,
            selection: heading_sel,
            srcs,
            end_src,
        });
    }
    boundary[old.len()] = rows.len();
    let shift = |span: &mut std::ops::Range<usize>| {
        span.start = boundary[span.start];
        span.end = boundary[span.end];
    };
    for table in &mut vmap.tables {
        shift(&mut table.rows_span);
    }
    for code in &mut vmap.code_blocks {
        shift(&mut code.rows_span);
    }
    for media in &mut vmap.media {
        shift(&mut media.rows_span);
    }
    for directive in &mut vmap.directives {
        shift(&mut directive.rows_span);
    }
    vmap.rows = rows;
    rasters
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

/// Split `source` into styled lines: its markup coloured by `smap`, any part of
/// the `[start, end)` selection reversed, and the host's highlights washed under
/// both.
///
/// `smap` is the source view's answer to the WYSIWYG view's glyph styles. An
/// empty one paints every line as plain text — which is exactly what this did
/// before the map existed, and what a frontend that never calls
/// [`Doc::build_source`] still gets.
fn build_lines(
    source: &str,
    smap: &SourceMap,
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
        // Where the syntax changes is a place to break a span for exactly the
        // reason a selection edge is. `edges_in` does its own clipping to the
        // line, but it speaks *document* offsets, as every range in core does —
        // and `cuts` is line-local, which is what `raw` below is indexed by. So
        // the edges it just appended are rebased, the way `cut` rebases the ones
        // above. Getting this wrong is invisible on the first line of a document
        // and slices out of bounds on every other one.
        let appended = cuts.len();
        smap.edges_in(line_start..line_end, &mut cuts);
        for cut in &mut cuts[appended..] {
            *cut -= line_start;
        }
        cuts.sort_unstable();
        cuts.dedup();

        let mut spans = Vec::new();
        for pair in cuts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            // Styled by what covers the run's first byte: no edge falls strictly
            // inside a run, by construction, so one probe answers for all of it.
            // No style edge falls strictly inside a run, by construction — the
            // syntax edges are among the `cuts` — so one probe at the run's
            // first byte answers for all of it, as it already did for the
            // selection.
            let base = theme.to_ratatui(smap.style_at(line_start + a));
            push(
                &mut spans,
                &raw[a..b],
                composed(base, line_start + a, sel, &mut covering, theme),
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
    use leaf_core::Format;
    use ratatui::style::Modifier;

    /// The syntax map for `src`, built the way [`render`] builds it.
    fn smap(src: &str) -> SourceMap {
        let mut doc = Doc::from_source(src.into(), Format::Markdown).unwrap();
        doc.build_source();
        doc.smap.clone()
    }

    /// The text of every span the predicate accepts, concatenated — "what came
    /// out dim?" in a form an assertion can read.
    fn spans_where(line: &Line<'_>, pred: impl Fn(&Style) -> bool) -> String {
        line.spans
            .iter()
            .filter(|s| pred(&s.style))
            .map(|s| s.content.as_ref())
            .collect()
    }

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
        // Prose with no markup in it, so the syntax map is empty and this stays
        // the test of the selection/highlight composition it always was.
        let lines = build_lines(
            "one two three",
            &SourceMap::default(),
            Some((6, 9)),
            &painted,
            &theme,
        );
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
        let lines = build_lines(source, &SourceMap::default(), None, &painted, &theme);
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

    /// The point of the whole exercise: raw markup comes out coloured, with the
    /// scaffolding told apart from the prose it holds.
    #[test]
    fn the_source_view_paints_markup_apart_from_the_text_it_delimits() {
        let theme = Theme::dark();
        let src = "# Title\n";
        let lines = build_lines(src, &smap(src), None, &[], &theme);
        assert_eq!(
            spans_where(&lines[0], |s| s.fg == Some(theme.delimiter)),
            "# ",
            "the hash is scaffolding"
        );
        assert_eq!(
            spans_where(&lines[0], |s| s.fg == Some(theme.heading[0])),
            "Title",
            "and the text is a heading"
        );
    }

    /// The map is document-wide byte offsets, like the highlights beside it, so
    /// a painter that forgot to clamp per line would colour the wrong columns
    /// further down.
    #[test]
    fn syntax_lands_on_the_line_it_actually_covers() {
        let theme = Theme::dark();
        let src = "plain line\n\n# Heading\n";
        let lines = build_lines(src, &smap(src), None, &[], &theme);
        let dim = |i: usize| spans_where(&lines[i], |s| s.fg == Some(theme.delimiter));
        assert_eq!(dim(0), "", "nothing on the prose line");
        assert_eq!(dim(2), "# ", "and the hash on the heading's");
        let whole: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(whole, "plain line", "the line still reads whole");
    }

    /// Syntax is a *base* the selection and the host's washes compose over, not
    /// a fourth thing fighting them — the delimiter keeps its ink under a
    /// highlight and still reverses under the selection.
    #[test]
    fn syntax_composes_with_the_selection_over_it() {
        let theme = Theme::dark();
        let src = "a **b** c\n";
        // Over the opening `**` and the `b` inside it.
        let lines = build_lines(src, &smap(src), Some((2, 5)), &[], &theme);
        let reversed = spans_where(&lines[0], |s| s.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(
            reversed, "**b",
            "the selection still reverses what it covers"
        );
        let bold = spans_where(&lines[0], |s| s.add_modifier.contains(Modifier::BOLD));
        assert_eq!(
            bold, "**b**",
            "and the bold run is bold, delimiters and all"
        );
    }

    /// An unbuilt map is the old behaviour exactly: every frontend that never
    /// calls `build_source` keeps painting plain text.
    #[test]
    fn an_empty_map_paints_exactly_what_it_used_to() {
        let theme = Theme::dark();
        let src = "# Title\n";
        let plain = build_lines(src, &SourceMap::default(), None, &[], &theme);
        assert_eq!(plain[0].spans.len(), 1, "one unstyled run for the line");
        assert_eq!(plain[0].spans[0].style, Style::default());
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

    #[cfg(feature = "images")]
    fn heading_doc() -> Doc {
        let mut path = std::env::temp_dir();
        path.push("leaf_ratatui_heading_layout.md");
        std::fs::write(&path, "# Large title\n\nbody\n").unwrap();
        let mut doc = Doc::open(path).unwrap();
        doc.build_visual(80);
        doc
    }

    /// Every title fits — the shape of the closure `render` builds on a
    /// terminal wide enough for the heading under test.
    #[cfg(feature = "images")]
    fn fits(_: &str, _: u8, _: u16) -> bool {
        true
    }

    #[cfg(feature = "images")]
    #[test]
    fn inactive_h1_reserves_three_rows_without_adding_caret_stops() {
        let mut doc = heading_doc();
        doc.caret = doc.source.len();
        let active = doc.vmap.pos_of_offset(doc.caret).0;
        let old_rows = doc.vmap.rows.len();
        let rasters = expand_headings(&mut doc.vmap, doc.caret, active, None, fits);
        assert_eq!(rasters.len(), 1);
        assert_eq!(rasters[0].text, "Large title");
        assert_eq!(rasters[0].rows_span.len(), 3);
        assert_eq!(doc.vmap.rows.len(), old_rows + 2);
        assert!(doc.vmap.rows[rasters[0].rows_span.end - 1].decoration);
        // The caret is elsewhere, so this raster carries no editing UI.
        assert_eq!(rasters[0].caret, None);
        assert_eq!(rasters[0].selection, None);
    }

    /// The point of painting the editing UI into the raster: the active heading
    /// expands like any other, and its raster names where the caret falls in
    /// its own text.
    #[cfg(feature = "images")]
    #[test]
    fn the_active_heading_rasterizes_too_and_carries_the_caret() {
        let mut doc = heading_doc();
        // "# Large title\n" — byte 8 is between "Large " and "title", which is
        // byte 6 of the projected text "Large title".
        doc.caret = 8;
        let active = doc.vmap.pos_of_offset(doc.caret).0;
        let old_rows = doc.vmap.rows.len();
        let rasters = expand_headings(&mut doc.vmap, doc.caret, active, None, fits);
        assert_eq!(rasters.len(), 1);
        assert_eq!(rasters[0].rows_span.len(), 3);
        assert_eq!(doc.vmap.rows.len(), old_rows + 2);
        assert_eq!(rasters[0].caret, Some(6));
    }

    /// A selection reaching into the heading maps to a byte range of the
    /// heading's own text, clamped to it; one that misses it entirely leaves
    /// the raster plain.
    #[cfg(feature = "images")]
    #[test]
    fn a_selection_maps_into_the_headings_own_text() {
        let mut doc = heading_doc();
        doc.caret = doc.source.len();
        let active = doc.vmap.pos_of_offset(doc.caret).0;

        // "Large" is source bytes 2..7 → text bytes 0..5.
        let rasters = expand_headings(&mut doc.vmap, doc.caret, active, Some((2, 7)), fits);
        assert_eq!(rasters[0].selection, Some((0, 5)));

        // A selection running past the heading's end clamps to the text.
        let mut doc = heading_doc();
        let rasters = expand_headings(
            &mut doc.vmap,
            doc.caret,
            active,
            Some((8, doc.source.len())),
            fits,
        );
        assert_eq!(rasters[0].selection, Some((6, "Large title".len())));

        // One entirely in the body doesn't touch the raster.
        let mut doc = heading_doc();
        let end = doc.source.len();
        let rasters = expand_headings(&mut doc.vmap, doc.caret, active, Some((end - 3, end)), fits);
        assert_eq!(rasters[0].selection, None);
    }

    /// The raster's reverse mapping: `srcs[i]` is the source offset of the
    /// projection's `i`-th character, which is what a hit-tested click is
    /// answered with — never a re-derived display column.
    #[cfg(feature = "images")]
    #[test]
    fn the_raster_carries_a_char_to_source_table() {
        let mut doc = heading_doc();
        doc.caret = doc.source.len();
        let active = doc.vmap.pos_of_offset(doc.caret).0;
        let rasters = expand_headings(&mut doc.vmap, doc.caret, active, None, fits);
        // "# Large title": 'L' is at source byte 2, 't' of "title" at 8.
        assert_eq!(rasters[0].srcs.len(), "Large title".chars().count());
        assert_eq!(rasters[0].srcs[0], 2);
        assert_eq!(rasters[0].srcs[6], 8);
        // Past the end lands at the heading's trailing edge, inside its rows.
        assert!(rasters[0].end_src > *rasters[0].srcs.last().unwrap());
    }

    /// A title `fits` refuses — one that would wrap past the raster's single
    /// layout line — is not expanded at all: it stays ordinary terminal text,
    /// fully visible and fully editable, rather than a raster with a culled,
    /// unclickable tail.
    #[cfg(feature = "images")]
    #[test]
    fn a_title_too_long_for_the_raster_stays_ordinary_text() {
        let mut doc = heading_doc();
        doc.caret = doc.source.len();
        let active = doc.vmap.pos_of_offset(doc.caret).0;
        let old_rows = doc.vmap.rows.len();
        let rasters = expand_headings(&mut doc.vmap, doc.caret, active, None, |_, _, _| false);
        assert!(rasters.is_empty());
        assert_eq!(doc.vmap.rows.len(), old_rows, "no filler rows reserved");
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

    /// A terminal with no graphics protocol gets the heading as *text*, not as a
    /// half-block mosaic of one. `EditorState::new` is half-blocks until
    /// `query_graphics` finds better, which is exactly the state a kitty-less
    /// terminal stays in, so the heading must render as ordinary bold letters on
    /// one row with nothing reserved beneath it.
    #[cfg(feature = "images")]
    #[test]
    fn a_heading_stays_plain_text_without_a_graphics_protocol() {
        let src = "# Large title\n\nbody\n";
        let mut p = std::env::temp_dir();
        p.push("leaf_ratatui_plain_heading.md");
        std::fs::write(&p, src).unwrap();
        let mut doc = Doc::open(p).unwrap();
        // Park the caret in the body: an *active* heading is left alone anyway,
        // so only an inactive one can show whether rows were reserved for a
        // raster that this terminal cannot paint.
        doc.caret = doc.source.len();
        let mut state = EditorState::new();
        state.set_color_scheme(ColorScheme::Dark);
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        term.draw(|f| render(f, f.area(), &mut doc, &mut state))
            .unwrap();
        let buf = term.backend().buffer().clone();
        let lines: Vec<String> = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect();
        let joined = lines.join("\n");
        let title = lines
            .iter()
            .position(|l| l.contains("Large title"))
            .unwrap_or_else(|| panic!("heading not drawn as text:\n{joined}"));
        let body = lines
            .iter()
            .position(|l| l.contains("body"))
            .unwrap_or_else(|| panic!("body not drawn:\n{joined}"));
        // One heading row, one blank line, then the body — the two filler rows an
        // H1 raster would reserve are absent.
        assert_eq!(
            body - title,
            2,
            "heading reserved raster rows on a half-blocks terminal:\n{joined}"
        );
    }
}
