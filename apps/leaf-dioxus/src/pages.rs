//! The pages, drawn: the document as its PDF sets it, with the caret and
//! the selection on Typst's own glyphs, and a press on a page put into the
//! text. Beside the editor (the preview) or in its place (the page view).
//!
//! Each page is twig-export's (`pdf::layout`): a picture of it, and its
//! glyphs, each with the bytes of the source it was set from. [`Geometry`]
//! reads the glyphs as lines: where a caret at a byte is drawn, which byte a
//! point on a page lands on, the byte a line up or down, a line's edges, and
//! the boxes a selection covers. The document is still the editor's: a
//! press here is the editor's own pointer event, at the byte it landed on
//! ([`leaf_dioxus::Target::Offset`]), so a double-click selects a word, a
//! ⌘-click follows a link and a right-click opens the context menu as they
//! do in the editor; its keys still edit; and in the page view its moves by
//! line go by these lines ([`PageLines`], the editor's
//! [`leaf_dioxus::Lines`]).
//!
//! A caret the editor places where no glyph is (an empty paragraph, a
//! line's hidden markup) is drawn beside the nearest glyph of the source.
//! The view scrolls and zooms itself (`unit::scroll`), as the editor
//! scrolls, rather than leave it to Blitz: so that it can keep the caret in
//! view, and zoom about the pointer.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use leaf_core::Doc;
use leaf_dioxus::{Pointer, Target, press_mode};
use twig_export::pdf::Glyph;

use crate::app::{self, AppEvent};

/// Space around the pages and between them, in CSS pixels.
const PAD: f64 = 16.0;
/// The widest a page is drawn, in CSS pixels, however wide the view, at
/// zoom 1.
const WIDEST: f64 = 920.0;

/// The least and the most the pages zoom.
pub const ZOOMS: (f64, f64) = (0.25, 4.0);

/// How far a zoom command zooms.
pub const ZOOM_STEP: f64 = 1.25;

/// A page as set: a picture of it, its size in points, and its glyphs.
#[derive(Clone, Debug)]
pub struct Sheet {
    /// A `data:` URL of the page's SVG.
    pub url: Rc<str>,
    pub width: f64,
    pub height: f64,
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
struct Line {
    page: usize,
    baseline: f64,
    ascent: f64,
    descent: f64,
    glyphs: Vec<(f64, f64, Range<usize>)>,
    /// How wide a space is on the line: one Typst set there, or a quarter
    /// of an em, the em guessed from the line's height.
    space: Option<f64>,
}

impl Line {
    fn space(&self) -> f64 {
        self.space.unwrap_or(0.22 * (self.ascent + self.descent))
    }

    fn first(&self) -> usize {
        self.glyphs.first().map_or(0, |g| g.2.start)
    }

    fn last(&self) -> usize {
        self.glyphs.last().map_or(0, |g| g.2.end)
    }

