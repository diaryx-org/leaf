//! The pages, drawn: the document as Typst sets it, a picture a block,
//! with the caret and the selection on Typst's own glyphs, and a press on a
//! page put into the text. In the editor's place (the page view, the
//! reflowed view) or beside it (the preview of the source).
//!
//! The document is still the editor's: a press here is the editor's own
//! pointer event, at the byte it landed on ([`leaf_dioxus::Target::Offset`]),
//! so a double-click selects a word, a ⌘-click follows a link and a
//! right-click opens the context menu as they do in the editor; its keys
//! still edit. The view scrolls and zooms itself (`unit::scroll`), as the
//! editor scrolls, rather than leave it to Blitz: so that it can keep the
//! caret in view, and zoom about the pointer.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use leaf_dioxus::{Pointer, Target, press_mode};

use crate::Pages;
use crate::geometry::{Geometry, PAD, PageBox, Piece, Sheet, Spread};

/// The least and the most the pages zoom.
pub const ZOOMS: (f64, f64) = (0.25, 4.0);

/// How far a zoom command zooms.
pub const ZOOM_STEP: f64 = 1.25;

/// What the view draws: where it is, the pages, the caret and selection,
/// and how far it is scrolled.
#[derive(Clone, Debug, Default)]
pub struct PagesView {
    /// How far down the window the view's area starts, and where it ends:
    /// the host's ([`crate::Area`]), below its own bars.
    pub area_top: f64,
    pub floor: f64,
    /// The view's left and width in the window, in CSS pixels; no width
    /// while it is hidden.
    pub left: f64,
    pub width: f64,
    /// How far up from the area's bottom the view stops: the editor's
    /// status line, under the page view.
    pub bottom: f64,
    /// How far down from the area's top the view starts: the editor's
    /// find bar, while it is open over the page view.
    pub inset: f64,
    /// Whether it is the page view, which is the editor's place, rather than
    /// the preview beside it.
    pub editing: bool,
    /// Whether the page is reflowed to the view: drawn at its true size, on
    /// the editor's own ground, with no desk round it.
    pub reflow: bool,
    pub sheets: Vec<Sheet>,
    /// What is drawn on them: the view draws those near the screen.
    pub pieces: Vec<Piece>,
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
    /// The view's height.
    pub fn height(&self) -> f64 {
        (self.floor - self.top() - self.bottom).max(0.0)
    }

    /// The view's top in the window: the area's, under the find bar.
    pub fn top(&self) -> f64 {
        self.area_top + self.inset
    }

    /// The pages placed down the view.
    pub fn spread(&self) -> Spread {
        Spread::of(&self.sheets, self.width, self.reflow)
    }

