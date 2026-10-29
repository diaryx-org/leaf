//! A window's pages: the document as Typst sets it, set again as it
//! changes. In the editor's place, where they are edited, reflowed to the
//! view's width (the default) or on sheets (the page view); or beside the
//! source view, where the source is edited and the pages show what it sets.
//!
//! The pages are the PDF's (`pdf.rs`): twig-export sets the document with
//! Typst on a worker thread, and each block comes back as a picture, drawn
//! on the paper's white whatever the appearance, and each page as its
//! glyphs, each with the bytes of the source it was set from. One setting is
//! under way at a time, and what is typed while it is set is set together
//! after it, so the pages keep up with typing as closely as Typst does and
//! never fall behind. A piece whose picture is the one on show already is
//! left as it is: an edit draws again only the pieces it moved.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use base64::Engine;
use dioxus::prelude::*;
use leaf_core::{CellSpan, Doc, Format};
use leaf_dioxus::{Directive, Editor, Pointer, ZoomCommand};
use twig_export::pdf::Glyph;

use crate::Host;
use crate::geometry::{Geometry, PageLines, Piece, Sheet};
use crate::pdf::{Paper, Type};
use crate::typeset::{Found, Supply};
use crate::view::{Card, PagesView, ZOOM_STEP};

/// The share of the area's width the preview takes.
const SHARE: f64 = 0.5;

/// How a window shows its document. There is no view of the editor's own
/// rendering: what reads as the document is what Typst sets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// The reflowed view: in the editor's place, edited there, as one page
    /// as wide as the window and as tall as the text.
    #[default]
    Reflow,
    /// The page view: in the editor's place, edited there, on sheets.
    Pages,
    /// The editor's source view on the left, and the pages it sets on the
    /// right.
    Source,
}

impl Mode {
    /// Whether it is in the editor's place, and edited there.
    pub fn edits(self) -> bool {
        matches!(self, Mode::Pages | Mode::Reflow)
    }
}

/// A window's pages: what they are set from, what is on show, and the
/// view's state. Made by the host, kept in an `Rc`, and drawn by a
/// [`crate::PagesPane`] given it.
pub struct Pages {
    host: Rc<dyn Host>,
    /// What the pane draws, once it is made.
    view: Cell<Option<Signal<PagesView>>>,
    mode: Cell<Mode>,
    /// The view the document was edited in before the source: what leaving
    /// the source goes back to.
    last: Cell<Mode>,
    /// Whether the editor moves by the pages' lines.
    lines_on: Cell<bool>,
    /// Whether the caret the document opened with has been put on a stop.
    snapped: Cell<bool>,
    /// A setting is under way on the worker.
    busy: Cell<bool>,
    /// What the pages were last set from, or are being set from.
    from: RefCell<Option<Setting>>,
    /// The page view's lines, which the editor moves by while it is shown.
    lines: Rc<PageLines>,
    /// The caret and the selection the view last drew, and the revision of
    /// the pages it drew them on.
    drawn: Cell<Option<(usize, Option<usize>, u64)>>,
    /// Which setting of the pages is on show, counting up.
    set: Cell<u64>,
    /// The pictures of the pieces on show, by key.
    pictures: RefCell<HashMap<u128, Rc<str>>>,
    /// The source the pages on show were set from: until the pages of an
    /// edit are set, the caret is drawn through the edit onto these.
    laid: RefCell<Rc<str>>,
    /// The pages' throw: odd while one is under way. Starting another, or
    /// stopping it, moves it on, and the loop that moves the pages stops.
    glide: Cell<u64>,
    /// The page count the host was last told of.
    counted: Cell<Option<Option<usize>>>,
    /// The directives the host draws a view of, in the setting under way:
    /// each one's bytes of the source, and what the host is given to draw.
    asked: RefCell<Vec<(Range<usize>, Directive)>>,
}

/// Everything the pages depend on but the source: what decides whether
/// they need setting again.
#[derive(Clone, Debug, PartialEq)]
struct Setting {
    revision: u64,
    /// Where a stand-in is set for the caret's empty line ([`hole`]).
    hole: Option<usize>,
    format: Format,
    path: PathBuf,
    ty: Type,
    sheet: crate::pdf::Sheet,
    /// What the host found: the pictures, and the room for its views.
    supply: Arc<Supply>,
}

/// The pages as the worker sets them, for the view to take on its turn:
/// each page's size, the lines of their glyphs, and their pieces, each with
/// its picture as a `data:` URL if the window had none of its key.
struct Set {
    sheets: Vec<Sheet>,
    geometry: Geometry,
    pieces: Vec<(crate::typeset::Tiled, Option<String>)>,
}

/// What a setting answers: the source it was set from, and the pages.
type Answer = (String, Result<Set, String>);

impl Pages {
    /// The pages of the document `host`'s editor edits, in the reflowed
    /// view, set once they are placed and updated.
    pub fn new(host: Rc<dyn Host>) -> Pages {
        Pages {
            host,
            view: Cell::new(None),
            mode: Cell::default(),
            last: Cell::default(),
            lines_on: Cell::new(false),
            snapped: Cell::new(false),
            busy: Cell::new(false),
            from: RefCell::new(None),
            lines: Rc::default(),
            drawn: Cell::new(None),
            set: Cell::new(0),
            pictures: RefCell::default(),
            laid: RefCell::default(),
            glide: Cell::new(0),
            counted: Cell::new(None),
            asked: RefCell::default(),
        }
    }

    /// The view the document is shown in.
    pub fn mode(&self) -> Mode {
        self.mode.get()
    }

    /// How many pages are on show, in the page view: for a host to show
    /// beside the word count. `None` in any other view, or before the pages
    /// are first set.
    pub fn page_count(&self) -> Option<usize> {
        if self.mode() != Mode::Pages {
            return None;
        }
        let n = self.peek(|v| v.sheets.len());
        (n > 0).then_some(n)
    }

    /// The pane's view, as it is made.
    pub(crate) fn attach(&self, view: Signal<PagesView>) {
        self.view.set(Some(view));
    }

    /// Run `f` in the host's runtime: `None` if it did not run it.
    fn scoped<R>(&self, f: impl FnOnce() -> R) -> Option<R> {
        let mut f = Some(f);
        let mut out = None;
        self.host.in_scope(&mut || {
            if let Some(f) = f.take() {
                out = Some(f());
            }
        });
        out
    }

