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
        Role::Code => s.fg(Color::Green),
        Role::Link => s.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED),
        Role::Mark => s.fg(Color::Black).bg(Color::Yellow),
        Role::ListMarker => s.fg(Color::Yellow),
        Role::QuoteGutter => s.fg(Color::Green),
        // The code fence, thematic breaks, and table rules are all quiet grey.
        Role::CodeFence | Role::Rule => s.fg(Color::DarkGray),
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
pub fn wysiwyg_lines(vmap: &VisualMap, sel: Option<(usize, usize)>) -> Vec<Line<'static>> {
    let (ss, se) = sel.unwrap_or((usize::MAX, usize::MAX));
    vmap.rows
        .iter()
        .map(|row| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut buf = String::new();
            let mut cur: Option<Style> = None;
            for g in &row.glyphs {
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