    /// Bring the scroll up to date with the view's size and the pages', and
    /// keep it within them.
    pub fn fit(&mut self) {
        let spread = self.spread();
        let height = self.height();
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
/// the host's bars and over whatever of the editor reaches past its box.
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
.pages .piece { position: absolute; display: block; }
.pages .caret { position: absolute; width: 1.5px; background: #1d4ed8; }
.pages .selected { position: absolute; background: rgba(37, 99, 235, 0.25); }
.pages .note { position: absolute; left: 16px; right: 16px; top: 8px; z-index: 1;
               font: 12px system-ui, -apple-system, sans-serif; white-space: pre-wrap; }";

#[derive(Props, Clone)]
pub struct PagesPaneProps {
    pages: Rc<Pages>,
}

impl PartialEq for PagesPaneProps {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.pages, &other.pages)
    }
}

/// Which node draws each piece on show. A piece's node is kept while its
/// picture is; a picture set anew (a keystroke's, of the block it is in)
/// takes the node of the one it replaces, where that one was. Blitz draws a
/// node's old picture until its new `src` is loaded, where a new node would
/// draw nothing until then: the line typed in would vanish at each
/// keystroke, for as long as typing outran the pictures' loading.
#[derive(Default)]
struct Slots {
    /// The pieces last on show: each one's node, and its top and bottom.
    shown: std::collections::HashMap<String, (u64, f64, f64)>,
    next: u64,
}

impl Slots {
    /// The node for each piece of `shown` (its id, top and bottom), of the
    /// pieces `all`: a piece no longer among them was replaced, and its
    /// node is free for the new piece over most of its place.
    fn assign(
        &mut self,
        shown: &[(String, f64, f64)],
        all: &std::collections::HashSet<&str>,
    ) -> Vec<u64> {
        let mut free: Vec<(u64, f64, f64)> = self
            .shown
            .iter()
            .filter(|(id, _)| !all.contains(id.as_str()))
            .map(|(_, s)| *s)
            .collect();
        free.sort_by_key(|f| f.0);
        let mut out = Vec::with_capacity(shown.len());
        let mut now = std::collections::HashMap::with_capacity(shown.len());
        for (id, top, bottom) in shown {
            let slot = match self.shown.get(id) {
                Some(&(slot, ..)) => slot,
                None => {
                    let overlap = |&(_, t, b): &(u64, f64, f64)| b.min(*bottom) - t.max(*top);
                    let best = free
                        .iter()
                        .enumerate()
                        .filter(|(_, f)| overlap(f) > 0.0)
                        .max_by(|(_, a), (_, b)| overlap(a).total_cmp(&overlap(b)))
                        .map(|(i, _)| i);
                    match best {
                        Some(i) => free.remove(i).0,
                        None => {
                            self.next += 1;
                            self.next
                        }
                    }
                }
            };
            now.insert(id.clone(), (slot, *top, *bottom));
            out.push(slot);
        }
        self.shown = now;
        out
    }
}

/// The pages, while they are shown, with the caret and the selection over
/// them: what a host puts in its page, over its editor, for `pages` to draw
/// in.
#[allow(non_snake_case)]
pub fn PagesPane(props: PagesPaneProps) -> Element {
    let pages = props.pages;
    let mut view = use_signal(PagesView::default);
    use_hook(|| pages.attach(view));
    let slots = use_hook(|| Rc::new(RefCell::new(Slots::default())));
    let v = view.read();
    if v.width <= 0.0 {
        return rsx! {};
    }
    let top = v.top();
    // Its height, not a distance from the window's bottom: a host's area
    // may stop short of it (a footer, an on-screen keyboard).
    let (left, width, height) = (v.left, v.width, v.height());
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
    let screen = v.scroll.view.1.max(1.0);
    let (from, to) = (oy - screen, oy + 2.0 * screen);
    let mut seen: std::collections::HashMap<u128, usize> = Default::default();
    let mut ids = Vec::with_capacity(v.pieces.len());
    let shown: Vec<(usize, f64, f64, f64, f64, Rc<str>)> = v
        .pieces
        .iter()
        .filter_map(|p| {
            // The same picture twice (two rules) is two nodes.
            let n = seen.entry(p.key).or_default();
            *n += 1;
            ids.push(format!("{:x}-{n}", p.key));
            let (left, top) = (*spread.lefts.get(p.page)?, *spread.tops.get(p.page)?);
            let (x, y) = (left * z, (top + p.top * s) * z);
            let (w, h) = (p.width * s * z, p.height * s * z);
            (y + h >= from && y <= to).then(|| (ids.len() - 1, x, y, w, h, p.url.clone()))
        })
        .collect();
    let all: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
    let placed: Vec<(String, f64, f64)> = shown
        .iter()
        .map(|&(i, _, y, _, h, _)| (ids[i].clone(), y, y + h))
        .collect();
    let nodes = slots.borrow_mut().assign(&placed, &all);
    let shown: Vec<(u64, f64, f64, f64, f64, Rc<str>)> = shown
        .into_iter()
        .zip(nodes)
        .map(|((_, x, y, w, h, url), slot)| (slot, x, y, w, h, url))
        .collect();
    // A press, or a drag, at a point in the window: the line and the byte it
    // lands on, as the editor's target.
    let land = move |x: f64, y: f64| -> Option<(Target, usize)> {
        let v = view.peek();
        let spread = v.spread();
        let (cx, cy) = v.scroll.to_content(x - v.left, y - top);
        let (page, px, py) = spread.nearest(&v.sheets, cx, cy)?;
        let (line, at) = v.geometry.hit(page, px, py)?;
        Some((Target::Offset(at), line))
    };
    let (press, drag) = (pages.clone(), pages.clone());
    rsx! {
        div { class: if v.reflow { "pages reflow" } else { "pages" }, role: "region",
            aria_label: if v.reflow { "Document" } else if v.editing { "Pages" } else { "PDF preview" },
            style: "left: {left}px; top: {top}px; width: {width}px; height: {height}px",
            // The keys stay with the editor; a press puts its caret here.
            onmousedown: move |e: MouseEvent| {
                e.prevent_default();
                let p = e.client_coordinates();
                if let Some((at, line)) = land(p.x, p.y) {
                    let (extend, mode) = press_mode(&e);
                    press.point(Pointer::Press { at, line, x: 0.0, extend, mode });
                }
            },
            onmousemove: move |e: MouseEvent| {
                if e.held_buttons().contains(MouseButton::Primary) {
                    let p = e.client_coordinates();
                    if let Some((at, line)) = land(p.x, p.y) {
                        drag.point(Pointer::Drag { at, line, x: 0.0 });
                    }
                }
            },
            // A finger's tap, which sends no press (the editor's `touch.rs`).
            onclick: move |e: MouseEvent| {
                let p = e.client_coordinates();
                if let Some((at, line)) = land(p.x, p.y) {
                    pages.point(Pointer::Tap { at, line, x: 0.0 });
                }
            },
            // The wheel scrolls, as the editor's does; with ⌃ (or ⌘), as a
            // pinch does, it zooms about the pointer.
            onwheel: move |e: WheelEvent| {
                e.prevent_default();
                let mut v = view.write();
                v.fit();
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
                    div { key: "p{i}", class: "sheet", role: "img", aria_label: "Page {i + 1}",
                        style: "left: {spread.lefts[i] * z}px; top: {spread.tops[i] * z}px; width: {sheet.width * s * z}px; height: {sheet.height * s * z}px",
                    }
                }
                // Only the pieces within a screen of the view: a long
                // document is a few screens' pictures, not all of them.
                // A piece's height is its picture's: a node given a new
                // picture of another height (a line added or taken away)
                // keeps drawing the old one until the new one is loaded,
                // and given the new height, the old picture, which fills
                // its box, would be stretched to it for as long.
                for (k, x, y, w, _, url) in shown {
                    img { key: "i{k}", class: "piece", src: "{url}", alt: "",
                        style: "left: {x}px; top: {y}px; width: {w}px",
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A piece drawn anew where one was replaced takes its node; a piece
    /// kept keeps its own, and one scrolled out frees none.
    #[test]
    fn a_new_picture_takes_the_node_of_the_one_it_replaces() {
        let mut slots = Slots::default();
        let p = |id: &str, top: f64| (id.to_owned(), top, top + 10.0);
        let all = |ids: &[&'static str]| ids.iter().copied().collect();
        let first = slots.assign(
            &[p("a", 0.0), p("b", 20.0), p("c", 40.0)],
            &all(&["a", "b", "c"]),
        );
        // A keystroke in b, which wraps it a little lower.
        let second = slots.assign(
            &[p("a", 0.0), p("b2", 22.0), p("c", 40.0)],
            &all(&["a", "b2", "c"]),
        );
        assert_eq!(second, first);
        // c scrolled out of the view, still a piece: a new one under it is
        // a node of its own.
        let third = slots.assign(
            &[p("a", 0.0), p("b2", 22.0), p("d", 40.0)],
            &all(&["a", "b2", "c", "d"]),
        );
        assert_eq!(&third[..2], &first[..2]);
        assert!(!first.contains(&third[2]));
        // Nothing where a replaced piece was: a node of its own.
        let fourth = slots.assign(&[p("a2", 100.0)], &all(&["a2"]));
        assert!(!third.contains(&fourth[0]));
    }

    /// Zoomed in, the pages scroll across as well as down; a caret on a
    /// later page is brought into view at the zoom; and a zoom about a point
    /// keeps that point of the pages where it was.
    #[test]
    fn the_pages_zoom_and_keep_the_caret_in_view() {
        let sheet = Sheet {
            width: 600.0,
            height: 800.0,
        };
        let mut v = PagesView {
            width: 640.0,
            sheets: vec![sheet.clone(), sheet],
            ..PagesView::default()
        };
        v.floor = 500.0;
        v.fit();
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
}