    /// Change what the pane draws, inside the host's runtime; not before
    /// the pane is made.
    pub fn with_view(&self, f: impl FnOnce(Signal<PagesView>)) {
        if let Some(view) = self.view.get() {
            self.scoped(|| f(view));
        }
    }

    /// Read what the pane draws; the default before it is made.
    pub fn peek<R: Default>(&self, f: impl FnOnce(&PagesView) -> R) -> R {
        match self.view.get() {
            Some(view) => f(&view.peek()),
            None => R::default(),
        }
    }

    /// The view the host draws of `directive`, over the room left for it.
    pub(crate) fn draw(&self, directive: &Directive) -> Element {
        match self.host.editor() {
            Some(ed) => ed.host().draw_directive(directive),
            None => rsx! {},
        }
    }

    /// Run `f` on the editor, inside the host's runtime.
    fn with_editor<R>(&self, f: impl FnOnce(&Editor) -> R) -> Option<R> {
        let ed = self.host.editor()?;
        self.scoped(|| f(&ed))
    }

    /// Read or change the document; `None` without an editor, or while it
    /// has the document borrowed this instant.
    fn with_doc<R>(&self, f: impl FnOnce(&mut Doc) -> R) -> Option<R> {
        let doc = self.host.editor()?.shared_doc();
        let mut d = doc.try_borrow_mut().ok()?;
        Some(f(&mut d))
    }

    /// A press on the pages, for the editor.
    pub(crate) fn point(&self, p: Pointer) {
        self.host.point(p);
    }

    /// Tell the host the page count, if it changed.
    fn count(&self) {
        let n = self.page_count();
        if self.counted.replace(Some(n)) != Some(n) {
            self.host.counted(n);
        }
    }

    /// Place the pages in the host's area, and tell their view where it is:
    /// the editor has the rest, which is how far the pages reach in from the
    /// area's right, answered. In the page view the editor keeps the area,
    /// under the pages, so that its status line shows below them.
    pub fn place(self: &Rc<Self>) -> f64 {
        let mode = self.mode();
        let area = self.host.area();
        let (left, pane, bottom) = match mode {
            Mode::Source => {
                let pane = (area.width * SHARE).round();
                (area.left + area.width - pane, pane, 0.0)
            }
            Mode::Pages | Mode::Reflow => (area.left, area.width, leaf_dioxus::STATUS),
        };
        let floor = area.top + area.height;
        // Edited on the pages, the editor moves by their lines, and its Enter
        // writes no empty paragraph Typst would set as nothing.
        if self.lines_on.get() != mode.edits() {
            self.lines_on.set(mode.edits());
            let lines = mode
                .edits()
                .then(|| self.lines.clone() as Rc<dyn leaf_dioxus::Lines>);
            self.with_editor(|ed| {
                ed.set_lines(lines);
                // The pages are drawn over it: it need not draw the document.
                ed.set_drawn(!mode.edits());
            });
            self.with_doc(|d| d.set_typeset(mode.edits()));
        }
        let reflow = mode == Mode::Reflow;
        self.with_view(|mut v| {
            let now = v.peek().clone();
            if (
                now.left,
                now.width,
                now.bottom,
                now.area_top,
                now.floor,
                now.editing,
                now.reflow,
            ) != (left, pane, bottom, area.top, floor, mode.edits(), reflow)
            {
                let mut v = v.write();
                v.left = left;
                v.width = pane;
                v.bottom = bottom;
                v.area_top = area.top;
                v.floor = floor;
                v.editing = mode.edits();
                v.reflow = reflow;
                if reflow {
                    v.scroll.zoom = 1.0;
                }
            }
            if v.peek().scroll.view != (pane, v.peek().height()) {
                v.write().fit();
            }
        });
        self.measure();
        // Reflowed, a new width is a new page: set it again.
        if reflow {
            self.update();
        }
        if mode == Mode::Source { pane } else { 0.0 }
    }

    /// Tell the page view's lines how many points of the pages a screen
    /// shows, for a move by a screen: at the scale the pages are drawn at
    /// now.
    fn measure(&self) {
        let points = self.peek(|v| {
            let scale = v.spread().scale * v.scroll.zoom;
            v.height() / scale
        });
        self.lines.screen.set(points);
    }

    /// Show the document in `mode`: the editor in its source view for the
    /// source, in its rich one otherwise, which is what its commands act as.
    pub fn show(self: &Rc<Self>, mode: Mode) {
        let source = mode == Mode::Source;
        self.with_editor(|ed| {
            if (ed.doc().view == leaf_core::View::Source) != source {
                ed.doc().toggle_view();
                ed.refresh();
            }
        });
        self.switch(mode);
    }

    /// The editor's view changed under the pages (⌘E, the editor's own
    /// command): follow it, into the source or back to the view before.
    pub fn follow(self: &Rc<Self>, source_view: bool) {
        let mode = self.mode();
        if source_view && mode != Mode::Source {
            self.switch(Mode::Source);
        } else if !source_view && mode == Mode::Source {
            self.switch(self.last.get());
        }
    }

    fn switch(self: &Rc<Self>, mode: Mode) {
        let now = self.mode();
        if now == mode {
            return;
        }
        if now.edits() {
            self.last.set(now);
        }
        self.mode.set(mode);
        self.drawn.set(None);
        // Another view is another scale: start from the top, and the caret's
        // reveal scrolls it into view from there.
        self.with_view(|mut v| {
            let mut v = v.write();
            v.scroll.x = 0.0;
            v.scroll.y = 0.0;
        });
        self.count();
        // The host places the pages again; its toolbar's tools and its
        // menus' ticks follow.
        self.host.switched();
    }

