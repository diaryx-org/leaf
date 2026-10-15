//! Proportional text: README.md's "What it would take", item 4.
//!
//! The rest of the spike is monospace because its caret was one node at
//! `left: Nch`. The caret is now inline (see `row.rs`), which places it after
//! the right glyph in any font, so what proportional text still needs is what
//! the Swift editor does with CTTypesetter: wrap each block at a *pixel*
//! width, and map a click's x back to a column.
//!
//! Core gives one unwrapped row per block (`Doc::build_visual_unwrapped`, as
//! for leaf-swift), and this module lays each one out with **parley** — the
//! engine Blitz itself shapes with, over the same system font collection, with
//! the same family lists the stylesheet names — so its line breaks and glyph
//! positions are the ones Blitz draws. The editor renders each of its lines
//! as a `white-space: pre` div, so Blitz never wraps anything itself: these
//! breaks are the truth, and only a sub-glyph x could drift between the two.
//!
//! A row's leading list marker, indent or quote gutter is laid out apart from
//! its body, so a wrapped item hangs under its text and a quote's gutter
//! repeats on every line — the shape Swift's `BlockChrome` draws.

use std::borrow::Cow;
use std::ops::Range;

use dioxus_widget_spike::rows::{Run, RowView};
use parley::{
    Affinity, Cursor, FontContext, FontFamily, FontStyle, FontWeight, Layout, LayoutContext,
    LineHeight, StyleProperty,
};
use unicode_width::UnicodeWidthChar;

/// Body text: the Swift editor's default face, then fallbacks.
pub const BODY: &str = r#""Iowan Old Style", Georgia, serif"#;
/// Code, tables, and anything else core spells in character cells.
pub const MONO: &str = "Menlo, monospace";
/// The stylesheet's `15px/1.5`.
pub const SIZE: f32 = 15.0;
const LEADING: f32 = 1.5;

/// The fonts and parley's scratch space, kept for the editor's lifetime.
pub struct Fonts {
    fcx: FontContext,
    lcx: LayoutContext<()>,
}

impl Fonts {
    pub fn new() -> Self {
        Self {
            fcx: FontContext::new(),
            lcx: LayoutContext::new(),
        }
    }

    /// Lay one row out `width` pixels wide. `mono` rows (code, tables) are
    /// spelled in cells by core and never wrap, as Swift scrolls code
    /// sideways rather than wrapping it.
    pub fn layout(&mut self, v: &RowView, width: f32, mono: bool) -> RowLayout {
        let split = v.runs.iter().take_while(|r| is_prefix(r)).count();
        let (prefix, body) = v.runs.split_at(split);
        let repeat = prefix.iter().any(|r| has_class(r, "gutter"));

        let pre = self.build(prefix, mono, None);
        // Not `pre.layout.width()` alone: parley gives even empty text a
        // few pixels of width, which hung every wrapped paragraph's lines
        // that far in.
        let prefix_w = if prefix.is_empty() { 0.0 } else { pre.layout.width() };
        let max = (!mono).then(|| (width - prefix_w).max(SIZE * 4.0));
        let body = self.build(body, mono, max);

        let mut lines: Vec<Line> = body
            .layout
            .lines()
            .map(|l| {
                let m = l.metrics();
                Line {
                    bytes: l.text_range(),
                    top: m.block_min_coord,
                    height: m.line_height,
                }
            })
            .collect();
        // An empty row still has a line to hold the caret.
        if lines.is_empty() {
            let h = pre.layout.lines().next().map_or(SIZE * LEADING, |l| l.metrics().line_height);
            lines.push(Line { bytes: 0..0, top: 0.0, height: h });
        }
        // A line is at least the body's own height, as a CSS line box is at
        // least its strut: a line of superscript alone is still 22.5px.
        for l in &mut lines {
            l.height = l.height.max(SIZE * LEADING);
        }
        let height = lines.iter().map(|l| l.height).sum();
        RowLayout {
            prefix_cols: cols(&pre.text),
            prefix_runs: split,
            prefix_w,
            repeat,
            prefix: pre,
            body,
            lines,
            height,
        }
    }

