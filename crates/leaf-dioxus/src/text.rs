//! Proportional text: `spikes/dioxus-widget/README.md`'s "What it would
//! take", item 4.
//!
//! The spike drew monospace because its caret was one node at `left: Nch`.
//! The caret here is inline (see `row.rs`), which places it after
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

use crate::bridge::rows::{RowView, Run};
pub use parley::Alignment;
use parley::{
    Affinity, AlignmentOptions, Cursor, FontContext, FontFamily, FontStyle, FontWeight, InlineBox,
    InlineBoxKind, Layout, LayoutContext, LineHeight, StyleProperty, VerticalAlign,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// How the text is set: the faces, the size and the leading. One value, held
/// by [`Fonts`] and written into the stylesheet (`view.rs`), so parley
/// measures exactly what Blitz draws — the two must never disagree, or every
/// click lands a few glyphs off. The host makes one, as part of its
/// [`Look`](crate::Look); changing it is a relayout of every row.
#[derive(Clone, Debug, PartialEq)]
pub struct Type {
    /// Body text, as a CSS family list: the face, then fallbacks.
    pub body: String,
    /// Code, tables, and anything else core spells in character cells.
    pub mono: String,
    /// The body size, in CSS pixels.
    pub size: f32,
    /// The line box over the size: CSS's unitless `line-height`.
    pub leading: f32,
}

impl Default for Type {
    /// The Swift editor's defaults: Iowan Old Style at 16 on 1.5.
    fn default() -> Self {
        Self {
            body: r#""Iowan Old Style", Georgia, serif"#.into(),
            mono: "Menlo, monospace".into(),
            size: 16.0,
            leading: 1.5,
        }
    }
}

impl Type {
    /// One body line's height.
    pub fn line(&self) -> f32 {
        self.size * self.leading
    }
}

/// How a row is set.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Kind {
    /// Body text: proportional, wrapped, headings at their sizes.
    Prose,
    /// Spelled in character cells by core — code, a table's picture — so
    /// monospace and never wrapped, as Swift scrolls code sideways.
    Cells,
    /// A source-view line: monospace, wrapped, and every run at body size —
    /// a heading's `#` line is bold, not 2em, as in the Swift editor.
    Source,
}

/// The fonts and parley's scratch space, kept for the editor's lifetime.
pub struct Fonts {
    fcx: FontContext,
    lcx: LayoutContext<()>,
    ty: Type,
}

impl Fonts {
    pub fn new(ty: Type) -> Self {
        Self {
            fcx: FontContext::new(),
            lcx: LayoutContext::new(),
            ty,
        }
    }

    pub fn ty(&self) -> &Type {
        &self.ty
    }

    /// Set the text in another type. Every layout made before is stale.
    pub fn set_ty(&mut self, ty: Type) {
        self.ty = ty;
    }

    /// Whether the system has a family by this name — a settings choice
    /// that is not installed would only fall back to the next in its list.
    pub fn has_family(&mut self, name: &str) -> bool {
        self.fcx.collection.family_by_name(name).is_some()
    }

    /// The mean advance of a lowercase body character, the alphabet and a
    /// space: what turns a measure in characters into pixels — the Swift
    /// editor's `averageCharWidth`, measured off the real font the same way.
    pub fn char_width(&mut self) -> f32 {
        const SAMPLE: &str = "abcdefghijklmnopqrstuvwxyz ";
        let w = self
            .build(&[plain(SAMPLE)], Kind::Prose, None)
            .layout
            .width();
        w / SAMPLE.len() as f32
    }

    /// One character cell of the code face.
    pub fn cell_width(&mut self) -> f32 {
        const SAMPLE: &str = "0000000000";
        let w = self
            .build(&[plain(SAMPLE)], Kind::Cells, None)
            .layout
            .width();
        w / SAMPLE.len() as f32
    }

    /// Lay one row out `width` pixels wide.
    pub fn layout(&mut self, v: &RowView, width: f32, kind: Kind) -> RowLayout {
        self.layout_aligned(v, width, kind, Alignment::Start)
    }