    /// Set the pages again, if they are shown and the document or the
    /// choices have changed since they were last set; after the setting
    /// under way, if there is one. And draw the caret on them where it is
    /// now.
    pub fn update(self: &Rc<Self>) {
        // The find bar is the editor's, at the top of its box: edited on the
        // pages, they start below it while it is open.
        let inset = if self.mode().edits() {
            self.with_editor(|ed| ed.bar_height()).unwrap_or(0.0)
        } else {
            0.0
        };
        if self.peek(|v| v.inset) != inset {
            self.with_view(|mut v| {
                let mut v = v.write();
                v.inset = inset;
                v.fit();
            });
            self.measure();
        }
        self.caret();
        if self.busy.get() {
            return;
        }
        let choices = self.host.choices();
        let paged = Type {
            template: choices.template.clone(),
            size: choices.size * 0.75,
            dark: false,
        };
        let (ty, sheet) = if self.mode() == Mode::Reflow {
            // The view's width, in points; the reader's size at the window's
            // zoom, as the editor's; light ink in a dark window.
            let width = self.peek(|v| v.width);
            if width <= 0.0 {
                return;
            }
            let ty = Type {
                size: choices.size * 0.75 * choices.zoom,
                dark: self.host.dark(),
                ..paged
            };
            (
                ty,
                crate::pdf::Sheet::Reflow {
                    width: (width * 0.75) as f32,
                },
            )
        } else {
            let (width, height) = choices.paper;
            let paper = Paper {
                width,
                height,
                margin: 72.0,
            };
            (paged, crate::pdf::Sheet::Paper(paper))
        };
        // Reflowed, in a built-in template, a chunk at a time (`reflow.rs`):
        // the document's top-level blocks are what it is cut by.
        let chunked =
            matches!(sheet, crate::pdf::Sheet::Reflow { .. }) && crate::reflow::chunks(&ty);
        let host = self.host.editor().map(|ed| ed.host());
        let dark = ty.dark;
        let Some((source, setting, blocks, asked, cells)) = self
            .with_doc(|d| {
                let (supply, asked) = match &host {
                    Some(host) => gather(d, &**host, dark),
                    None => Default::default(),
                };
                let setting = Setting {
                    revision: d.revision(),
                    hole: hole(&d.source, d.caret),
                    format: d.format,
                    path: d.path.clone(),
                    ty,
                    sheet,
                    supply: Arc::new(supply),
                };
                let chunked = chunked && matches!(d.format, Format::Markdown | Format::Djot);
                (self.from.borrow().as_ref() != Some(&setting)).then(|| {
                    let blocks = chunked.then(|| d.top_level());
                    (d.source.clone(), setting, blocks, asked, d.table_cells())
                })
            })
            .flatten()
        else {
            return;
        };
        *self.from.borrow_mut() = Some(setting.clone());
        *self.asked.borrow_mut() = asked;
        self.busy.set(true);
        let known: HashSet<u128> = self.pictures.borrow().keys().copied().collect();
        let started = std::time::Instant::now();
        let inbox: Arc<Mutex<Option<Answer>>> = Arc::default();
        let me = Rc::downgrade(self);
        let answered = Arc::clone(&inbox);
        let wake = self.host.park(Box::new(move || {
            let Some(p) = me.upgrade() else { return };
            let Some((source, set)) = answered.lock().ok().and_then(|mut a| a.take()) else {
                return;
            };
            let arrived = std::time::Instant::now();
            p.busy.set(false);
            if set.is_ok() {
                *p.laid.borrow_mut() = Rc::from(source);
            }
            p.take(set);
            leaf_dioxus::trace(|| {
                format!(
                    "pages set {:.1} ms, taken {:.1} ms",
                    (arrived - started).as_secs_f64() * 1000.0,
                    arrived.elapsed().as_secs_f64() * 1000.0
                )
            });
            p.update();
            // The pages are a signal's: the window's page runs again to draw
            // them, now, not at the next event that happens to poll it.
            p.scoped(|| ());
        }));
        tokio::task::spawn_blocking(move || {
            let answer = set(source, &setting, blocks.as_deref(), &cells, &known);
            if let Ok(mut a) = inbox.lock() {
                *a = Some(answer);
            }
            wake();
        });
    }

    /// The pages as set, or why they could not be: a piece the window has
    /// the picture of already keeps it, and only the pictures of these pages
    /// are kept.
    fn take(&self, set: Result<Set, String>) {
        let set = match set {
            Ok(set) => set,
            Err(e) => {
                self.with_view(|mut v| v.write().error = Some(e));
                return;
            }
        };
        let geometry = Rc::new(set.geometry);
        *self.lines.geometry.borrow_mut() = geometry.clone();
        // Each view the host draws, over the room left for it.
        let cards: Vec<Card> = self
            .asked
            .borrow()
            .iter()
            .filter_map(|(span, directive)| {
                Some(Card {
                    at: geometry.object(span.clone())?,
                    directive: directive.clone(),
                })
            })
            .collect();
        let pieces: Vec<Piece> = {
            let mut pictures = self.pictures.borrow_mut();
            let mut kept = HashMap::with_capacity(set.pieces.len());
            let pieces = set
                .pieces
                .into_iter()
                .filter_map(|(t, url)| {
                    let url = match url {
                        Some(u) => Rc::from(u),
                        None => kept.get(&t.key).or_else(|| pictures.get(&t.key)).cloned()?,
                    };
                    kept.insert(t.key, Rc::clone(&url));
                    Some(Piece {
                        page: t.page,
                        top: t.top,
                        height: t.height,
                        width: t.width,
                        key: t.key,
                        url,
                    })
                })
                .collect();
            *pictures = kept;
            pieces
        };
        let set = set.sheets;
        self.set.set(self.set.get() + 1);
        // Set at a new width, the text moves under the reader: where the caret
        // is on the screen, if it is on it, to keep it there.
        let held = self.peek(|v| {
            let widened = v.sheets.first().map(|s| s.width) != set.first().map(|s| s.width);
            let b = v.caret.filter(|_| widened)?;
            let y = v.content_box(&b).1 * v.scroll.zoom - v.scroll.y;
            (0.0..=v.height()).contains(&y).then_some(y)
        });
        self.with_view(|mut v| {
            let mut v = v.write();
            v.sheets = set;
            v.pieces = pieces;
            v.cards = cards;
            v.geometry = geometry;
            v.error = None;
            v.fit();
        });
        self.measure();
        self.caret();
        if let Some(y) = held {
            self.with_view(|mut v| {
                let mut v = v.write();
                if let Some(b) = v.caret {
                    v.scroll.y = v.content_box(&b).1 * v.scroll.zoom - y;
                    v.fit();
                }
            });
        }
        self.count();
    }

