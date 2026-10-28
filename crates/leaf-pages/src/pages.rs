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
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use base64::Engine;
use dioxus::prelude::*;
use leaf_core::{Doc, Format};
use leaf_dioxus::{Editor, Pointer, ZoomCommand};
use twig_export::pdf::Glyph;

use crate::Host;
use crate::geometry::{Geometry, PageLines, Piece, Sheet};
use crate::pdf::{Paper, Type};
use crate::view::{PagesView, ZOOM_STEP};

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
        let Some((source, setting, blocks)) = self
            .with_doc(|d| {
                let setting = Setting {
                    revision: d.revision(),
                    hole: hole(&d.source, d.caret),
                    format: d.format,
                    path: d.path.clone(),
                    ty,
                    sheet,
                };
                let chunked = chunked && matches!(d.format, Format::Markdown | Format::Djot);
                (self.from.borrow().as_ref() != Some(&setting)).then(|| {
                    let blocks = chunked.then(|| d.top_level());
                    (d.source.clone(), setting, blocks)
                })
            })
            .flatten()
        else {
            return;
        };
        *self.from.borrow_mut() = Some(setting.clone());
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
            let answer = set(source, &setting, blocks.as_deref(), &known);
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
/// (its top-level blocks) are given and it is reflowed, else whole.
fn set(
    source: String,
    s: &Setting,
    blocks: Option<&[leaf_core::TopBlock]>,
    known: &HashSet<u128>,
) -> Answer {
    // The caret's empty line, set with a stand-in where the caret is.
    let set_from = match s.hole {
        Some(at) => format!("{}{STAND_IN}{}", &source[..at], &source[at..]),
        None => source.clone(),
    };
    if let (Some(blocks), crate::pdf::Sheet::Reflow { width }) = (blocks, s.sheet) {
        let how = crate::reflow::How {
            format: s.format,
            ty: s.ty.clone(),
            width,
            root: crate::typeset::root(Some(&s.path)).to_path_buf(),
        };
        let t = std::time::Instant::now();
        let laid = crate::reflow::lay_out(&source, blocks, s.hole, &how, known);
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
    let layout = crate::pdf::layout(&set_from, s.format, Some(&s.path), &s.ty, s.sheet, known);
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
            .map(|p| {
                let glyphs = match s.hole {
                    Some(at) => unhole(p.glyphs, at),
                    None => p.glyphs,
                };
                (p.height, glyphs)
            })
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
            geometry: Geometry::new(&lines),
            pieces,
        }
    });
    (source, set)
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

/// Glyphs set from a source with a stand-in at `at`, as glyphs of the
/// source without it: the stand-in's at `at`, empty, which a caret there
/// is drawn before.
fn unhole(glyphs: Vec<Glyph>, at: usize) -> Vec<Glyph> {
    let len = STAND_IN.len();
    glyphs
        .into_iter()
        .map(|mut g| {
            g.source = g.source.map(|r| {
                let map = |b: usize| {
                    if b <= at {
                        b
                    } else {
                        b.saturating_sub(len).max(at)
                    }
                };
                map(r.start)..map(r.end)
            });
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
        let out = unhole(vec![g(0..1), g(3..5), g(5..6)], 3);
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
            &Default::default(),
        )
        .expect("a layout");
        let glyphs = unhole(layout.pages.clone()[0].glyphs.clone(), 7);
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
            &known,
        )
        .expect("a layout");
        assert!(again.tiles.iter().all(|t| t.svg.is_none()));

        let pages = layout.pages.clone();
        let g = Geometry::new(&[(pages[0].height, pages[0].glyphs.clone())]);
        let one = g.caret_box(5, source).expect("after the heading");
        let a = g.caret_box(7, source).expect("before the paragraph");
        assert!(a.top > one.top, "{one:?} {a:?}");
        assert_eq!(g.line_edge(9, false), Some(7));
        assert_eq!(g.line_edge(9, true), Some(14));
    }
}
