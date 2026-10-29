//! The pages' geometry: the document as Typst set it, read as lines of
//! glyphs, each with the bytes of the source it was set from.
//!
//! Each page is twig-export's (`pdf::layout`): its size, its pieces (a
//! picture of each block), and its glyphs. [`Geometry`] reads the glyphs as
//! lines: where a caret at a byte is drawn, which byte a point on a page
//! lands on, the byte a line up or down, a line's edges, and the boxes a
//! selection covers. In the page view the editor's moves by line go by these
//! lines ([`PageLines`], the editor's [`leaf_dioxus::Lines`]).
//!
//! A caret the editor places where no glyph is (an empty paragraph, a
//! line's hidden markup) is drawn beside the nearest glyph of the source.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use leaf_core::Doc;
use twig_export::pdf::Glyph;

/// Space around the pages and between them, in CSS pixels.
pub(crate) const PAD: f64 = 16.0;
/// The widest a page is drawn, in CSS pixels, however wide the view, at
/// zoom 1.
const WIDEST: f64 = 920.0;

/// A page as set: its size in points. What is drawn on it is its pieces'.
#[derive(Clone, Debug)]
pub struct Sheet {
    pub width: f64,
    pub height: f64,
}

/// A piece of a page, drawn by itself: a block of the document as set there
/// (twig-export's tiles), at its place on its page, in points.
#[derive(Clone, Debug)]
pub struct Piece {
    pub page: usize,
    pub top: f64,
    pub height: f64,
    pub width: f64,
    /// What it draws, as a hash: the same picture wherever it is.
    pub key: u128,
    /// A `data:` URL of its SVG.
    pub url: Rc<str>,
}

/// The pages placed down a view `width` CSS pixels wide: the scale from
/// points, each page's left and top in the view, and how tall they are
/// together.
#[derive(Clone, Debug, PartialEq)]
pub struct Spread {
    pub scale: f64,
    pub lefts: Vec<f64>,
    pub tops: Vec<f64>,
    pub height: f64,
}

impl Spread {
    /// Reflowed, the one page is the view's own width, drawn at its true
    /// size (a point is 4/3 of a CSS pixel) from the view's top left.
    pub fn of(sheets: &[Sheet], width: f64, reflow: bool) -> Spread {
        if reflow {
            let scale = 4.0 / 3.0;
            let mut tops = Vec::with_capacity(sheets.len());
            let mut y = 0.0;
            for s in sheets {
                tops.push(y);
                y += s.height * scale;
            }
            return Spread {
                scale,
                lefts: vec![0.0; sheets.len()],
                tops,
                height: y,
            };
        }
        let widest = sheets.iter().map(|s| s.width).fold(1.0, f64::max);
        let scale = ((width - 2.0 * PAD).min(WIDEST) / widest).max(0.05);
        let mut tops = Vec::with_capacity(sheets.len());
        let mut lefts = Vec::with_capacity(sheets.len());
        let mut y = PAD;
        for s in sheets {
            tops.push(y);
            lefts.push(((width - s.width * scale) / 2.0).max(PAD));
            y += s.height * scale + PAD;
        }
        Spread {
            scale,
            lefts,
            tops,
            height: y,
        }
    }

    /// The page a point `y` down the view is on, and the point in the
    /// page's points.
    pub fn on_page(&self, sheets: &[Sheet], x: f64, y: f64) -> Option<(usize, f64, f64)> {
        let page = self.tops.partition_point(|&t| t <= y).checked_sub(1)?;
        let px = (x - self.lefts[page]) / self.scale;
        let py = (y - self.tops[page]) / self.scale;
        (py <= sheets[page].height + PAD / self.scale).then_some((page, px, py))
    }
}