    /// One parley layout over `runs`, with each run's style from its classes.
    fn build(&mut self, runs: &[Run], mono: bool, max: Option<f32>) -> Built {
        let mut text = String::new();
        let mut ranges = Vec::with_capacity(runs.len());
        for r in runs {
            let start = text.len();
            text.push_str(&r.text);
            ranges.push(start..text.len());
        }
        let mut b = self.lcx.ranged_builder(&mut self.fcx, &text, 1.0, true);
        b.push_default(StyleProperty::FontFamily(FontFamily::Source(Cow::Borrowed(
            if mono { MONO } else { BODY },
        ))));
        b.push_default(StyleProperty::FontSize(SIZE));
        b.push_default(StyleProperty::LineHeight(LineHeight::FontSizeRelative(LEADING)));
        for (r, range) in runs.iter().zip(&ranges) {
            for property in style_of(&r.class) {
                b.push(property, range.clone());
            }
        }
        let mut layout = b.build(&text);
        layout.break_all_lines(max);
        Built { text, layout }
    }
}

/// The CSS in `style.rs` and `LAYOUT`, as the parley properties that change
/// a glyph's advance or a line's height. Colour and decoration change
/// neither, so they are left to the stylesheet alone.
fn style_of(class: &str) -> Vec<StyleProperty<'static, ()>> {
    let mut out = Vec::new();
    for c in class.split(' ') {
        match c {
            "h1" => out.extend([StyleProperty::FontSize(SIZE * 2.0), StyleProperty::FontWeight(FontWeight::BOLD)]),
            "h2" => out.extend([StyleProperty::FontSize(SIZE * 1.6), StyleProperty::FontWeight(FontWeight::BOLD)]),
            "h3" => out.extend([StyleProperty::FontSize(SIZE * 1.3), StyleProperty::FontWeight(FontWeight::BOLD)]),
            "h4" | "h5" | "h6" | "b" => out.push(StyleProperty::FontWeight(FontWeight::BOLD)),
            "i" | "t-comment" => out.push(StyleProperty::FontStyle(FontStyle::Italic)),
            "code" => out.push(StyleProperty::FontFamily(FontFamily::Source(Cow::Borrowed(MONO)))),
            "sup" | "sub" => out.push(StyleProperty::FontSize(SIZE * 0.8)),
            _ => {}
        }
    }
    out
}

fn has_class(r: &Run, name: &str) -> bool {
    r.class.split(' ').any(|c| c == name)
}

/// The block chrome a row starts with: a list marker, its indent, a quote's
/// gutter. Everything after the first run that is none of these is body.
fn is_prefix(r: &Run) -> bool {
    has_class(r, "marker") || has_class(r, "indent") || has_class(r, "gutter")
}

struct Built {
    text: String,
    layout: Layout<()>,
}

/// One visual line of a row's body.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Its bytes in the body's text.
    pub bytes: Range<usize>,
    /// Its top within the row, and its height, in CSS pixels.
    pub top: f32,
    pub height: f32,
}

/// A row laid out: where it wraps, how tall it is, and the geometry a click
/// and a vertical motion need.
pub struct RowLayout {
    /// The display columns the prefix spans, and how many runs it is.
    pub prefix_cols: usize,
    pub prefix_runs: usize,
    /// How wide the prefix draws: continuation lines are indented this far.
    pub prefix_w: f32,
    /// Whether continuation lines repeat the prefix (a quote's gutter) or
    /// only hang under it (a list item).
    pub repeat: bool,
    prefix: Built,
    body: Built,
    pub lines: Vec<Line>,
    pub height: f32,
}

impl RowLayout {
    /// The display column under `x` (CSS pixels from the row's left edge) on
    /// visual line `line`.
    pub fn col_at(&self, line: usize, x: f32) -> usize {
        let line = line.min(self.lines.len() - 1);
        if line == 0 && x < self.prefix_w {
            let at = Cursor::from_point(&self.prefix.layout, x, 1.0).index();
            return cols(&self.prefix.text[..at]);
        }
        let l = &self.lines[line];
        let y = l.top + l.height / 2.0;
        let at = Cursor::from_point(&self.body.layout, (x - self.prefix_w).max(0.0), y).index();
        // A click past a soft-wrapped line's end lands before its trailing
        // space, not at the next line's start.
        let at = at.min(self.line_end(line));
        self.prefix_cols + cols(&self.body.text[..at])
    }

    /// The visual line and x of display column `col`.
    pub fn caret(&self, col: usize) -> (usize, f32) {
        if col < self.prefix_cols {
            let at = byte_of_col(&self.prefix.text, col);
            let x = Cursor::from_byte_index(&self.prefix.layout, at, Affinity::Downstream)
                .geometry(&self.prefix.layout, 0.0)
                .x0 as f32;
            return (0, x);
        }
        let at = byte_of_col(&self.body.text, col - self.prefix_cols);
        let line = self.line_of(at);
        let x = Cursor::from_byte_index(&self.body.layout, at, Affinity::Downstream)
            .geometry(&self.body.layout, 0.0)
            .x0 as f32;
        (line, self.prefix_w + x)
    }

