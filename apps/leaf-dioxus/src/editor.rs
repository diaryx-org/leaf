//! The editor: its state, and the repaint that turns the document into rows.
//!
//! An [`Editor`] is a cheap handle — shared state behind an `Rc`, and the
//! signals the render reads, which are `Copy` — so every event source (the
//! window's keys and IME, the menu bar, the pointer, a file dialog's task)
//! holds one and calls a method on it. The methods live beside what they are
//! about: motion and the pointer in `motion.rs`, input in `dispatch.rs`, the
//! find bar in `findbar.rs`, files in `file.rs`.
//!
//! A feature usually needs three things from here: state (a field of
//! [`State`]), something the render reads (a field of [`Signals`]), and a
//! step in [`Editor::repaint`]. Things drawn in place of rows — tables, and
//! whatever comes next — go through `blocks.rs` instead, so they need none of
//! the three.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use dioxus::prelude::*;
use dioxus_native::winit::window::Window;
use crate::bridge::frame::Frame;
use crate::bridge::rows::{Marks, RowHandle, RowView};
use leaf_core::{Doc, View};

use crate::blocks::{self, Block};
use crate::find::{BarView, Find};
use crate::ime::Composer;
use crate::motion::Press;
use crate::prefs::PrefsView;
use crate::row::LayoutHandle;
use crate::settings::Settings;
use crate::source::SourceRows;
use crate::status;
use crate::text::{Fonts, Kind, Type};
use crate::trace::trace;
use crate::viewport::{LINE, Layout};

/// The status line's height, in CSS pixels — `.status` in the stylesheet.
pub const STATUS: f64 = 24.0;

/// The source view's measure: 82 code cells, a line wrapped at 80 with room
/// for the caret — which is how most markup is written.
const SOURCE_CELLS: f32 = 82.0;

/// The editor's own height in CSS pixels: the window's surface, less the
/// status line.
pub fn viewport_height(physical: u32, scale: f64) -> f64 {
    (physical as f64 / scale - STATUS).max(LINE)
}

/// The window's width in CSS pixels.
pub fn window_width(physical: u32, scale: f64) -> f32 {
    (physical as f64 / scale) as f32
}

/// The text column's width in CSS pixels: the window less the side padding,
/// up to `measure` — the Swift editor's rule, its measure counted in
/// characters of the body face and so following the size and the zoom — and
/// never narrower than ten ems.
pub fn column_width(window: f32, measure: Option<f32>, size: f32) -> f32 {
    let room = window - 2.0 * crate::row::PAD as f32;
    measure.map_or(room, |m| room.min(m)).max(size * 10.0)
}

/// The measure in pixels, for each view.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Measures {
    pub wysiwyg: Option<f32>,
    pub source: f32,
}

impl Measures {
    /// From the column-width choice and the faces as `fonts` sets them.
    pub fn of(settings: &Settings, fonts: &mut Fonts) -> Self {
        Self {
            wysiwyg: settings.column.chars().map(|c| c * fonts.char_width()),
            source: SOURCE_CELLS * fonts.cell_width(),
        }
    }

    pub fn of_view(&self, view: View) -> Option<f32> {
        match view {
            View::Wysiwyg => self.wysiwyg,
            View::Source => Some(self.source),
        }
    }
}

/// What the editor keeps between events.
pub struct State {
    pub doc: RefCell<Doc>,
    pub window: Arc<dyn Window>,
    /// The rendered view's rows, diffed against last frame's.
    frame: RefCell<Frame>,
    /// The source view's rows, reused by content.
    srows: RefCell<SourceRows>,
    /// The view the rows were last built for, to start afresh on a switch.
    pub shown: Cell<Option<View>>,
    pub fonts: RefCell<Fonts>,
    layouts: RefCell<Layouts>,
    /// Last frame's blocks, for reuse.
    pub block_cache: RefCell<Vec<Block>>,
    pub composer: RefCell<Composer>,
    pub find: RefCell<Find>,
    /// The window title last set, so it is only set when it changes.
    title: RefCell<String>,
    /// The last press, for counting a double- and triple-click.
    pub last_press: Cell<Option<Press>>,
    /// The x a run of ↑/↓ aims for, kept across the lines it passes through
    /// so a short line does not pull the caret left for good.
    pub goal: Cell<Option<f32>>,
    /// The display choices (`settings.rs`), and the text zoom over them.
    pub settings: RefCell<Settings>,
    pub zoom: Cell<f32>,
    /// The column's measure in each view, for the type `fonts` sets.
    pub measures: Cell<Measures>,
}