impl Spread {
    /// The page nearest a point `y` down the view, and the point in its
    /// points, held to the page: a press below the last page, above the
    /// first or between two lands on the nearest, as a press past a line's
    /// end lands at its end.
    pub fn nearest(&self, sheets: &[Sheet], x: f64, y: f64) -> Option<(usize, f64, f64)> {
        if sheets.is_empty() {
            return None;
        }
        let mut page = self.tops.partition_point(|&t| t <= y).saturating_sub(1);
        // Nearer the next page's top than this one's bottom.
        if let Some(&next) = self.tops.get(page + 1) {
            let bottom = self.tops[page] + sheets[page].height * self.scale;
            if y - bottom > next - y {
                page += 1;
            }
        }
        let px = (x - self.lefts[page]) / self.scale;
        let py = ((y - self.tops[page]) / self.scale).clamp(0.0, sheets[page].height);
        Some((page, px, py))
    }
}

/// A box on a page, in its points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageBox {
    pub page: usize,
    pub x: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

/// One line of glyphs from the source: a page's glyphs on one baseline,
/// left to right.
#[derive(Clone, Debug)]
pub struct Line {
    page: usize,
    baseline: f64,
    ascent: f64,
    descent: f64,
    /// Shared, so that a line moved down a page is a cheap copy.
    glyphs: std::sync::Arc<[(f64, f64, Range<usize>)]>,
    /// How wide a space is on the line: one Typst set there, or a quarter
    /// of an em, the em guessed from the line's height.
    space: Option<f64>,
}

impl Line {
    /// Its baseline, in points down its page.
    pub fn baseline(&self) -> f64 {
        self.baseline
    }

    /// This line `dy` points further down its page.
    pub fn shifted(&self, dy: f64) -> Line {
        Line {
            baseline: self.baseline + dy,
            ..self.clone()
        }
    }

    fn space(&self) -> f64 {
        self.space.unwrap_or(0.22 * (self.ascent + self.descent))
    }

    fn first(&self) -> usize {
        self.glyphs.first().map_or(0, |g| g.2.start)
    }

    fn last(&self) -> usize {
        self.glyphs.last().map_or(0, |g| g.2.end)
    }

    /// The byte nearest a point `x` along the line: the nearest edge of a
    /// glyph, before it or after it. In running text one glyph's right edge
    /// is the next one's left; across a gap (a table's cells, a column's
    /// gutter) the press lands on the side it is nearer, not past the gap.
    fn offset_at(&self, x: f64) -> usize {
        let mut best = (f64::INFINITY, self.last());
        for (gx, w, r) in self.glyphs.iter() {
            for (edge, at) in [(*gx, r.start), (gx + w, r.end)] {
                let d = (x - edge).abs();
                if d < best.0 {
                    best = (d, at);
                }
            }
        }
        best.1
    }
}

/// The pages' glyphs as lines, in reading order: what a caret is placed,
/// drawn and moved by.
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    lines: Vec<Line>,
    /// Each page's top, in points down the pages laid end to end with no
    /// gap: how far apart two lines on different pages are.
    tops: Vec<f64>,
}

impl Geometry {
    /// The lines of pages `heights` points tall, whose glyphs are `glyphs`.
    pub fn new(pages: &[(f64, Vec<Glyph>)]) -> Geometry {
        let mut lines = Vec::new();
        let mut tops = Vec::with_capacity(pages.len());
        let mut top = 0.0;
        for (page, (height, glyphs)) in pages.iter().enumerate() {
            tops.push(top);
            top += height;
            lines.extend(Self::lines_of(page, glyphs, 0.0));
        }
        Geometry { lines, tops }
    }

    /// Pages whose lines are made already, `tops` down the pages laid end
    /// to end: a host that lays a document out in parts makes each part's
    /// lines once, and puts them together.
    pub fn from_lines(lines: Vec<Line>, tops: Vec<f64>) -> Geometry {
        Geometry { lines, tops }
    }