    /// Draw the editor's caret and selection on the pages, if either, or the
    /// pages under them, has changed since they were last drawn; and scroll
    /// the caret into view if it moved.
    fn caret(&self) {
        // A document opens with its caret at its first byte, which in the rich
        // view may be hidden markup (a heading's `# `), where typing would type
        // before the marker: put it on the first letter, as a press there would.
        if !self.snapped.get() {
            let snapped = self.with_doc(|d| {
                if d.view == leaf_core::View::Wysiwyg && d.anchor.is_none() {
                    d.build_visual_unwrapped();
                    d.place_caret(d.caret, false);
                }
            });
            self.snapped.set(snapped.is_some());
        }
        let Some((caret, anchor, source)) =
            self.with_doc(|d| (d.caret, d.anchor, d.source.clone()))
        else {
            return;
        };
        let now = (caret, anchor, self.set.get());
        let before = self.drawn.replace(Some(now));
        if before == Some(now) {
            return;
        }
        let moved = before.is_none_or(|(c, ..)| c != caret);
        let laid = self.laid.borrow().clone();
        self.with_view(|mut v| {
            let mut v = v.write();
            let g = v.geometry.clone();
            v.caret = g.caret_through(caret, &source, &laid);
            v.selection = match anchor.filter(|&a| a != caret) {
                Some(a) => {
                    let (a, c) = (
                        crate::geometry::through(a, &source, &laid),
                        crate::geometry::through(caret, &source, &laid),
                    );
                    g.selection(a.min(c), a.max(c))
                }
                None => Vec::new(),
            };
            if moved && let Some(b) = v.caret {
                v.fit();
                v.reveal(b);
            }
        });
    }

    /// Zoom the pages in, out, or to their width, about the view's middle,
    /// if the page view is shown: whether it was, and so took the command.
    pub fn zoom(&self, z: ZoomCommand) -> bool {
        if self.mode() != Mode::Pages {
            return false;
        }
        self.with_view(|mut v| {
            let mut v = v.write();
            v.fit();
            let (px, py) = (v.scroll.view.0 / 2.0, v.scroll.view.1 / 2.0);
            let to = match z {
                ZoomCommand::In => v.scroll.zoom * ZOOM_STEP,
                ZoomCommand::Out => v.scroll.zoom / ZOOM_STEP,
                ZoomCommand::Reset => 1.0,
            };
            v.scroll.zoom_to(to, px, py);
        });
        self.measure();
        true
    }

    /// A trackpad's pinch, by `delta` (positive spreads the fingers), at the
    /// pointer `at` in the window: the pages zoom about it, if they are shown
    /// and it is over them. Whether they took it.
    pub fn pinch(&self, delta: f64, at: Option<(f64, f64)>) -> bool {
        // Reflowed, the text grows by its size, not by a zoom of the page.
        if self.mode() == Mode::Reflow || !delta.is_finite() {
            return false;
        }
        let top = self.peek(|v| v.top());
        let mut took = false;
        self.with_view(|mut v| {
            let (x, y) = at.unwrap_or((0.0, 0.0));
            let (left, width) = (v.peek().left, v.peek().width);
            if at.is_none() || x < left || x > left + width || y < top {
                return;
            }
            let mut v = v.write();
            v.fit();
            v.scroll.zoom_at(1.0 + delta, x - left, y - top);
            took = true;
        });
        if took {
            self.measure();
        }
        took
    }

    /// Carry the pages on at `vel` CSS pixels a millisecond down, slowing as
    /// the editor's throw slows (`unit::scroll::Glide`): frame by frame, each
    /// moved as far as the time since the last says.
    pub fn fling(self: &Rc<Self>, vel: f64) {
        let id = self.glide.get() / 2 * 2 + 3;
        self.glide.set(id);
        let Some(mut g) = self.peek(|v| {
            let (_, (min, max)) = v.scroll.range();
            Some(unit::scroll::Glide {
                pos: v.scroll.y,
                vel,
                min,
                max,
            })
        }) else {
            return;
        };
        let me = Rc::downgrade(self);
        self.scoped(|| {
            spawn(async move {
                let mut last = std::time::Instant::now();
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(8)).await;
                    let Some(p) = me.upgrade() else {
                        return;
                    };
                    if p.glide.get() != id {
                        return;
                    }
                    let now = std::time::Instant::now();
                    let dt = now.duration_since(last).as_secs_f64() * 1000.0;
                    last = now;
                    g.max = p.peek(|v| v.scroll.range().1.1);
                    let moving = g.step(dt);
                    p.with_view(|mut v| v.write().scroll.y = g.pos);
                    p.host.redraw();
                    if !moving {
                        p.glide.set(id + 1);
                        return;
                    }
                }
            });
        });
    }

    /// Stop the pages' throw where they are.
    pub fn stop_fling(&self) {
        let g = self.glide.get();
        if g % 2 == 1 {
            self.glide.set(g + 1);
        }
    }

    /// Whether the pages are shown under window point (`x`, `y`).
    pub fn under(&self, (x, y): (f64, f64)) -> bool {
        y >= self.peek(|v| v.top())
            && self.peek(|v| v.width > 0.0 && x >= v.left && x <= v.left + v.width)
    }

    /// The caret on the pages, in the window: its left, top and height, in
    /// CSS pixels. For the input method, which the editor under the pages
    /// tells where the caret is (`leaf_dioxus::Host::caret_area`).
    pub fn caret_area(&self) -> Option<(f64, f64, f64)> {
        self.peek(|v| {
            let b = v.caret?;
            let (x, y, _, h) = v.content_box(&b);
            let z = v.scroll.zoom;
            Some((
                v.left + x * z - v.scroll.x,
                v.top() + y * z - v.scroll.y,
                h * z,
            ))
        })
    }
}