    /// The same, with its lines aligned within `width` — a table cell's
    /// `:-:` or `--:`.
    pub fn layout_aligned(
        &mut self,
        v: &RowView,
        width: f32,
        kind: Kind,
        align: Alignment,
    ) -> RowLayout {
        let split = v.runs.iter().take_while(|r| is_prefix(r)).count();
        let (prefix, body) = v.runs.split_at(split);
        let repeat = prefix.iter().any(|r| has_class(r, "gutter"));

        let pre = self.build(prefix, kind, None);
        // Not `pre.layout.width()` alone: parley gives even empty text a
        // few pixels of width, which hung every wrapped paragraph's lines
        // that far in.
        let prefix_w = if prefix.is_empty() {
            0.0
        } else {
            pre.layout.width()
        };
        let line = self.ty.line();
        let max = (kind != Kind::Cells).then(|| (width - prefix_w).max(self.ty.size * 4.0));
        let mut body = self.build(body, kind, max);
        if align != Alignment::Start && max.is_some() {
            body.layout.align(align, AlignmentOptions::default());
        }

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
            let h = pre
                .layout
                .lines()
                .next()
                .map_or(line, |l| l.metrics().line_height);
            lines.push(Line {
                bytes: 0..0,
                top: 0.0,
                height: h,
            });
        }
        // A line is at least the body's own height, as a CSS line box is at
        // least its strut: a line of superscript alone is still 22.5px.
        for l in &mut lines {
            l.height = l.height.max(line);
        }
        let height = lines.iter().map(|l| l.height).sum();
        let width = prefix_w + body.layout.width();
        RowLayout {
            width,
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
    fn build(&mut self, runs: &[Run], kind: Kind, max: Option<f32>) -> Built {
        let mut text = String::new();
        let mut ranges = Vec::with_capacity(runs.len());
        for r in runs {
            let start = text.len();
            text.push_str(&r.text);
            ranges.push(start..text.len());
        }
        let ty = &self.ty;
        let mut b = self.lcx.ranged_builder(&mut self.fcx, &text, 1.0, true);
        b.push_default(StyleProperty::FontFamily(FontFamily::Source(Cow::Owned(
            if kind == Kind::Prose {
                ty.body.clone()
            } else {
                ty.mono.clone()
            },
        ))));
        b.push_default(StyleProperty::FontSize(ty.size));
        b.push_default(StyleProperty::LineHeight(LineHeight::FontSizeRelative(
            ty.leading,
        )));
        for (r, range) in runs.iter().zip(&ranges) {
            for property in style_of(&r.class, kind == Kind::Source, ty) {
                b.push(property, range.clone());
            }
            // An inline formula: its atom glyph takes no room, and its
            // picture is a box after it, on the text's baseline.
            if let Some(src) = r.src.filter(|_| has_class(r, "math"))
                && let Some(p) = crate::picture::lay_inline(src, run_size(&r.class, ty))
            {
                b.push(StyleProperty::FontSize(0.01), range.clone());
                b.push_inline_box(InlineBox {
                    id: src as u64,
                    kind: InlineBoxKind::InFlow,
                    index: range.end,
                    width: p.size.0,
                    height: p.size.1,
                    baseline: Some(p.size.1 - p.depth),
                    vertical_align: VerticalAlign::BASELINE,
                });
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
fn style_of(class: &str, uniform: bool, ty: &Type) -> Vec<StyleProperty<'static, ()>> {
    let size = ty.size;
    let mut out = Vec::new();
    for c in class.split(' ') {
        match c {
            // The source view sets every run at body size; see `Kind::Source`.
            "h1" | "h2" | "h3" | "sup" | "sub" if uniform => {
                out.push(StyleProperty::FontWeight(if c.starts_with('h') {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                }))
            }
            "h1" => out.extend([
                StyleProperty::FontSize(size * 2.0),
                StyleProperty::FontWeight(FontWeight::BOLD),
            ]),
            "h2" => out.extend([
                StyleProperty::FontSize(size * 1.6),
                StyleProperty::FontWeight(FontWeight::BOLD),
            ]),
            "h3" => out.extend([
                StyleProperty::FontSize(size * 1.3),
                StyleProperty::FontWeight(FontWeight::BOLD),
            ]),
            "h4" | "h5" | "h6" | "b" => out.push(StyleProperty::FontWeight(FontWeight::BOLD)),
            "i" | "t-comment" => out.push(StyleProperty::FontStyle(FontStyle::Italic)),
            "code" => out.push(StyleProperty::FontFamily(FontFamily::Source(Cow::Owned(
                ty.mono.clone(),
            )))),
            "sup" | "sub" => out.push(StyleProperty::FontSize(size * 0.8)),
            _ => {}
        }
    }
    out
}

/// The font size a run is set at, for a formula set inline in it.
fn run_size(class: &str, ty: &Type) -> f32 {
    let mut size = ty.size;
    for c in class.split(' ') {
        size = match c {
            "h1" => ty.size * 2.0,
            "h2" => ty.size * 1.6,
            "h3" => ty.size * 1.3,
            "sup" | "sub" => ty.size * 0.8,
            _ => size,
        };
    }
    size
}

/// A run of body text with no style of its own.
fn plain(text: &str) -> Run {
    Run {
        text: text.into(),
        class: "g".into(),
        src: None,
    }
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
    /// How wide its widest line draws — a table column's natural width.
    pub width: f32,
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
        let at = if end {
            self.line_end(line)
        } else {
            self.lines[line].bytes.start
        };
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

/// Display columns, the unit core's `click` and `caret_col` speak — counted
/// as core counts them (`leaf_core::text_width`), per grapheme cluster: a
/// family emoji is five codepoints and one 2-column character. Summing each
/// codepoint's width made it 6, so a caret core placed after it mapped to a
/// byte *inside* it, the run split there, and the halves shaped apart.
pub fn cols(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// The byte at which `col` display columns of `s` have passed — always a
/// grapheme cluster's boundary, never inside one.
pub fn byte_of_col(s: &str, col: usize) -> usize {
    let mut seen = 0;
    for (i, cluster) in s.grapheme_indices(true) {
        if seen >= col {
            return i;
        }
        seen += UnicodeWidthStr::width(cluster);
    }
    s.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, class: &str) -> Run {
        Run {
            text: text.into(),
            class: class.into(),
            src: None,
        }
    }

    fn row(runs: Vec<Run>) -> RowView {
        RowView {
            runs,
            ..RowView::default()
        }
    }

    const PROSE: &str = "The quick brown fox jumps over the lazy dog, and then it does so again, \
                         at length, until the line has no choice but to wrap at least twice over.";

    #[test]
    fn a_long_row_wraps_and_every_line_is_a_body_line_tall() {
        let mut f = Fonts::new(Type::default());
        let l = f.layout(&row(vec![run(PROSE, "g")]), 300.0, Kind::Prose);
        assert!(l.lines.len() >= 3, "{} lines", l.lines.len());
        assert!(l.lines.iter().all(|x| x.height >= Type::default().line()));
        assert_eq!(l.lines.first().unwrap().bytes.start, 0);
        assert_eq!(l.lines.last().unwrap().bytes.end, PROSE.len());
    }

    #[test]
    fn a_column_round_trips_through_its_x() {
        let mut f = Fonts::new(Type::default());
        let l = f.layout(&row(vec![run(PROSE, "g")]), 300.0, Kind::Prose);
        for col in [0, 5, 40, 90, PROSE.len()] {
            let (line, x) = l.caret(col);
            assert_eq!(
                l.col_at(line, x + 0.5),
                col,
                "col {col} at line {line} x {x}"
            );
        }
    }

    #[test]
    fn a_list_item_hangs_its_body_under_the_marker() {
        let mut f = Fonts::new(Type::default());
        let l = f.layout(
            &row(vec![run("• ", "g marker"), run(PROSE, "g")]),
            300.0,
            Kind::Prose,
        );
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
        let mut f = Fonts::new(Type::default());
        let l = f.layout(&row(vec![run(PROSE, "g")]), 300.0, Kind::Prose);
        assert_eq!(l.prefix_w, 0.0);
        assert_eq!(l.col_at(1, 0.0), l.edge(1, false));
    }

    #[test]
    fn a_code_row_never_wraps() {
        let mut f = Fonts::new(Type::default());
        let l = f.layout(&row(vec![run(PROSE, "g code")]), 300.0, Kind::Cells);
        assert_eq!(l.lines.len(), 1);
    }

    #[test]
    fn a_heading_line_is_taller() {
        let mut f = Fonts::new(Type::default());
        let l = f.layout(&row(vec![run("Title", "g h1 b")]), 600.0, Kind::Prose);
        assert!(
            (l.height - Type::default().line() * 2.0).abs() < 0.5,
            "{}",
            l.height
        );
    }

    #[test]
    fn a_family_emoji_is_one_two_column_character() {
        let family = "👨‍👩‍👧";
        let s = format!("a{family}b");
        assert_eq!(cols(family), 2);
        assert_eq!(byte_of_col(&s, 1), 1);
        assert_eq!(byte_of_col(&s, 3), 1 + family.len());
        // A column inside it lands at its end, never between its codepoints.
        assert_eq!(byte_of_col(&s, 2), 1 + family.len());
    }

    #[test]
    fn a_source_line_wraps_and_a_heading_stays_body_height() {
        let mut f = Fonts::new(Type::default());
        let l = f.layout(&row(vec![run(PROSE, "g")]), 300.0, Kind::Source);
        assert!(l.lines.len() > 1);
        let h = f.layout(&row(vec![run("# Title", "g h1 b")]), 600.0, Kind::Source);
        assert!(
            (h.height - Type::default().line()).abs() < 0.5,
            "{}",
            h.height
        );
    }

    #[test]
    fn an_empty_row_has_one_line() {
        let mut f = Fonts::new(Type::default());
        let l = f.layout(&row(vec![]), 600.0, Kind::Prose);
        assert_eq!(l.lines.len(), 1);
        assert_eq!(l.caret(0).0, 0);
    }

    #[test]
    fn a_larger_size_or_leading_sets_taller_lines() {
        let base = Type::default();
        let mut f = Fonts::new(base.clone());
        let one = f
            .layout(&row(vec![run("Text", "g")]), 600.0, Kind::Prose)
            .height;
        assert!((one - base.line()).abs() < 0.5, "{one}");
        f.set_ty(Type {
            size: 20.0,
            ..base.clone()
        });
        let bigger = f
            .layout(&row(vec![run("Text", "g")]), 600.0, Kind::Prose)
            .height;
        assert!((bigger - 30.0).abs() < 0.5, "{bigger}");
        f.set_ty(Type {
            leading: 2.0,
            ..base
        });
        let looser = f
            .layout(&row(vec![run("Text", "g")]), 600.0, Kind::Prose)
            .height;
        assert!((looser - 32.0).abs() < 0.5, "{looser}");
    }

    #[test]
    fn a_larger_size_wraps_sooner_and_still_round_trips() {
        let mut f = Fonts::new(Type::default());
        let small = f
            .layout(&row(vec![run(PROSE, "g")]), 300.0, Kind::Prose)
            .lines
            .len();
        f.set_ty(Type {
            size: 24.0,
            ..Type::default()
        });
        let l = f.layout(&row(vec![run(PROSE, "g")]), 300.0, Kind::Prose);
        assert!(l.lines.len() > small, "{} vs {small}", l.lines.len());
        for col in [0, 7, 50, PROSE.len()] {
            let (line, x) = l.caret(col);
            assert_eq!(l.col_at(line, x + 0.5), col);
        }
    }

    #[test]
    fn a_character_is_wider_at_a_larger_size() {
        let mut f = Fonts::new(Type::default());
        let (body, cell) = (f.char_width(), f.cell_width());
        // Menlo's cell is 0.602em; a lowercase serif letter about half an em.
        assert!((cell - 0.602 * 16.0).abs() < 0.2, "{cell}");
        assert!(body > 16.0 * 0.3 && body < 16.0 * 0.6, "{body}");
        f.set_ty(Type {
            size: 32.0,
            ..Type::default()
        });
        assert!((f.char_width() - 2.0 * body).abs() < 0.2);
    }

    #[test]
    fn a_family_is_known_by_name() {
        let mut f = Fonts::new(Type::default());
        assert!(f.has_family("Menlo"));
        assert!(!f.has_family("No Such Face Anywhere"));
    }
}