    /// The lines of `glyphs` on `page`, `dy` points further down it, in
    /// order down the page: a line is the glyphs within half a point of its
    /// first's baseline. Sorted, not searched for, so a tall reflowed page
    /// of thousands of lines is no slower than a sheet.
    pub fn lines_of(page: usize, glyphs: &[Glyph], dy: f64) -> Vec<Line> {
        let mut down: Vec<&Glyph> = glyphs.iter().filter(|g| g.source.is_some()).collect();
        down.sort_by(|a, b| a.baseline.total_cmp(&b.baseline));
        // Each line, and the glyphs on it as they are found.
        type Placed = (f64, f64, Range<usize>);
        let mut mine: Vec<(Line, Vec<Placed>)> = Vec::new();
        for g in down {
            let source = g.source.clone().expect("only sourced glyphs");
            let baseline = g.baseline + dy;
            if mine
                .last()
                .is_none_or(|(l, _)| (baseline - l.baseline).abs() >= 0.5)
            {
                let line = Line {
                    page,
                    baseline,
                    ascent: 0.0,
                    descent: 0.0,
                    glyphs: std::sync::Arc::new([]),
                    space: None,
                };
                mine.push((line, Vec::new()));
            }
            let (line, on) = mine.last_mut().expect("a line");
            if g.text == " " && line.space.is_none() {
                line.space = Some(g.width);
            }
            line.ascent = line.ascent.max(g.ascent);
            line.descent = line.descent.max(g.descent);
            on.push((g.x, g.width, source));
        }
        mine.into_iter()
            .map(|(mut l, mut on)| {
                on.sort_by(|a, b| a.0.total_cmp(&b.0));
                l.glyphs = on.into();
                l
            })
            .collect()
    }

    /// The line a caret at byte `offset` is drawn on, and its x there: before
    /// the glyph set from that byte, else after the one set up to it, else
    /// beside the glyph nearest it in the source.
    fn caret(&self, offset: usize) -> Option<(usize, f64)> {
        self.caret_at(offset).map(|(i, x, _)| (i, x))
    }

    /// [`Self::caret`], and the byte the x stands for: `offset` itself, or
    /// the nearest glyph's edge.
    fn caret_at(&self, offset: usize) -> Option<(usize, f64, usize)> {
        let mut after = None;
        let mut nearest: Option<(usize, usize, f64, usize)> = None;
        for (i, line) in self.lines.iter().enumerate() {
            for (x, w, r) in line.glyphs.iter() {
                if r.start == offset {
                    return Some((i, *x, offset));
                }
                if r.end == offset && after.is_none() {
                    after = Some((i, x + w, offset));
                }
                let (gap, at, from) = if offset < r.start {
                    (r.start - offset, *x, r.start)
                } else {
                    (offset.saturating_sub(r.end), x + w, r.end)
                };
                if nearest.is_none_or(|(g, ..)| gap < g) {
                    nearest = Some((gap, i, at, from));
                }
            }
        }
        after.or(nearest.map(|(_, i, x, from)| (i, x, from)))
    }

    /// Where a caret at `offset` in `source` is drawn. Spaces between the
    /// last glyph and the caret move it on a space's width each: a space at
    /// the end of a paragraph is no content to the parser, so Typst sets no
    /// glyph for it, and the caret after it would otherwise stay after the
    /// letter before.
    pub fn caret_box(&self, offset: usize, source: &str) -> Option<PageBox> {
        let (i, mut x, from) = self.caret_at(offset)?;
        let l = &self.lines[i];
        if let Some(gap) = source.get(from..offset)
            && !gap.is_empty()
            && gap.bytes().all(|b| b == b' ')
        {
            x += gap.len() as f64 * l.space();
        }
        Some(PageBox {
            page: l.page,
            x,
            top: l.baseline - l.ascent,
            width: 0.0,
            height: l.ascent + l.descent,
        })
    }