/// What the render reads. Each is a signal, so the page re-renders when an
/// event changes one.
#[derive(Clone, Copy)]
pub struct Signals {
    pub rows: Signal<Vec<RowHandle>>,
    pub lays: Signal<Vec<LayoutHandle>>,
    pub kinds: Signal<Vec<Kind>>,
    /// What is drawn in place of rows (tables, …).
    pub blocks: Signal<Vec<Block>>,
    /// Every row's top, for the virtualized window.
    pub layout: Signal<Layout>,
    pub status: Signal<String>,
    /// The word count, as of a revision; see `status::recount`.
    pub words: Signal<Option<(u64, usize)>>,
    /// What the find bar draws.
    pub bar: Signal<BarView>,
    /// The viewport: how far down the document it starts, and how tall it
    /// is. The editor scrolls itself rather than letting `.leaf` overflow:
    /// Blitz can scroll an element only from a task, and dioxus-native polls
    /// tasks while Blitz holds its document borrowed, so following the caret
    /// that way panicked ("RefCell already borrowed"). Owning the offset makes
    /// following the caret a signal write.
    pub top: Signal<f64>,
    pub height: Signal<f64>,
    /// The window's width, from winit: dioxus-native implements neither
    /// `onresize` nor a measurement outside a task.
    pub win_w: Signal<f32>,
    /// The column the rows were last laid out for; set by `repaint`.
    pub width: Signal<f32>,
    /// The type the rows are set in, which the stylesheet writes out.
    pub ty: Signal<Type>,
    /// What the settings panel draws.
    pub prefs: Signal<PrefsView>,
}

impl Signals {
    /// The signals, made as hooks of the calling component.
    pub fn new(window: &Arc<dyn Window>) -> Self {
        let size = window.surface_size();
        let scale = window.scale_factor();
        Self {
            rows: use_signal(Vec::new),
            lays: use_signal(Vec::new),
            kinds: use_signal(Vec::new),
            blocks: use_signal(Vec::new),
            layout: use_signal(Layout::default),
            status: use_signal(String::new),
            words: use_signal(|| None),
            bar: use_signal(BarView::default),
            top: use_signal(|| 0.0),
            height: use_signal(|| viewport_height(size.height, scale)),
            win_w: use_signal(|| window_width(size.width, scale)),
            width: use_signal(|| 0.0),
            ty: use_signal(Type::default),
            prefs: use_signal(PrefsView::default),
        }
    }
}

/// Write a signal through a copy of its handle: a signal is `Copy`, and its
/// `set` takes `&mut self`, which a method on `&self` has not got.
pub fn set<T: 'static>(mut s: Signal<T>, v: T) {
    s.set(v);
}

/// Every row's layout, reused while its handle and the column width hold.
/// Keyed by the handle's address: the frame keeps an unchanged row's `Rc`
/// even when an edit above it shifts its index, so a Return relays out the
/// rows it touched, not every row below.
///
/// Each entry holds the row's handle as well as its layout: an address is
/// only a safe key while the `Rc` behind it is alive, and a dropped row's
/// address is free to be reused by the next row allocated — which would then
/// be drawn with a stale layout.
#[derive(Default)]
struct Layouts {
    width: f32,
    ty: Type,
    by_row: HashMap<*const RowView, (RowHandle, Kind, LayoutHandle)>,
}

/// The editor, as every event source holds it.
#[derive(Clone)]
pub struct Editor {
    pub st: Rc<State>,
    pub sig: Signals,
}

