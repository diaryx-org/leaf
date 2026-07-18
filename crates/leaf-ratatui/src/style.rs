//! The terminal end of the styling seam: map leaf-core's toolkit-neutral
//! [`leaf_core::Style`] onto a `ratatui::Style`, and turn a WYSIWYG
//! [`VisualMap`] into styled ratatui lines.
//!
//! This is the code that used to live on `VisualMap::to_lines` in the old
//! single-crate leaf. It moved here because a `Line`/`Span` is a ratatui type;
//! the geometry it reads (glyphs, source offsets) stays in leaf-core so a GUI
//! frontend reuses it unchanged.

use leaf_core::style::{Role, Style as LStyle};
use leaf_core::VisualMap;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// The tint behind code — the pill behind an inline `` `code` `` run and the
/// fill of a fenced block's box — and the block box's border. Quiet 256-colour
/// greys so they read as a subtle panel on a dark terminal and degrade to
/// nothing alarming elsewhere; the code text stays its own green on top.
pub const CODE_BG: Color = Color::Indexed(235);
pub const CODE_BORDER: Color = Color::Indexed(240);

/// The frame drawn around a block image's reserved area — the picture sits inside
/// it, and it stands alone as the "picture goes here" placeholder when the raster
/// can't be painted (a remote/unresolved image, or one scrolled so it doesn't
/// fully fit — a graphics-protocol image can't be clipped, but this cell-drawn
/// border can). A muted magenta, kin to the `🖼` [`Role::Image`] label.
pub const IMAGE_BORDER: Color = Color::Indexed(96);

/// How far a fenced code block's text is inset from the left edge of its box —
/// one column, the room the box's left border sits in. The caret and mouse math
/// in `ui` shift a code row's columns by this same amount.
pub const CODE_INSET: usize = 1;

/// The terminal's palette, keyed on a glyph's semantic [`Role`]. This is the
/// presentation core used to bake in and no longer does: a terminal can only
/// tell a heading from body text by *color*, so the choice of which color lives
/// here, in the frontend that has the constraint — not in the shared model. A
/// GUI, which can vary size and font, maps the same roles to entirely different
/// looks.
///
/// Returns the base style for the role; the caller layers the author's own
/// emphasis (bold/italic/…) on top. Headings cycle a color by level and bold,
/// exactly as before the palette moved out of core.
fn role_style(role: Role) -> Style {
    let s = Style::default();
    match role {
        Role::Body => s,
        Role::Heading(level) => {
            let base = s.add_modifier(Modifier::BOLD);
            match level {
                1 => base.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED),
                2 => base.fg(Color::Green),
                3 => base.fg(Color::Yellow),
                4 => base.fg(Color::Blue),
                5 => base.fg(Color::Magenta),
                _ => base.fg(Color::Gray),
            }
        }
        // Code reads green on the code tint — inline it's the whole pill, in a
        // fenced block the box's fill matches so the two blend.
        Role::Code => s.fg(Color::Green).bg(CODE_BG),
        Role::Link => s.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED),
        Role::Mark => s.fg(Color::Black).bg(Color::Yellow),
        Role::ListMarker => s.fg(Color::Yellow),
        Role::QuoteGutter => s.fg(Color::Green),
        // Thematic breaks and table rules are quiet grey.
        Role::Rule => s.fg(Color::DarkGray),
        // A block image's `🖼 alt` placeholder: the terminal has no raster
        // primitive, so it paints the label — dim magenta to read as a
        // stand-in for content it can't draw, not as prose.
        Role::Image => s.fg(Color::Magenta).add_modifier(Modifier::DIM),
    }
}

/// Map a neutral core style onto a ratatui style: the role picks the palette,
/// then the author's own emphasis flags layer on top.
pub fn to_ratatui(s: LStyle) -> Style {
    let mut out = role_style(s.role);
    if s.bold {
        out = out.add_modifier(Modifier::BOLD);
    }
    if s.italic {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if s.underline {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    if s.strikethrough {
        out = out.add_modifier(Modifier::CROSSED_OUT);
    }
    out
}

/// Styled ratatui lines for the WYSIWYG map, drawing any glyph whose source
/// offset is within the `[start, end)` selection reversed. Adjacent glyphs of
/// equal style are merged into one span.
///
/// `code_shift(row)` returns `Some(scroll)` for a fenced code-block row — the
/// display columns to scroll it left inside its box — and `None` for ordinary
/// text. A code row is drawn inset by [`CODE_INSET`] (room for the box's left
/// border) and scrolled: the leading `scroll` columns are dropped so a long line
/// slides under the box rather than wrapping or running off the right edge. The
/// caret and mouse in `ui` mirror this exact shift, so a code column still round-
/// trips to its source byte.
pub fn wysiwyg_lines(
    vmap: &VisualMap,
    sel: Option<(usize, usize)>,
    code_shift: impl Fn(usize) -> Option<usize>,
) -> Vec<Line<'static>> {
    let (ss, se) = sel.unwrap_or((usize::MAX, usize::MAX));
    vmap.rows
        .iter()
        .enumerate()
        .map(|(r, row)| {
            let shift = code_shift(r);
            let mut spans: Vec<Span<'static>> = Vec::new();
            // A code row opens with its inset: the columns the box's left border
            // lands on, tinted so the fill runs edge to edge under the border.
            if shift.is_some() {
                spans.push(Span::styled(
                    " ".repeat(CODE_INSET),
                    Style::default().bg(CODE_BG),
                ));
            }
            let mut buf = String::new();
            let mut cur: Option<Style> = None;
            // Column of the current glyph within the row, to drop the ones a code
            // row has scrolled off its left edge.
            let mut col = 0usize;
            for g in &row.glyphs {
                let w = char_cols(g.ch);
                let hidden = shift.is_some_and(|scroll| col < scroll);
                col += w;
                if hidden {
                    continue;
                }
                let mut style = to_ratatui(g.style);
                if g.src >= ss && g.src < se {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                if cur == Some(style) {
                    buf.push(g.ch);
                } else {
                    if let Some(s) = cur.take() {
                        spans.push(Span::styled(std::mem::take(&mut buf), s));
                    }
                    cur = Some(style);
                    buf.push(g.ch);
                }
            }
            if let Some(s) = cur {
                spans.push(Span::styled(buf, s));
            }
            if spans.is_empty() {
                spans.push(Span::raw(""));
            }
            Line::from(spans)
        })
        .collect()
}

/// The display-column width of one glyph — the terminal's own measure, matched
/// to how `leaf-core` lays a row out into columns so a scrolled code column and
/// its caret can't drift apart.
fn char_cols(ch: char) -> usize {
    leaf_core::wysiwyg::text_width(ch.encode_utf8(&mut [0u8; 4]))
}