/// Set `source` as `s` says, on the worker: a chunk at a time if `blocks`
/// (its top-level blocks) are given and it is reflowed, else whole. `cells`
/// are its table cells' text (`Doc::table_cells`), which its lines are
/// split at.
fn set(
    source: String,
    s: &Setting,
    blocks: Option<&[leaf_core::TopBlock]>,
    cells: &[CellSpan],
    known: &HashSet<u128>,
) -> Answer {
    // The caret's empty line, and every empty cell, set with a stand-in.
    let holes = holes(s.hole, cells);
    let set_from = stand_in(&source, &holes);
    if let (Some(blocks), crate::pdf::Sheet::Reflow { width }) = (blocks, s.sheet) {
        let how = crate::reflow::How {
            format: s.format,
            ty: s.ty.clone(),
            width,
            root: crate::typeset::root(Some(&s.path)).to_path_buf(),
            supply: Arc::clone(&s.supply),
        };
        let t = std::time::Instant::now();
        let laid = crate::reflow::lay_out(&source, blocks, s.hole, &holes, cells, &how, known);
        let t_lay = t.elapsed();
        let set = laid.map(|r| Set {
            sheets: vec![r.sheet],
            geometry: r.geometry,
            pieces: r
                .pieces
                .into_iter()
                .map(|mut t| {
                    let url = t.svg.take().map(|svg| data_url(&svg));
                    (t, url)
                })
                .collect(),
        });
        leaf_dioxus::trace(|| format!("reflow: laid {t_lay:?}, pictures by {:?}", t.elapsed()));
        return (source, set);
    }
    // Set with the stand-in, a room is asked for by where its directive is
    // in that source.
    let supply = match s.hole {
        Some(h) => Arc::new(Supply {
            pictures: s.supply.pictures.clone(),
            rooms: s
                .supply
                .rooms
                .iter()
                .map(|(&at, &room)| (if at >= h { at + STAND_IN.len() } else { at }, room))
                .collect(),
        }),
        None => Arc::clone(&s.supply),
    };
    let layout = crate::pdf::layout(
        &set_from,
        s.format,
        Some(&s.path),
        &s.ty,
        s.sheet,
        supply,
        known,
    );
    let set: Result<Set, String> = layout.map(|l| {
        let sheets = l
            .pages
            .iter()
            .map(|p| Sheet {
                width: p.width,
                height: p.height,
            })
            .collect();
        let lines: Vec<(f64, Vec<Glyph>)> = l
            .pages
            .into_iter()
            .map(|p| (p.height, unhole(p.glyphs, &holes)))
            .collect();
        let pieces = l
            .tiles
            .into_iter()
            .map(|mut t| {
                let url = t.svg.take().map(|svg| data_url(&svg));
                (t, url)
            })
            .collect();
        Set {
            sheets,
            geometry: Geometry::new(&lines, cells),
            pieces,
        }
    });
    (source, set)
}

/// What the host finds for the document `d` that its source does not say:
/// where its pictures are, as the editor draws them, in ink for a `dark`
/// ground or a light one; and, for each directive it draws a view of, the
/// room to leave, and the directive by its bytes of the source.
pub fn gather(
    d: &mut Doc,
    host: &dyn leaf_dioxus::Host,
    dark: bool,
) -> (Supply, Vec<(Range<usize>, Directive)>) {
    let mut supply = Supply::default();
    let mut asked = Vec::new();
    if d.view != leaf_core::View::Wysiwyg {
        return (supply, asked);
    }
    d.build_visual_unwrapped();
    let scheme = if dark {
        leaf_core::ColorScheme::Dark
    } else {
        leaf_core::ColorScheme::Light
    };
    let doc: &Doc = d;
    for m in &doc.vmap.media {
        let Some(dest) = m.still(scheme) else {
            continue;
        };
        let found = match host.picture(doc, dest) {
            Some(p) => found_at(&p.url),
            None => match leaf_dioxus::locate_media(host, doc, dest) {
                Some(path) => Found::File(path),
                None => Found::Missing,
            },
        };
        supply.pictures.insert(m.destination.clone(), found);
    }
    for info in &doc.vmap.directives {
        let directive = Directive {
            name: info.name.clone(),
            attrs: info.attrs.clone(),
            label: info.label.clone(),
        };
        let Some(height) = host.directive_height(doc, &directive) else {
            continue;
        };
        let Some(row) = doc.vmap.rows.get(info.rows_span.start) else {
            continue;
        };
        let Some(start) = row.glyphs.last().map(|g| g.src) else {
            continue;
        };
        // A CSS pixel is three quarters of a point.
        supply.rooms.insert(start, height * 0.75);
        asked.push((start..row.end_src, directive));
    }
    (supply, asked)
}

/// Where a picture a host loaded itself is, by the URL it draws it from: in
/// hand, from a `data:` URL; a file, from a `file:` one. Typst reads no web
/// address. A `data:` URL is decoded once while the host gives it: the same
/// bytes in the same hand keep every chunk of the reflowed view that holds
/// them as it was set ([`Supply::pictures_key`]).
fn found_at(url: &str) -> Found {
    thread_local! {
        static DECODED: RefCell<Vec<(String, Found)>> = const { RefCell::new(Vec::new()) };
    }
    if !url.starts_with("data:") {
        return match leaf_dioxus::media_path(url, None) {
            Some(path) if url.starts_with("file:") || url.starts_with('/') => Found::File(path),
            _ => Found::Missing,
        };
    }
    if let Some(found) =
        DECODED.with_borrow(|d| d.iter().find(|(u, _)| u == url).map(|(_, f)| f.clone()))
    {
        return found;
    }
    let found = decode_data_url(url)
        .filter(|b| crate::typeset::settable(b))
        .map_or(Found::Missing, |b| Found::Bytes(b.into()));
    DECODED.with_borrow_mut(|d| {
        // A few pictures a document, not every one a session has seen.
        if d.len() >= 32 {
            d.remove(0);
        }
        d.push((url.to_owned(), found.clone()));
    });
    found
}

/// A `data:` URL's bytes: base64, or percent-encoded.
fn decode_data_url(url: &str) -> Option<Vec<u8>> {
    let (meta, body) = url.strip_prefix("data:")?.split_once(',')?;
    if meta.ends_with(";base64") {
        let clean: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        return base64::engine::general_purpose::STANDARD.decode(clean).ok();
    }
    let b = body.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = body.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(out)
}

/// What stands in for the text of a line that has none, where the caret
/// is: a no-break space, which Typst sets as a glyph of a space's width and
/// draws as nothing.
pub const STAND_IN: &str = "\u{a0}";