impl Editor {
    pub fn new(doc: Doc, window: Arc<dyn Window>, sig: Signals) -> Self {
        let settings = Settings::load();
        let mut fonts = Fonts::new(settings.ty(1.0));
        let measures = Measures::of(&settings, &mut fonts);
        let st = State {
            doc: RefCell::new(doc),
            window,
            frame: RefCell::default(),
            srows: RefCell::default(),
            shown: Cell::new(None),
            fonts: RefCell::new(fonts),
            layouts: RefCell::default(),
            block_cache: RefCell::default(),
            composer: RefCell::default(),
            find: RefCell::default(),
            title: RefCell::default(),
            last_press: Cell::new(None),
            goal: Cell::new(None),
            settings: RefCell::new(settings),
            zoom: Cell::new(1.0),
            measures: Cell::new(measures),
        };
        Self { st: Rc::new(st), sig }
    }

    /// The first paint, and every repaint after a key, a click or a resize:
    /// the document's rows, their layouts, the blocks drawn in place of some
    /// of them, and the status. Answers the caret's row, for the viewport to
    /// follow.
    pub fn repaint(&self) -> Option<usize> {
        let st = &*self.st;
        let t0 = Instant::now();
        let mut doc = st.doc.borrow_mut();
        // While composing, the caret is drawn at the input method's cursor
        // and the marked text underlined; see `ime.rs`.
        let composer = st.composer.borrow();
        let caret = composer.caret().unwrap_or(doc.caret);
        let mut frame = st.frame.borrow_mut();
        let mut srows = st.srows.borrow_mut();
        // A switch of view shares no rows with the view before it.
        if st.shown.get() != Some(doc.view) {
            *frame = Frame::default();
            srows.clear();
            st.shown.set(Some(doc.view));
        }
        let (handles, kinds, caret_row, rebuilt) = match doc.view {
            View::Source => {
                doc.build_source();
                let (h, (row, _)) = srows.update(&doc, caret, doc.selection(), composer.marked());
                let h = h.to_vec();
                let kinds = vec![Kind::Source; h.len()];
                (h, kinds, Some(row), srows.rebuilt)
            }
            View::Wysiwyg => {
                doc.build_visual_unwrapped();
                let marks = Marks::of(&doc.vmap, caret, doc.anchor).composing(composer.marked());
                let h = frame.update(&doc.vmap, &marks).to_vec();
                // A table's picture is box-drawn in character cells, so its
                // rows stay monospace, as code does.
                let mut kinds: Vec<Kind> =
                    h.iter().map(|r| if r.0.code { Kind::Cells } else { Kind::Prose }).collect();
                for t in &doc.vmap.tables {
                    for r in t.rows_span.clone() {
                        if let Some(k) = kinds.get_mut(r) {
                            *k = Kind::Cells;
                        }
                    }
                }
                (h, kinds, marks.caret.map(|(r, _)| r), frame.last.rebuilt)
            }
        };
        // An empty document maps to no rows at all, which would leave the
        // caret nowhere to be drawn and nothing to click: give it the one
        // empty line every text view shows.
        let (handles, kinds, caret_row) = if handles.is_empty() {
            let blank = RowView { caret_col: Some(0), ..RowView::default() };
            let k = if doc.view == View::Source { Kind::Source } else { Kind::Prose };
            (vec![RowHandle::new(blank)], vec![k], Some(0))
        } else {
            (handles, kinds, caret_row)
        };
        let t1 = Instant::now();

        let mut fonts = st.fonts.borrow_mut();
        let w = column_width(*self.sig.win_w.peek(), st.measures.get().of_view(doc.view), fonts.ty().size);
        if w != *self.sig.width.peek() {
            set(self.sig.width, w);
        }
        let mut layouts = st.layouts.borrow_mut();
        // A new type is a new layout for every row, as a new width is.
        let same = layouts.width == w && layouts.ty == *fonts.ty();
        let mut kept = HashMap::with_capacity(handles.len());
        let mut laid = 0;
        let row_lays: Vec<LayoutHandle> = handles
            .iter()
            .zip(&kinds)
            .map(|(h, &k)| {
                let key = Rc::as_ptr(&h.0);
                let l = match layouts.by_row.get(&key) {
                    Some((_, kind, l)) if same && *kind == k => l.clone(),
                    _ => {
                        laid += 1;
                        LayoutHandle(Rc::new(fonts.layout(&h.0, w, k)))
                    }
                };
                kept.insert(key, (h.clone(), k, l.clone()));
                l
            })
            .collect();
        layouts.by_row = kept;
        layouts.width = w;
        if !same {
            layouts.ty = fonts.ty().clone();
        }

        // What is drawn in place of rows; each stands in for its rows'
        // heights, its first row holding its whole height.
        let blocks = blocks::build(&doc, caret, composer.marked(), &mut fonts, w, &st.block_cache);
        let t2 = Instant::now();
        let mut heights: Vec<f64> = row_lays.iter().map(|l| l.0.height as f64).collect();
        for b in &blocks {
            for r in b.rows() {
                if let Some(h) = heights.get_mut(r) {
                    *h = if r == b.first() { b.height() } else { 0.0 };
                }
            }
        }
        set(self.sig.layout, Layout::of(heights.into_iter()));
        if *self.sig.blocks.peek() != blocks {
            set(self.sig.blocks, blocks);
        }
        set(self.sig.rows, handles);
        set(self.sig.lays, row_lays);
        set(self.sig.kinds, kinds);
        set(self.sig.status, status::line(&doc));
        // The file's name, and "Edited" while it has unsaved changes, as a
        // Mac document window says it.
        let now = match doc.dirty {
            true => format!("{} — Edited", doc.file_name()),
            false => doc.file_name(),
        };
        if *st.title.borrow() != now {
            st.window.set_title(&now);
            *st.title.borrow_mut() = now;
        }
        if self.sig.words.peek().is_none_or(|(r, _)| r != doc.revision()) {
            status::recount(self.st.clone(), doc.revision(), self.sig.words);
        }
        trace(|| {
            format!("repaint: rows {:.1?} text {:.1?} ({laid} rows laid out); {rebuilt} rows rebuilt", t1 - t0, t2 - t1)
        });
        caret_row
    }

