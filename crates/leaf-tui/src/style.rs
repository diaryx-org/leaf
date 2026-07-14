//! The terminal end of the styling seam: map leaf-core's toolkit-neutral
//! [`leaf_core::Style`] onto a `ratatui::Style`, and turn a WYSIWYG
//! [`VisualMap`] into styled ratatui lines.
//!
//! This is the code that used to live on `VisualMap::to_lines` in the old
//! single-crate leaf. It moved here because a `Line`/`Span` is a ratatui type;
//! the geometry it reads (glyphs, source offsets) stays in leaf-core so a GUI
//! frontend reuses it unchanged.

use leaf_core::style::{Color as LColor, Style as LStyle};
use leaf_core::VisualMap;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Map a neutral core color onto a terminal color.
fn color(c: LColor) -> Color {
    match c {
        LColor::Default => Color::Reset,
        LColor::Black => Color::Black,
        LColor::Cyan => Color::Cyan,
        LColor::Green => Color::Green,
        LColor::Yellow => Color::Yellow,
        LColor::Blue => Color::Blue,
        LColor::Magenta => Color::Magenta,
        LColor::Gray => Color::Gray,
        LColor::DarkGray => Color::DarkGray,
    }
}

/// Map a neutral core style onto a ratatui style.
pub fn to_ratatui(s: LStyle) -> Style {
    let mut out = Style::default().fg(color(s.fg)).bg(color(s.bg));
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