    /// The display column at the start or end of visual line `line`.
    pub fn edge(&self, line: usize, end: bool) -> usize {
        let line = line.min(self.lines.len() - 1);
        let at = if end { self.line_end(line) } else { self.lines[line].bytes.start };
        self.prefix_cols + cols(&self.body.text[..at])
    }

    /// The line a body byte belongs to: the last one starting at or before
    /// it, so a byte at a soft break is the next line's start.
    pub fn line_of(&self, byte: usize) -> usize {
        self.lines
            .iter()
            .rposition(|l| l.bytes.start <= byte)
            .unwrap_or(0)
    }

    /// The end of a line's text, before the spaces a soft wrap hangs past
    /// the edge. The last line keeps everything.
    fn line_end(&self, line: usize) -> usize {
        let b = &self.lines[line].bytes;
        if line + 1 == self.lines.len() {
            return b.end;
        }
        let s = &self.body.text[b.clone()];
        b.start + s.trim_end_matches(' ').len()
    }
}

/// Display columns, the unit core's `click` and `caret_col` speak.
pub fn cols(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// The byte at which `col` display columns of `s` have passed.
pub fn byte_of_col(s: &str, col: usize) -> usize {
    let mut seen = 0;
    for (i, c) in s.char_indices() {
        if seen >= col {
            return i;
        }
        seen += c.width().unwrap_or(0);
    }
    s.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, class: &str) -> Run {
        Run { text: text.into(), class: class.into(), src: None }
    }

    fn row(runs: Vec<Run>) -> RowView {
        RowView { runs, ..RowView::default() }
    }

    const PROSE: &str = "The quick brown fox jumps over the lazy dog, and then it does so again, \
                         at length, until the line has no choice but to wrap at least twice over.";

    #[test]
    fn a_long_row_wraps_and_every_line_is_a_body_line_tall() {
        let mut f = Fonts::new();
        let l = f.layout(&row(vec![run(PROSE, "g")]), 300.0, false);
        assert!(l.lines.len() >= 3, "{} lines", l.lines.len());
        assert!(l.lines.iter().all(|x| x.height >= SIZE * LEADING));
        assert_eq!(l.lines.first().unwrap().bytes.start, 0);
        assert_eq!(l.lines.last().unwrap().bytes.end, PROSE.len());
    }

    #[test]
    fn a_column_round_trips_through_its_x() {
        let mut f = Fonts::new();
        let l = f.layout(&row(vec![run(PROSE, "g")]), 300.0, false);
        for col in [0, 5, 40, 90, PROSE.len()] {
            let (line, x) = l.caret(col);
            assert_eq!(l.col_at(line, x + 0.5), col, "col {col} at line {line} x {x}");
        }
    }

    #[test]
    fn a_list_item_hangs_its_body_under_the_marker() {
        let mut f = Fonts::new();
        let l = f.layout(&row(vec![run("• ", "g marker"), run(PROSE, "g")]), 300.0, false);
        assert_eq!(l.prefix_cols, 2);
        assert!(l.prefix_w > 0.0);
        assert!(!l.repeat);
        // Clicking left of the body on line 0 lands in the marker.
        assert!(l.col_at(0, 0.0) < 2);
        // On a continuation line it lands at the body's start of that line.
        assert_eq!(l.col_at(1, 0.0), l.edge(1, false));
    }

    #[test]
    fn a_paragraph_has_no_prefix_to_hang_under() {
        let mut f = Fonts::new();
        let l = f.layout(&row(vec![run(PROSE, "g")]), 300.0, false);
        assert_eq!(l.prefix_w, 0.0);
        assert_eq!(l.col_at(1, 0.0), l.edge(1, false));
    }

    #[test]
    fn a_code_row_never_wraps() {
        let mut f = Fonts::new();
        let l = f.layout(&row(vec![run(PROSE, "g code")]), 300.0, true);
        assert_eq!(l.lines.len(), 1);
    }

    #[test]
    fn a_heading_line_is_taller() {
        let mut f = Fonts::new();
        let l = f.layout(&row(vec![run("Title", "g h1 b")]), 600.0, false);
        assert!((l.height - SIZE * 2.0 * LEADING).abs() < 0.5, "{}", l.height);
    }

    #[test]
    fn an_empty_row_has_one_line() {
        let mut f = Fonts::new();
        let l = f.layout(&row(vec![]), 600.0, false);
        assert_eq!(l.lines.len(), 1);
        assert_eq!(l.caret(0).0, 0);
    }
}