    /// The byte nearest a point `x` along the line: before the glyph whose
    /// left half it is in, after the last if it is past them all.
    fn offset_at(&self, x: f64) -> usize {
        self.glyphs
            .iter()
            .find(|(gx, w, _)| x < gx + w / 2.0)
            .map_or(self.last(), |g| g.2.start)
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
        let mut lines: Vec<Line> = Vec::new();
        let mut tops = Vec::with_capacity(pages.len());
        let mut top = 0.0;
        for (page, (height, glyphs)) in pages.iter().enumerate() {
            tops.push(top);
            top += height;
            let mut mine: Vec<Line> = Vec::new();
            for g in glyphs {
                let Some(source) = g.source.clone() else {
                    continue;
                };
                let line = match mine
                    .iter_mut()
                    .find(|l| (l.baseline - g.baseline).abs() < 0.5)
                {
                    Some(l) => l,
                    None => {
                        mine.push(Line {
                            page,
                            baseline: g.baseline,
                            ascent: 0.0,
                            descent: 0.0,
                            glyphs: Vec::new(),
                            space: None,
                        });
                        mine.last_mut().expect("just pushed")
                    }
                };
                if g.text == " " && line.space.is_none() {
                    line.space = Some(g.width);
                }
                line.ascent = line.ascent.max(g.ascent);
                line.descent = line.descent.max(g.descent);
                line.glyphs.push((g.x, g.width, source));
            }
            for l in &mut mine {
                l.glyphs.sort_by(|a, b| a.0.total_cmp(&b.0));
            }
            mine.sort_by(|a, b| a.baseline.total_cmp(&b.baseline));
            lines.extend(mine);
        }
        Geometry { lines, tops }
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
            for (x, w, r) in &line.glyphs {
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

    /// The line a press at (`x`, `y`) on `page`, in its points, lands on,
    /// and the byte: the line whose glyphs' height it is within, else the
    /// nearest.
    pub fn hit(&self, page: usize, x: f64, y: f64) -> Option<(usize, usize)> {
        let (i, line) = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.page == page)
            .min_by(|a, b| distance(a.1, y).total_cmp(&distance(b.1, y)))?;
        Some((i, line.offset_at(x)))
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

/// What the view draws: where it is, the pages, the caret and selection,
/// and how far it is scrolled.
#[derive(Clone, Debug, Default)]
pub struct PagesView {
    /// The view's left and width in the window, in CSS pixels; no width
    /// while it is hidden.
    pub left: f64,
    pub width: f64,
    /// How far up from the window's bottom the view stops: the editor's
    /// status line, under the page view.
    pub bottom: f64,
    /// Whether it is the page view, which is the editor's place, rather than
    /// the preview beside it.
    pub editing: bool,
    /// Whether the page is reflowed to the view: drawn at its true size, on
    /// the editor's own ground, with no desk round it.
    pub reflow: bool,
    pub sheets: Vec<Sheet>,
    pub geometry: Rc<Geometry>,
    /// Why the document as it stands would not set, over the pages it last
    /// set as.
    pub error: Option<String>,
    pub caret: Option<PageBox>,
    pub selection: Vec<PageBox>,
    /// Where the pages are scrolled to and how far they are zoomed: the
    /// pages laid out at the view's width ([`Spread`]) are the content at
    /// zoom 1.
    pub scroll: unit::scroll::Scroll,
}

impl PagesView {
    /// The view's height, in a window `height` CSS pixels tall.
    pub fn height(&self, window: f64) -> f64 {
        (window - crate::page::chrome_height() - self.bottom).max(0.0)
    }

    /// The pages placed down the view.
    pub fn spread(&self) -> Spread {
        Spread::of(&self.sheets, self.width, self.reflow)
    }

    /// Bring the scroll up to date with the view's size and the pages', in
    /// a window `window` CSS pixels tall, and keep it within them.
    pub fn fit(&mut self, window: f64) {
        let spread = self.spread();
        let height = self.height(window);
        let s = &mut self.scroll;
        s.view = (self.width, height);
        s.content = (self.width, spread.height);
        s.zooms = ZOOMS;
        s.zoom = s.zoom.clamp(ZOOMS.0, ZOOMS.1);
        s.clamp();
    }

    /// A box on a page as the content's, at zoom 1: left, top, width,
    /// height.
    pub fn content_box(&self, b: &PageBox) -> (f64, f64, f64, f64) {
        let spread = self.spread();
        let s = spread.scale;
        let x = spread.lefts.get(b.page).copied().unwrap_or(PAD) + b.x * s;
        let y = spread.tops.get(b.page).copied().unwrap_or(PAD) + b.top * s;
        (x, y, b.width * s, b.height * s)
    }

    /// Scroll just far enough that `b` is in view, with a few lines' room.
    pub fn reveal(&mut self, b: PageBox) {
        let rect = self.content_box(&b);
        let margin = 3.0 * rect.3 * self.scroll.zoom;
        self.scroll.reveal(rect, margin);
    }
}

/// The view's look: a desk the pages lie on, in both appearances, under
/// the toolbar and over whatever of the editor reaches past its box.
pub const CSS: &str = "
:root {
  --desk-bg: #e4e4e7;
  --desk-text: #3f3f46;
}
@media (prefers-color-scheme: dark) {
  :root {
    --desk-bg: #232326;
    --desk-text: #d4d4d8;
  }
}
.pages { position: fixed; z-index: 2; box-sizing: border-box; overflow: hidden;
         background: var(--desk-bg); color: var(--desk-text);
         border-left: 1px solid var(--field-border); }
.pages .spread { position: absolute; }
.pages.reflow { background: var(--leaf-bg); border-left: none; }
.pages.reflow .sheet { background: transparent; box-shadow: none; }
.pages .sheet { position: absolute; display: block; background: white;
                box-shadow: 0 1px 4px rgba(0, 0, 0, 0.3); }
.pages .caret { position: absolute; width: 1.5px; background: #1d4ed8; }
.pages .selected { position: absolute; background: rgba(37, 99, 235, 0.25); }
.pages .note { position: absolute; left: 16px; right: 16px; top: 8px; z-index: 1;
               font: 12px system-ui, -apple-system, sans-serif; white-space: pre-wrap; }";

#[derive(Props, Clone, PartialEq)]
pub struct PagesProps {
    view: Signal<PagesView>,
}

/// The pages, while they are shown, with the caret and the selection over
/// them.
#[allow(non_snake_case)]
pub fn Pages(props: PagesProps) -> Element {
    let mut view = props.view;
    let id = dioxus_native::use_window().id();
    let v = view.read();
    if v.width <= 0.0 {
        return rsx! {};
    }
    let top = crate::page::chrome_height();
    let (left, width, bottom) = (v.left, v.width, v.bottom);
    let spread = v.spread();
    let s = spread.scale;
    let note = match (&v.error, v.sheets.is_empty()) {
        (Some(e), _) => Some(format!("This version will not set as a PDF:\n{e}")),
        (None, true) => Some("Setting the pages…".to_string()),
        (None, false) => None,
    };
    let z = v.scroll.zoom;
    let at = |b: &PageBox| {
        let (x, y, w, h) = v.content_box(b);
        (x * z, y * z, w * z, h * z)
    };
    let caret = v.caret.map(|b| at(&b));
    let selection: Vec<_> = v.selection.iter().map(at).collect();
    let (ox, oy) = (v.scroll.x, v.scroll.y);
    let (total_w, total_h) = (width * z, spread.height * z);
    // A press, or a drag, at a point in the window: the line and the byte it
    // lands on, as the editor's target.
    let land = move |x: f64, y: f64| -> Option<(Target, usize)> {
        let v = view.peek();
        let spread = v.spread();
        let (cx, cy) = v.scroll.to_content(x - v.left, y - top);
        let (page, px, py) = spread.on_page(&v.sheets, cx, cy)?;
        let (line, at) = v.geometry.hit(page, px, py)?;
        Some((Target::Offset(at), line))
    };
    rsx! {
        div { class: if v.reflow { "pages reflow" } else { "pages" }, role: "region",
            aria_label: if v.reflow { "Document" } else if v.editing { "Pages" } else { "PDF preview" },
            style: "left: {left}px; top: {top}px; width: {width}px; bottom: {bottom}px",
            // The keys stay with the editor; a press puts its caret here.
            onmousedown: move |e: MouseEvent| {
                e.prevent_default();
                let p = e.client_coordinates();
                if let Some((at, line)) = land(p.x, p.y) {
                    let (extend, mode) = press_mode(&e);
                    point(id, Pointer::Press { at, line, x: 0.0, extend, mode });
                }
            },
            onmousemove: move |e: MouseEvent| {
                if e.held_buttons().contains(MouseButton::Primary) {
                    let p = e.client_coordinates();
                    if let Some((at, line)) = land(p.x, p.y) {
                        point(id, Pointer::Drag { at, line, x: 0.0 });
                    }
                }
            },
            // A finger's tap, which sends no press (the editor's `touch.rs`).
            onclick: move |e: MouseEvent| {
                let p = e.client_coordinates();
                if let Some((at, line)) = land(p.x, p.y) {
                    point(id, Pointer::Tap { at, line, x: 0.0 });
                }
            },
            // The wheel scrolls, as the editor's does; with ⌃ (or ⌘), as a
            // pinch does, it zooms about the pointer.
            onwheel: move |e: WheelEvent| {
                e.prevent_default();
                let window = crate::app::page(id).map_or(800.0, |p| p.size().1);
                let mut v = view.write();
                v.fit(window);
                let (dx, dy) = unit::scroll::wheel_delta(e.delta(), v.scroll.view.1);
                let m = e.modifiers();
                if m.ctrl() || m.meta() {
                    let p = e.client_coordinates();
                    let (px, py) = (p.x - v.left, p.y - top);
                    v.scroll.zoom_at(unit::scroll::zoom_factor(dy), px, py);
                } else {
                    v.scroll.wheel(dx, dy);
                }
            },
            if let Some(n) = note {
                div { class: "note", "{n}" }
            }
            // Down the right edge, and along the bottom once the pages are
            // zoomed wider than the view.
            unit::ScrollBar { axis: unit::Axis::Vertical, scroll: v.scroll,
                label: "Pages".to_string(),
                onscroll: move |y| {
                    let mut v = view.write();
                    v.scroll.y = y;
                    v.scroll.clamp();
                },
            }
            unit::ScrollBar { axis: unit::Axis::Horizontal, scroll: v.scroll,
                label: "Pages across".to_string(),
                onscroll: move |x| {
                    let mut v = view.write();
                    v.scroll.x = x;
                    v.scroll.clamp();
                },
            }
            div { class: "spread", style: "left: {-ox}px; top: {-oy}px; width: {total_w}px; height: {total_h}px",
                for (i, sheet) in v.sheets.iter().enumerate() {
                    img { key: "{i}", class: "sheet", src: "{sheet.url}", alt: "Page {i + 1}",
                        style: "left: {spread.lefts[i] * z}px; top: {spread.tops[i] * z}px; width: {sheet.width * s * z}px; height: {sheet.height * s * z}px",
                    }
                }
                for (i, (x, y, w, h)) in selection.into_iter().enumerate() {
                    div { key: "s{i}", class: "selected",
                        style: "left: {x}px; top: {y}px; width: {w}px; height: {h}px" }
                }
                if let Some((x, y, _, h)) = caret {
                    div { class: "caret", style: "left: {x}px; top: {y}px; height: {h}px" }
                }
            }
        }
    }
}

/// Hand window `id`'s editor a pointer event on the pages, on the handler's
/// turn: everything after finding the byte is the editor's.
fn point(id: dioxus_native::winit::window::WindowId, p: Pointer) {
    app::post(AppEvent::Run(Box::new(move || {
        if let Some(page) = app::page(id) {
            page.with_editor(|ed, _| ed.pointer(p));
        }
    })));
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Below every line: the last.
        assert_eq!(g.hit(0, 0.0, 90.0), Some((1, 3)));
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

    /// While the page view gives the editor its lines, ↓, End and ↑ go by
    /// the lines twig-export set, not the editor's rows.
    #[test]
    fn the_editors_keys_move_by_the_pages_lines() {
        use dioxus::html::{Code, Key, Modifiers, NamedKey};
        use dioxus_core::ScopeId;

        let src = "First line.\n\nSecond line, longer.\n";
        let layout = crate::pdf::layout(
            src,
            leaf_core::Format::Markdown,
            None,
            &crate::pdf::Type {
                template: crate::settings::Template::default(),
                size: 12.0,
                dark: false,
            },
            crate::pdf::Sheet::Paper(crate::pdf::Paper {
                width: 612.0,
                height: 792.0,
                margin: 72.0,
            }),
        )
        .expect("a layout");
        let pages: Vec<_> = layout
            .pages()
            .into_iter()
            .map(|p| (p.height, p.glyphs))
            .collect();
        let lines = Rc::new(PageLines::default());
        *lines.geometry.borrow_mut() = Rc::new(Geometry::new(&pages));
        let (document, ed, _) = crate::sheet::offscreen(
            src,
            leaf_core::Format::Markdown,
            leaf_dioxus::Look::default(),
            Some((800.0, 600.0)),
        )
        .expect("an editor");
        document.vdom.runtime().in_scope(ScopeId::ROOT, || {
            ed.set_lines(Some(lines.clone()));
            ed.doc().select_range(0, 0);
            let press = |key: NamedKey, code: Code| {
                ed.key(Key::Named(key), code, Modifiers::empty());
                ed.doc().caret
            };
            let second = src.find("Second").unwrap();
            assert_eq!(press(NamedKey::ArrowDown, Code::ArrowDown), second);
            let end = second + "Second line, longer.".len();
            assert_eq!(press(NamedKey::End, Code::End), end);
            assert_eq!(press(NamedKey::ArrowUp, Code::ArrowUp), "First line.".len());
        });
    }

    /// A press on the pages is the editor's own: placed at the byte the
    /// page found, and a second on the same line selects the word, by the
    /// editor's counting.
    #[test]
    fn a_press_on_the_pages_is_the_editors() {
        use dioxus_core::ScopeId;
        use leaf_dioxus::PressMode;

        let src = "Some words here.\n";
        let (document, ed, _) = crate::sheet::offscreen(
            src,
            leaf_core::Format::Markdown,
            leaf_dioxus::Look::default(),
            Some((800.0, 600.0)),
        )
        .expect("an editor");
        let words = src.find("words").unwrap();
        let press = Pointer::Press {
            at: Target::Offset(words + 2),
            line: 0,
            x: 0.0,
            extend: false,
            mode: PressMode::Place,
        };
        document.vdom.runtime().in_scope(ScopeId::ROOT, || {
            ed.pointer(press);
            assert_eq!(ed.doc().caret, words + 2);
            ed.pointer(press);
            assert_eq!(ed.doc().selection(), Some((words, words + "words".len())));
        });
    }

    /// Zoomed in, the pages scroll across as well as down; a caret on a
    /// later page is brought into view at the zoom; and a zoom about a point
    /// keeps that point of the pages where it was.
    #[test]
    fn the_pages_zoom_and_keep_the_caret_in_view() {
        let sheet = Sheet {
            url: "".into(),
            width: 600.0,
            height: 800.0,
        };
        let mut v = PagesView {
            width: 640.0,
            sheets: vec![sheet.clone(), sheet],
            ..PagesView::default()
        };
        let window = crate::page::chrome_height() + 500.0;
        v.fit(window);
        assert_eq!(v.scroll.view, (640.0, 500.0));
        assert!(v.scroll.zoom_at(2.0, 320.0, 250.0));
        let ((x0, x1), _) = v.scroll.range();
        assert!(x1 > x0, "zoomed past the view's width: {x0}..{x1}");
        let under = v.scroll.to_content(100.0, 100.0);
        v.scroll.zoom_at(1.5, 100.0, 100.0);
        let back = v.scroll.to_view(under.0, under.1);
        assert!((back.0 - 100.0).abs() < 1e-6 && (back.1 - 100.0).abs() < 1e-6);
        let caret = PageBox {
            page: 1,
            x: 300.0,
            top: 400.0,
            width: 0.0,
            height: 12.0,
        };
        v.reveal(caret);
        let (x, y, _, h) = v.content_box(&caret);
        let (vx, vy) = v.scroll.to_view(x, y);
        let z = v.scroll.zoom;
        assert!((0.0..=640.0).contains(&vx), "{vx}");
        assert!(vy >= 0.0 && vy + h * z <= 500.0, "{vy}");
    }

    #[test]
    fn a_point_is_found_on_its_page() {
        let sheets = vec![
            Sheet {
                url: "".into(),
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