/// Where to set a stand-in for the caret, if it is on a line with no text
/// for Typst to set: a new paragraph, an empty heading, list item or quote.
/// Without one, Typst sets the line as nothing, and there is no glyph to
/// draw the caret by until a letter is typed.
fn hole(source: &str, caret: usize) -> Option<usize> {
    if !source.is_char_boundary(caret) {
        return None;
    }
    let start = source[..caret].rfind('\n').map_or(0, |i| i + 1);
    let end = source[caret..]
        .find('\n')
        .map_or(source.len(), |i| caret + i);
    if !source[caret..end].trim().is_empty() {
        return None;
    }
    // What is before the caret on the line is the block's markup alone.
    let mut rest = source[start..caret].trim_start();
    loop {
        let before = rest;
        for marker in ["#", ">", "- ", "* ", "+ ", "[ ] ", "[x] ", "[X] "] {
            rest = rest.strip_prefix(marker).unwrap_or(rest);
        }
        // A footnote's definition, `[^1]: `.
        if rest.starts_with("[^")
            && let Some(end) = rest.find("]:")
        {
            rest = &rest[end + 2..];
        }
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits > 0 && (rest[digits..].starts_with(". ") || rest[digits..].starts_with(") ")) {
            rest = &rest[digits + 2..];
        }
        rest = rest.trim_start();
        if rest == before {
            break;
        }
    }
    // `-`, `*` or a number with nothing after it yet is a list's too.
    let rest = rest.trim_end_matches(['-', '*', '+', '.', ')']);
    rest.trim_start_matches(|c: char| c.is_ascii_digit())
        .is_empty()
        .then_some(caret)
}