    /// Where a caret at `offset` in `now` is drawn on pages set from
    /// `laid`, an earlier source: through the edit between them, so that
    /// the caret keeps up with typing until the pages of the edit are set.
    /// A caret in what was typed is drawn after the text before it, moved
    /// on by an estimate of the typed text's width.
    pub fn caret_through(&self, offset: usize, now: &str, laid: &str) -> Option<PageBox> {
        if now == laid || laid.is_empty() {
            return self.caret_box(offset, now);
        }
        let (pre, suf) = edit_of(now, laid);
        if offset <= pre {
            return self.caret_box(offset, laid);
        }
        // Right after what was typed is still in it: the typed text has no
        // glyphs yet.
        if offset > now.len() - suf {
            return self.caret_box(offset + laid.len() - now.len(), laid);
        }
        let mut b = self.caret_box(pre, laid)?;
        let (i, ..) = self.caret_at(pre)?;
        let l = &self.lines[i];
        let n = l.glyphs.len().max(1) as f64;
        let letter = l.glyphs.iter().map(|g| g.1).sum::<f64>() / n;
        let letter = if letter > 0.0 {
            letter
        } else {
            l.space() * 2.0
        };
        for c in now[pre..offset].chars() {
            b.x += if c == ' ' { l.space() } else { letter };
        }
        Some(b)
    }

    /// The boxes a selection of bytes `lo..hi` covers: one a line.
    pub fn selection(&self, lo: usize, hi: usize) -> Vec<PageBox> {
        let mut boxes = Vec::new();
        for l in &self.lines {
            let inside: Vec<_> = l
                .glyphs
                .iter()
                .filter(|(_, _, r)| r.start < hi && r.end > lo)
                .collect();
            let (Some(first), Some(last)) = (inside.first(), inside.last()) else {
                continue;
            };
            boxes.push(PageBox {
                page: l.page,
                x: first.0,
                top: l.baseline - l.ascent,
                width: last.0 + last.1 - first.0,
                height: l.ascent + l.descent,
            });
        }
        boxes
    }

    /// The box of the object set from `span` (a picture, a host's room:
    /// twig-export's object glyph), if it is on the pages.
    pub fn object(&self, span: Range<usize>) -> Option<PageBox> {
        self.lines.iter().find_map(|l| {
            let (x, w, _) = l.glyphs.iter().find(|g| g.2 == span)?;
            Some(PageBox {
                page: l.page,
                x: *x,
                top: l.baseline - l.ascent,
                width: *w,
                height: l.ascent + l.descent,
            })
        })
    }

    /// The line a press at (`x`, `y`) on `page`, in its points, lands on,
    /// and the byte: the line whose glyphs' height it is within, else the
    /// nearest. Below the document's last line is its end, and above its
    /// first line its start, wherever across.
    pub fn hit(&self, page: usize, x: f64, y: f64) -> Option<(usize, usize)> {
        let (i, line) = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.page == page)
            .min_by(|a, b| distance(a.1, y).total_cmp(&distance(b.1, y)))?;
        let at = if i + 1 == self.lines.len() && y > line.baseline + line.descent {
            line.last()
        } else if i == 0 && y < line.baseline - line.ascent {
            line.first()
        } else {
            line.offset_at(x)
        };
        Some((i, at))
    }

    /// The byte a line down or up from `offset`, aiming for `goal` (the x of
    /// `offset` if none): the next line's nearest byte, or past the end of
    /// the last line, or before the first.
    pub fn vertical(&self, offset: usize, down: bool, goal: Option<f64>) -> Option<(usize, f64)> {
        let (i, x) = self.caret(offset)?;
        let goal = goal.unwrap_or(x);
        let to = if down {
            i + 1
        } else {
            match i.checked_sub(1) {
                Some(to) => to,
                None => return Some((self.lines[i].first(), goal)),
            }
        };
        Some(match self.lines.get(to) {
            Some(l) => (l.offset_at(goal), goal),
            None => (self.lines[i].last(), goal),
        })
    }

    /// The byte `span` points down or up from `offset`, on the line nearest
    /// it, aiming for `goal`.
    pub fn page(
        &self,
        offset: usize,
        down: bool,
        span: f64,
        goal: Option<f64>,
    ) -> Option<(usize, f64)> {
        let (i, x) = self.caret(offset)?;
        let goal = goal.unwrap_or(x);
        let here = self.y(i);
        let target = if down { here + span } else { here - span };
        let to = (0..self.lines.len()).min_by(|&a, &b| {
            (self.y(a) - target)
                .abs()
                .total_cmp(&(self.y(b) - target).abs())
        })?;
        Some((self.lines[to].offset_at(goal), goal))
    }

    /// The first or the last byte of the line `offset` is on.
    pub fn line_edge(&self, offset: usize, end: bool) -> Option<usize> {
        let (i, _) = self.caret(offset)?;
        let l = &self.lines[i];
        Some(if end { l.last() } else { l.first() })
    }

    /// Line `i`'s baseline, in points down the pages laid end to end.
    fn y(&self, i: usize) -> f64 {
        let l = &self.lines[i];
        self.tops[l.page] + l.baseline
    }
}