    /// Bring the caret's row into view after it moved: core's `follow_caret`,
    /// in pixels, because rows here are not all one line tall. Only after a
    /// key — a scroll with the caret sitting still is the reader's, and leaving
    /// it alone is what lets them scroll past the caret.
    pub fn follow(&self, caret_row: Option<usize>) {
        let Some(r) = caret_row else { return };
        let to = self.sig.layout.peek().reveal(r, *self.sig.top.peek(), self.view_height());
        trace(|| format!("follow caret row {r} -> {to:?}"));
        if let Some(y) = to {
            set(self.sig.top, y);
        }
    }

    /// Repaint, and follow the caret.
    pub fn refresh(&self) {
        let row = self.repaint();
        self.follow(row);
    }

    /// The height the document has to show in: the viewport, less the find
    /// bar when it is open.
    pub fn view_height(&self) -> f64 {
        *self.sig.height.peek() - self.sig.bar.peek().height()
    }

    /// The wheel and the trackpad, in CSS pixels: Blitz's own convention, a
    /// positive delta moving toward the top. Down to where the last row sits
    /// mid-viewport, as a page with room under it.
    pub fn scroll(&self, dy: f64) {
        let max = (self.sig.layout.peek().height() - *self.sig.height.peek() / 2.0).max(0.0);
        let to = (*self.sig.top.peek() - dy).clamp(0.0, max);
        if to != *self.sig.top.peek() {
            set(self.sig.top, to);
        }
    }

    /// The window changed size.
    pub fn resize(&self, physical_width: u32, physical_height: u32) {
        let scale = self.st.window.scale_factor();
        set(self.sig.height, viewport_height(physical_height, scale));
        let w = window_width(physical_width, scale);
        if w != *self.sig.win_w.peek() {
            set(self.sig.win_w, w);
            self.repaint();
        }
    }

    /// Refresh what the find bar draws from its state.
    pub fn sync_bar(&self) {
        let bar = BarView::of(&mut self.st.find.borrow_mut(), &self.st.doc.borrow());
        set(self.sig.bar, bar);
    }

    /// Put a different document in the editor — New, Open — and start every
    /// view of it afresh.
    pub fn install(&self, new: Doc) {
        *self.st.doc.borrow_mut() = new;
        *self.st.composer.borrow_mut() = Composer::default();
        self.st.find.borrow_mut().close();
        self.st.shown.set(None);
        self.st.goal.set(None);
        set(self.sig.top, 0.0);
        set(self.sig.words, None);
        self.refresh();
        self.sync_bar();
    }
}