/// Where stand-ins are set: at the caret's empty line, if it is on one
/// ([`hole`]), and in every empty table cell, which Typst would otherwise
/// set as nothing, with no glyph to press or draw a caret by. In order.
fn holes(caret: Option<usize>, cells: &[CellSpan]) -> Vec<usize> {
    let mut out: Vec<usize> = cells
        .iter()
        .filter(|c| c.text.is_empty())
        .map(|c| c.text.start)
        .chain(caret)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// `source` with a stand-in at each of `holes`, which are in order.
pub(crate) fn stand_in(source: &str, holes: &[usize]) -> String {
    let mut out = String::with_capacity(source.len() + holes.len() * STAND_IN.len());
    let mut from = 0;
    for &at in holes {
        let at = at.clamp(from, source.len());
        out.push_str(&source[from..at]);
        out.push_str(STAND_IN);
        from = at;
    }
    out.push_str(&source[from..]);
    out
}

/// A byte of a source set with stand-ins at `holes` ([`stand_in`]) as one of
/// the source without them: a stand-in's own bytes are where it stands.
pub(crate) fn unstand(at: usize, holes: &[usize]) -> usize {
    let len = STAND_IN.len();
    for (k, &h) in holes.iter().enumerate() {
        // Where the `k`th stand-in is, with those before it.
        let set = h + k * len;
        if at <= set {
            return at - k * len;
        }
        if at < set + len {
            return h;
        }
    }
    at - holes.len() * len
}

/// Glyphs set from a source with stand-ins at `holes`, as glyphs of the
/// source without them: each stand-in's at its hole, empty, which a caret
/// there is drawn before.
fn unhole(glyphs: Vec<Glyph>, holes: &[usize]) -> Vec<Glyph> {
    if holes.is_empty() {
        return glyphs;
    }
    glyphs
        .into_iter()
        .map(|mut g| {
            g.source = g
                .source
                .map(|r| unstand(r.start, holes)..unstand(r.end, holes));
            g
        })
        .collect()
}

/// An SVG as a URL Blitz draws a picture from, filling the box it is
/// drawn in. Blitz rounds an `img`'s box to whole pixels, by where it falls
/// on the screen; a picture that kept its own shape (SVG's default, `meet`)
/// would shrink to the rounded box and centre in it, a line's piece moved
/// across by as much as ten pixels, a different piece at each step of a
/// scroll.
pub fn data_url(svg: &str) -> String {
    let svg = match svg.strip_prefix("<svg") {
        Some(rest) if !svg.contains("preserveAspectRatio") => {
            format!("<svg preserveAspectRatio=\"none\"{rest}")
        }
        _ => svg.to_owned(),
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(svg);
    format!("data:image/svg+xml;base64,{b64}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new paragraph, an empty heading or list item has a stand-in; a
    /// line with text, or the caret before its text, has none.
    #[test]
    fn a_line_with_nothing_to_set_has_a_stand_in() {
        for (src, caret) in [
            ("Hello\n\n", 7),
            ("# \n", 2),
            ("- one\n- ", 8),
            ("1. ", 3),
            ("> ", 2),
            ("- [ ] ", 6),
            ("Text[^1].\n\n[^1]: ", 17),
            ("", 0),
        ] {
            assert_eq!(hole(src, caret), Some(caret), "{src:?}");
        }
        for (src, caret) in [
            ("Hello", 5),
            ("# Title", 2),
            ("- one", 2),
            ("Hello\n\nWorld", 7),
        ] {
            assert_eq!(hole(src, caret), None, "{src:?} at {caret}");
        }
    }

    /// Stand-ins at several holes: each stood-in byte goes back to its own
    /// hole, and the bytes after each are the source's again.
    #[test]
    fn stand_ins_go_back_to_their_holes() {
        let src = "|  | b |  |";
        let holes = [2, 9];
        let set = stand_in(src, &holes);
        assert_eq!(set, format!("| {STAND_IN} | b | {STAND_IN} |"));
        let back: Vec<usize> = (0..=set.len()).map(|b| unstand(b, &holes)).collect();
        // Every byte of the source is some byte of the set source's.
        for (i, c) in src.char_indices() {
            assert!(back.contains(&i), "{i} {c:?}");
        }
        assert_eq!(unstand(2, &holes), 2);
        assert_eq!(unstand(2 + STAND_IN.len(), &holes), 2);
        assert_eq!(
            unstand(set.find('b').unwrap(), &holes),
            src.find('b').unwrap()
        );
        assert_eq!(unstand(set.len(), &holes), src.len());
    }

    /// A document with a table, set reflowed or on paper as the views set
    /// it: its geometry, and its cells' text.
    fn table_set(src: &str, reflow: bool) -> (Geometry, Vec<CellSpan>) {
        let mut d = leaf_core::Doc::from_source(src.to_owned(), Format::Markdown).unwrap();
        let cells = d.table_cells();
        let blocks = d.top_level();
        let setting = Setting {
            revision: 0,
            hole: None,
            supply: Arc::default(),
            format: Format::Markdown,
            path: PathBuf::from("table.md"),
            ty: Type {
                template: crate::Template::default(),
                size: 12.0,
                dark: false,
            },
            sheet: if reflow {
                crate::pdf::Sheet::Reflow { width: 400.0 }
            } else {
                crate::pdf::Sheet::Paper(crate::pdf::Paper {
                    width: 612.0,
                    height: 792.0,
                    margin: 72.0,
                })
            },
        };
        let blocks = reflow.then_some(&blocks[..]);
        let (_, set) = set(src.to_owned(), &setting, blocks, &cells, &HashSet::new());
        (set.expect("set").geometry, cells)
    }

    const TABLE: &str = "Above.\n\n| Name | Notes |\n|---|---|\n|   | dd |\n| e |  |\n";

    /// An empty cell is set with a stand-in: a caret in it is drawn in its
    /// column and its row, and a press there lands in it.
    #[test]
    fn an_empty_cell_has_a_caret_and_takes_a_press() {
        for reflow in [true, false] {
            let (g, cells) = table_set(TABLE, reflow);
            let empty: Vec<usize> = cells
                .iter()
                .filter(|c| c.text.is_empty())
                .map(|c| c.text.start)
                .collect();
            assert_eq!(empty.len(), 2);
            let at = |b: usize| g.caret_box(b, TABLE).expect("a caret");
            let name = at(TABLE.find("Name").unwrap());
            let notes = at(TABLE.find("Notes").unwrap());
            let dd = at(TABLE.find("dd").unwrap());
            let e = at(TABLE.find("| e").unwrap() + 2);
            // Under Name, beside dd; under Notes, beside e.
            let (one, two) = (at(empty[0]), at(empty[1]));
            assert!(
                (one.x - name.x).abs() < 1.0 && (one.top - dd.top).abs() < 1.0,
                "{reflow} {one:?}"
            );
            assert!(
                (two.x - notes.x).abs() < 1.0 && (two.top - e.top).abs() < 1.0,
                "{reflow} {two:?}"
            );
            // A press a little right of the stand-in, in its cell.
            let mid = one.top + one.height / 2.0;
            assert_eq!(
                g.hit(one.page, one.x + 6.0, mid).map(|h| h.1),
                Some(empty[0]),
                "{reflow}"
            );
            let mid = two.top + two.height / 2.0;
            assert_eq!(
                g.hit(two.page, two.x + 6.0, mid).map(|h| h.1),
                Some(empty[1]),
                "{reflow}"
            );
        }
    }

    /// Home and End are the cell's text's edges, not the row's; a selection
    /// across a row is a box a cell.
    #[test]
    fn a_cells_edges_are_its_own() {
        for reflow in [true, false] {
            let (g, _) = table_set(TABLE, reflow);
            let name = TABLE.find("Name").unwrap();
            let notes = TABLE.find("Notes").unwrap();
            assert_eq!(g.line_edge(name + 2, true), Some(name + 4), "{reflow}");
            assert_eq!(g.line_edge(notes + 2, false), Some(notes), "{reflow}");
            assert_eq!(g.line_edge(notes + 2, true), Some(notes + 5), "{reflow}");
            assert_eq!(g.selection(name, notes + 5).len(), 2, "{reflow}");
        }
    }

    /// A press in the gap between two cells' text lands in the cell it is
    /// in: the cell before, unless it is in the padding before the next
    /// cell's text.
    #[test]
    fn a_press_between_cells_lands_in_its_cell() {
        let (g, _) = table_set(TABLE, true);
        let name = TABLE.find("Name").unwrap();
        let notes = TABLE.find("Notes").unwrap();
        let a = g.caret_box(name + 4, TABLE).unwrap();
        let b = g.caret_box(notes, TABLE).unwrap();
        let mid = a.top + a.height / 2.0;
        assert_eq!(g.hit(0, a.x + 2.0, mid).map(|h| h.1), Some(name + 4));
        assert_eq!(g.hit(0, b.x - 1.0, mid).map(|h| h.1), Some(notes));
    }

    /// In a row whose cells wrap to different heights, a press under the
    /// shorter cell's text is in that cell, not the taller one beside it.
    #[test]
    fn a_press_under_a_short_cells_text_stays_in_it() {
        let long = "words ".repeat(30);
        let src = format!("| short | {long}|\n|---|---|\n| x | y |\n");
        let (g, _) = table_set(&src, true);
        let short = src.find("short").unwrap();
        let s = g.caret_box(short, &src).unwrap();
        let w = g.caret_box(src.find("words").unwrap(), &src).unwrap();
        // Two lines below "short", in its column, beside the long cell's
        // third line.
        let y = s.top + s.height * 2.5;
        assert!(y < g.caret_box(src.find("| x").unwrap() + 2, &src).unwrap().top);
        assert!(w.x > s.x + 20.0);
        let (_, at) = g.hit(0, s.x + 5.0, y).unwrap();
        assert!((short..=short + 5).contains(&at), "{at}");
    }

    /// The blank well under the document's last line is past its end; its
    /// last line is not.
    #[test]
    fn the_blank_under_the_last_line_is_past_the_end() {
        let (g, _) = table_set(TABLE, true);
        let e = g.caret_box(TABLE.find("| e").unwrap() + 2, TABLE).unwrap();
        assert!(!g.past_end(0, e.top + e.height / 2.0));
        assert!(g.past_end(0, e.top + e.height * 4.0));
    }

    /// The stand-in's glyph is the caret's place; the glyphs after it are
    /// the source's again.
    #[test]
    fn a_stand_ins_glyph_is_where_the_caret_is() {
        let g = |r: std::ops::Range<usize>| Glyph {
            x: 0.0,
            baseline: 0.0,
            width: 1.0,
            ascent: 1.0,
            descent: 0.0,
            text: String::new(),
            source: Some(r),
        };
        let out = unhole(vec![g(0..1), g(3..5), g(5..6)], &[3]);
        let ranges: Vec<_> = out.into_iter().map(|g| g.source.unwrap()).collect();
        assert_eq!(ranges, [0..1, 3..3, 3..4]);
    }

    /// A Markdown paragraph of a stand-in alone is set, as a glyph.
    #[test]
    fn a_stand_in_is_set() {
        let src = format!("Hello\n\n{STAND_IN}");
        let layout = crate::pdf::layout(
            &src,
            Format::Markdown,
            None,
            &Type {
                template: crate::Template::default(),
                size: 12.0,
                dark: false,
            },
            crate::pdf::Sheet::Reflow { width: 400.0 },
            Default::default(),
            &Default::default(),
        )
        .expect("a layout");
        let glyphs = unhole(layout.pages.clone()[0].glyphs.clone(), &[7]);
        let hello = glyphs.iter().find(|g| g.source == Some(0..1)).unwrap();
        let caret = glyphs
            .iter()
            .find(|g| g.source == Some(7..7))
            .expect("the stand-in");
        assert!(caret.baseline > hello.baseline, "on a line of its own");
    }

    /// A page is a picture of its SVG, and its glyphs lead back to the
    /// source: a caret after "One" is drawn after the heading's last glyph,
    /// before the paragraph's first.
    #[test]
    fn a_page_is_a_picture_and_its_glyphs() {
        let source = "# One\n\nA page.\n";
        let layout = crate::pdf::layout(
            source,
            Format::Markdown,
            None,
            &Type {
                template: crate::Template::default(),
                size: 12.0,
                dark: false,
            },
            crate::pdf::Sheet::Paper(crate::pdf::Paper {
                width: 612.0,
                height: 792.0,
                margin: 72.0,
            }),
            Default::default(),
            &Default::default(),
        )
        .expect("a layout");
        // A piece a block, each its picture, as wide as the page.
        assert_eq!(layout.pages.len(), 1);
        assert_eq!(layout.tiles.len(), 2, "the heading and the paragraph");
        for t in &layout.tiles {
            let svg = t.svg.as_deref().expect("a picture for a key not known");
            let url = data_url(svg);
            let b64 = url.strip_prefix("data:image/svg+xml;base64,").unwrap();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .unwrap();
            // It fills the box it is drawn in, rounded or not.
            assert!(bytes.starts_with(b"<svg preserveAspectRatio=\"none\""));
            assert_eq!(t.width, 612.0);
        }
        // A key the window has is sent with no picture.
        let known = layout.tiles.iter().map(|t| t.key).collect();
        let again = crate::pdf::layout(
            source,
            Format::Markdown,
            None,
            &Type {
                template: crate::Template::default(),
                size: 12.0,
                dark: false,
            },
            crate::pdf::Sheet::Paper(crate::pdf::Paper {
                width: 612.0,
                height: 792.0,
                margin: 72.0,
            }),
            Default::default(),
            &known,
        )
        .expect("a layout");
        assert!(again.tiles.iter().all(|t| t.svg.is_none()));

        let pages = layout.pages.clone();
        let g = Geometry::new(&[(pages[0].height, pages[0].glyphs.clone())], &[]);
        let one = g.caret_box(5, source).expect("after the heading");
        let a = g.caret_box(7, source).expect("before the paragraph");
        assert!(a.top > one.top, "{one:?} {a:?}");
        assert_eq!(g.line_edge(9, false), Some(7));
        assert_eq!(g.line_edge(9, true), Some(14));
    }

    const BOX: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"></svg>"#;

    /// What the host found: the pictures as it reads them, and room under
    /// each directive it draws, at three quarters of a point a pixel; none
    /// for one it does not.
    #[test]
    fn the_host_supplies_its_pictures_and_its_rooms() {
        struct H;
        impl leaf_dioxus::Host for H {
            fn picture(&self, _doc: &Doc, dest: &str) -> Option<leaf_dioxus::Picture> {
                (dest == "web.png").then(|| leaf_dioxus::Picture {
                    url: format!(
                        "data:image/svg+xml;base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(BOX)
                    ),
                    size: (1.0, 1.0),
                })
            }
            fn directive_height(&self, _doc: &Doc, d: &Directive) -> Option<f32> {
                (d.name == "card").then_some(40.0)
            }
        }
        let src = "![a](web.png)\n\n![b](/abs/b.png)\n\n![c](https://x.org/c.png)\n\n::card[Hi]\n\n::other\n";
        let mut d = Doc::from_source(src.to_owned(), Format::Markdown).unwrap();
        let (supply, asked) = gather(&mut d, &H, false);
        assert_eq!(
            supply.pictures["web.png"],
            Found::Bytes(BOX.as_bytes().into())
        );
        assert_eq!(
            supply.pictures["/abs/b.png"],
            Found::File("/abs/b.png".into())
        );
        assert_eq!(supply.pictures["https://x.org/c.png"], Found::Missing);
        let at = src.find("::card").unwrap();
        assert_eq!(supply.rooms.len(), 1);
        assert_eq!(supply.rooms[&at], 30.0);
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0, at..at + "::card[Hi]".len());
        assert_eq!(asked[0].1.label, "Hi");
    }

    /// A host's `data:` picture, percent-encoded or base64, is its bytes,
    /// in the same hand each time it is given; one that is no picture Typst
    /// draws is none.
    #[test]
    fn a_hosts_data_url_is_decoded_once() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"></svg>"#;
        let encoded: String = svg
            .bytes()
            .map(|b| match b {
                b'<' | b'>' | b'"' | b' ' | b'/' | b'=' | b':' | b'.' => format!("%{b:02X}"),
                _ => (b as char).to_string(),
            })
            .collect();
        let url = format!("data:image/svg+xml,{encoded}");
        let (Found::Bytes(a), Found::Bytes(b)) = (found_at(&url), found_at(&url)) else {
            panic!("a picture");
        };
        assert_eq!(&*a, svg.as_bytes());
        assert!(Arc::ptr_eq(&a, &b), "decoded once");
        assert_eq!(found_at("data:image/png;base64,AAEC"), Found::Missing);
    }
}