/// The edit between two sources: how many bytes they share at the start,
/// and at the end, on character boundaries.
fn edit_of(now: &str, laid: &str) -> (usize, usize) {
    let pre = now
        .char_indices()
        .zip(laid.chars())
        .find(|((_, a), b)| a != b)
        .map_or(now.len().min(laid.len()), |((i, _), _)| i);
    let most = now.len().min(laid.len()) - pre;
    let suf = now[pre..]
        .chars()
        .rev()
        .zip(laid[pre..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .scan(0, |n, l| {
            *n += l;
            Some(*n)
        })
        .take_while(|&n| n <= most)
        .last()
        .unwrap_or(0);
    (pre, suf)
}

/// Byte `offset` of `now` in `laid`, an earlier source: the same before the
/// edit between them, moved by it after, and the edit's start within it.
pub fn through(offset: usize, now: &str, laid: &str) -> usize {
    if now == laid || laid.is_empty() {
        return offset;
    }
    let (pre, suf) = edit_of(now, laid);
    if offset <= pre {
        offset
    } else if offset >= now.len() - suf {
        offset + laid.len() - now.len()
    } else {
        pre
    }
}

/// How far `y` is from a line's glyphs, top to bottom: none within them.
fn distance(l: &Line, y: f64) -> f64 {
    let (top, bottom) = (l.baseline - l.ascent, l.baseline + l.descent);
    if y < top {
        top - y
    } else if y > bottom {
        y - bottom
    } else {
        0.0
    }
}

/// The page view's lines, which the editor moves its caret by while the
/// view is shown: the latest pages' [`Geometry`], and how many points a
/// screen of the view shows.
#[derive(Default)]
pub struct PageLines {
    pub geometry: RefCell<Rc<Geometry>>,
    pub screen: Cell<f64>,
}

impl leaf_dioxus::Lines for PageLines {
    fn vertical(&self, doc: &mut Doc, down: bool, extend: bool, goal: &Cell<Option<f32>>) {
        let g = self.geometry.borrow().clone();
        if let Some((to, x)) = g.vertical(doc.caret, down, goal.get().map(f64::from)) {
            doc.place_caret(to, extend);
            goal.set(Some(x as f32));
        }
    }

    fn page(&self, doc: &mut Doc, down: bool, extend: bool, goal: &Cell<Option<f32>>) {
        let g = self.geometry.borrow().clone();
        let span = self.screen.get().max(1.0);
        if let Some((to, x)) = g.page(doc.caret, down, span, goal.get().map(f64::from)) {
            doc.place_caret(to, extend);
            goal.set(Some(x as f32));
        }
    }

    fn line_edge(&self, doc: &mut Doc, end: bool, extend: bool) {
        let g = self.geometry.borrow().clone();
        if let Some(to) = g.line_edge(doc.caret, end) {
            doc.place_caret(to, extend);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A press past a table cell's text, nearer it than the next cell's,
    /// lands at its end, not in the next cell.
    #[test]
    fn a_press_past_a_cells_text_stays_in_the_cell() {
        let glyph = |x: f64, r: std::ops::Range<usize>| Glyph {
            x,
            baseline: 20.0,
            width: 5.0,
            ascent: 8.0,
            descent: 2.0,
            text: "a".into(),
            source: Some(r),
        };
        // "ab" at 0..10, then a cell away, "c" at 60.
        let g = Geometry::new(&[(
            100.0,
            vec![glyph(0.0, 2..3), glyph(5.0, 3..4), glyph(60.0, 9..10)],
        )]);
        assert_eq!(g.hit(0, 14.0, 18.0), Some((0, 4)), "after b");
        assert_eq!(g.hit(0, 55.0, 18.0), Some((0, 9)), "before c");
        assert_eq!(g.hit(0, 6.0, 18.0), Some((0, 3)), "between a and b");
    }

    /// A press below the last page, or between two, lands on the nearest.
    #[test]
    fn a_press_off_the_pages_lands_on_the_nearest() {
        let sheets = vec![
            Sheet {
                width: 100.0,
                height: 100.0,
            },
            Sheet {
                width: 100.0,
                height: 100.0,
            },
        ];
        let spread = Spread::of(&sheets, 400.0, false);
        let below = spread.height + 500.0;
        let (page, _, py) = spread.nearest(&sheets, 50.0, below).unwrap();
        assert_eq!((page, py), (1, 100.0));
        let gap = spread.tops[1] - 1.0;
        assert_eq!(spread.nearest(&sheets, 50.0, gap).unwrap().0, 1);
        let reflowed = Spread::of(&sheets[..1], 400.0, true);
        assert_eq!(
            reflowed.nearest(&sheets[..1], 10.0, 9999.0).unwrap().2,
            100.0
        );
    }

    /// Through an edit, a byte before it is where it was, one after it is
    /// moved by it, and one in what was typed is the edit's start.
    #[test]
    fn a_byte_is_found_through_an_edit() {
        let (laid, now) = ("a b", "a bcd e");
        assert_eq!(edit_of(now, laid), (3, 0));
        assert_eq!(through(2, now, laid), 2);
        assert_eq!(through(5, now, laid), 3);
        let (laid, now) = ("one two", "one xx two");
        assert_eq!(edit_of(now, laid), (4, 3));
        assert_eq!(through(9, now, laid), 6);
        assert_eq!(through(5, now, laid), 4);
        // Repeated letters: the shared end is not counted twice.
        assert_eq!(edit_of("aaa", "aa"), (2, 0));
        assert_eq!(edit_of("é!", "é"), (2, 0));
    }

    /// Typing a space and a letter on stale pages moves the caret on, not
    /// back to before the space.
    #[test]
    fn the_caret_keeps_up_with_typing_on_stale_pages() {
        let glyph = |x: f64, r: std::ops::Range<usize>, text: &str| Glyph {
            x,
            baseline: 10.0,
            width: 5.0,
            ascent: 8.0,
            descent: 2.0,
            text: text.into(),
            source: Some(r),
        };
        let g = Geometry::new(&[(100.0, vec![glyph(0.0, 0..1, "a")])]);
        let space = g.caret_through(2, "a ", "a").unwrap().x;
        let letter = g.caret_through(3, "a b", "a").unwrap().x;
        assert!(space > 5.0, "{space}");
        assert!(letter > space, "{letter} after {space}");
    }

    fn glyph(x: f64, baseline: f64, source: Option<Range<usize>>) -> Glyph {
        Glyph {
            x,
            baseline,
            width: 5.0,
            ascent: 8.0,
            descent: 2.0,
            text: "a".into(),
            source,
        }
    }

    /// Two lines on one page, "ab" (bytes 0..2) and "cd" (bytes 3..5), and
    /// a bullet that names nothing.
    fn two_lines() -> Geometry {
        Geometry::new(&[(
            100.0,
            vec![
                glyph(15.0, 20.0, Some(1..2)),
                glyph(10.0, 20.0, Some(0..1)),
                glyph(2.0, 40.0, None),
                glyph(10.0, 40.0, Some(3..4)),
                glyph(15.0, 40.0, Some(4..5)),
            ],
        )])
    }

    #[test]
    fn a_caret_is_drawn_before_its_byte_or_after_the_last() {
        let g = two_lines();
        let src = "ab\ncd\n";
        let b = g.caret_box(1, src).unwrap();
        assert_eq!((b.x, b.top, b.height), (15.0, 12.0, 10.0));
        // After the line's last glyph, on that line.
        assert_eq!(g.caret_box(2, src).unwrap().x, 20.0);
        assert_eq!(g.caret_box(3, src).unwrap().top, 32.0);
    }

    /// A space typed at a paragraph's end has no glyph; the caret after it
    /// is still drawn a space on.
    #[test]
    fn a_caret_after_a_trailing_space_is_a_space_on() {
        let g = Geometry::new(&[(
            100.0,
            vec![glyph(10.0, 20.0, Some(0..1)), glyph(15.0, 20.0, Some(1..2))],
        )]);
        let src = "ab  ";
        let after_b = g.caret_box(2, src).unwrap().x;
        let one = g.caret_box(3, src).unwrap().x;
        let two = g.caret_box(4, src).unwrap().x;
        assert!(one > after_b, "{after_b} {one}");
        assert!((two - one - (one - after_b)).abs() < 1e-9, "{one} {two}");
    }

    #[test]
    fn a_press_lands_on_the_nearest_byte_of_its_line() {
        let g = two_lines();
        assert_eq!(g.hit(0, 11.0, 18.0), Some((0, 0)));
        assert_eq!(g.hit(0, 13.0, 18.0), Some((0, 1)));
        assert_eq!(g.hit(0, 90.0, 18.0), Some((0, 2)));
        // Below every line: the last line's end, wherever across; above
        // every line, the first's start.
        assert_eq!(g.hit(0, 0.0, 90.0), Some((1, 5)));
        assert_eq!(g.hit(0, 90.0, 1.0), Some((0, 0)));
    }

    #[test]
    fn up_and_down_keep_the_goal_and_stop_at_the_ends() {
        let g = two_lines();
        assert_eq!(g.vertical(1, true, None), Some((4, 15.0)));
        assert_eq!(g.vertical(4, false, Some(15.0)), Some((1, 15.0)));
        // Down from the last line: its end; up from the first: its start.
        assert_eq!(g.vertical(4, true, None), Some((5, 15.0)));
        assert_eq!(g.vertical(1, false, None), Some((0, 15.0)));
        assert_eq!(g.line_edge(4, false), Some(3));
        assert_eq!(g.line_edge(3, true), Some(5));
    }

    #[test]
    fn a_selection_is_a_box_a_line() {
        let g = two_lines();
        let boxes = g.selection(1, 4);
        assert_eq!(boxes.len(), 2);
        assert_eq!((boxes[0].x, boxes[0].width), (15.0, 5.0));
        assert_eq!((boxes[1].x, boxes[1].width), (10.0, 5.0));
    }

    #[test]
    fn a_point_is_found_on_its_page() {
        let sheets = vec![
            Sheet {
                width: 100.0,
                height: 100.0,
            };
            2
        ];
        let spread = Spread::of(&sheets, 2.0 * PAD + 200.0, false);
        assert_eq!(spread.scale, 2.0);
        assert_eq!(
            spread.on_page(&sheets, PAD + 20.0, PAD + 10.0),
            Some((0, 10.0, 5.0))
        );
        let second = spread.tops[1];
        assert_eq!(
            spread.on_page(&sheets, PAD, second + 4.0),
            Some((1, 0.0, 2.0))
        );
    }
}
