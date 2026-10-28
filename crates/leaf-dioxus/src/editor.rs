//! The editor: its state, and the repaint that turns the document into rows.
//!
//! An [`Editor`] is a cheap handle — shared state behind an `Rc`, and the
//! signals the render reads, which are `Copy` — so every event source (the
//! window's keys and IME, the pointer, and whatever the host has: a menu bar,
//! a file dialog's task) holds one and calls a method on it. The methods live
//! beside what they are about: motion and the pointer in `motion.rs`, input
//! in `dispatch.rs`, the find bar in `findbar.rs`, links in `link.rs`.
//!
//! A feature usually needs three things from here: state (a field of
//! [`State`]), something the render reads (a field of [`Signals`]), and a
//! step in [`Editor::repaint`]. Things drawn in place of rows — tables, and
//! whatever comes next — go through `blocks.rs` instead, so they need none of
//! the three.

use std::cell::{Cell, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::bridge::frame::Frame;
use crate::bridge::rows::{Marks, RowHandle, RowView};
use crate::clock::Instant;
use dioxus::prelude::*;
use dioxus_native::winit::window::Window;
use leaf_core::{Doc, View};

use crate::blocks::{self, Block};
use crate::find::{BarView, Find};
use crate::host::Host;
use crate::ime::Composer;
use crate::motion::Press;
use crate::row::{LayoutHandle, PointerSink};
use crate::source::SourceRows;
use crate::status;
use crate::text::{Fonts, Kind, Type};
use crate::trace::trace;
use crate::view::Appearance;
use crate::viewport::{LINE, Layout};

/// The status line's height, in CSS pixels — `.status` in the stylesheet.
pub const STATUS: f64 = 24.0;

/// The source view's measure: 82 code cells, a line wrapped at 80 with room
/// for the caret — which is how most markup is written.
const SOURCE_CELLS: f32 = 82.0;

/// Where the caret is drawn: at `caret`, unless core's map would carry it
/// across a line break. An offset past a hidden closing delimiter at a line's
/// end — after ⌘B wraps the line's last word, `**X**|` — has no stop on its
/// row, and core snaps it forward to the next row's opening; but it is before
/// the newline, so it is drawn at the end of its own line.
pub fn shown_caret(doc: &Doc, caret: usize) -> usize {
    let rows = &doc.vmap.rows;
    let (r, _) = doc.vmap.pos_of_offset(caret);
    let Some(row) = rows.get(r) else { return caret };
    let open = row
        .glyphs
        .iter()
        .find(|g| g.stop)
        .map_or(row.end_src, |g| g.src);
    let crossed = open > caret
        && doc
            .source
            .get(caret..open)
            .is_some_and(|between| between.contains('\n'));
    if !crossed {
        return caret;
    }
    rows[..r]
        .iter()
        .rev()
        .find(|p| !p.decoration && p.end_src <= caret)
        .map_or(caret, |p| p.end_src)
}

/// The editor's own height in CSS pixels: the window's surface, less the
/// status line.
pub fn viewport_height(physical: u32, scale: f64) -> f64 {
    (physical as f64 / scale - STATUS).max(LINE)
}

/// Where the editor is in the window, in CSS pixels from the window's
/// top-left: the box its rows are laid out and scrolled in, the find bar
/// pinned to its top and the status line to its bottom.
///
/// The editor does not clip to it. Blitz paints the first frame of an
/// `overflow: hidden` element and not its changes, and skips a child that
/// hangs past its parent's box (`view.rs`), so the rows scrolled past the
/// box's top are still drawn above it, and those past its bottom below it,
/// as far as the window. A host covers what lies outside the box with its own
/// chrome, each piece in a stacking layer above the editor's — a toolbar over
/// the top, a sidebar beside it — and the window cuts off the rest.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bounds {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

/// What the status line under the editor shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Status {
    /// The file, whether it is saved, the view, a note, and the word count.
    #[default]
    Full,
    /// The word count alone.
    Words,
    /// No status line: the rows run to the bottom of the box.
    Hidden,
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

/// What the editor draws with: the host's display choices, whatever they
/// are made from — a settings file, a zoom, a theme of the host's own.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    /// The faces, the size and the leading, which `text.rs` measures with and
    /// the stylesheet sets.
    pub ty: Type,
    /// How wide the column runs, in characters of the body face — so it
    /// follows the face, the size and a zoom; `None` fills the window.
    pub measure: Option<f32>,
    /// Light or dark: the system's, or one of the two.
    pub appearance: Appearance,
    /// What the status line shows, if there is one.
    pub status: Status,
    /// Whether a scroll bar runs down the box's right edge
    /// ([`unit::ScrollBar`]): not where the editor is drawn in a terminal's
    /// cells, which a bar a few pixels wide does not fit.
    pub scroll_bar: bool,
}

impl Default for Look {
    /// The Swift editor's: Iowan Old Style and Menlo at 16px, a leading of
    /// 1.5, a 68-character column, and the system's appearance.
    fn default() -> Self {
        Self {
            ty: Type::default(),
            measure: Some(68.0),
            appearance: Appearance::System,
            status: Status::Full,
            scroll_bar: true,
        }
    }
}

/// The measure in pixels, for each view.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Measures {
    pub wysiwyg: Option<f32>,
    pub source: f32,
}

impl Measures {
    /// From the measure in characters and the faces as `fonts` sets them.
    pub fn of(measure: Option<f32>, fonts: &mut Fonts) -> Self {
        Self {
            wysiwyg: measure.map(|c| c * fonts.char_width()),
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
    /// The document, which the host may hold too ([`use_shared_editor`]).
    pub doc: Rc<RefCell<Doc>>,
    pub window: Arc<dyn Window>,
    /// Whoever embeds the editor; see `host.rs`.
    pub host: Rc<dyn Host>,
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
    /// The last press, for counting a double- and triple-click.
    pub last_press: Cell<Option<Press>>,
    /// The x a run of ↑/↓ aims for, kept across the lines it passes through
    /// so a short line does not pull the caret left for good.
    pub goal: Cell<Option<f32>>,
    /// The host's lines, while it draws the document laid out its own way:
    /// what ↑/↓, a screen's move and a line's edge go by instead of the rows.
    pub host_lines: RefCell<Option<Rc<dyn crate::host::Lines>>>,
    /// Whether the editor draws the document: not while a host draws it its
    /// own way over the editor (Leaf's pages), when a repaint keeps only what
    /// the keys need, core's map of the text, and the status.
    pub drawn: Cell<bool>,
    /// The column's measure in each view, for the type `fonts` sets.
    pub measures: Cell<Measures>,
    /// The words the host's spelling checker found misspelled, as source
    /// ranges in order, with the revision of the text it read. They are
    /// drawn only while the text is still at that revision: after an edit
    /// they stand where the words were, so they wait for the next check.
    misspelled: RefCell<(u64, Vec<(usize, usize)>)>,
    /// The window's surface, in CSS pixels, from its last resize.
    surface: Cell<(f64, f64)>,
    /// The host's box for the editor, or `None` for the whole window.
    bounds: Cell<Option<Bounds>>,
    /// A finger that went down and has not yet come up; see `touch.rs`.
    pub touch: Cell<Option<crate::touch::Touch>>,
    /// Whether the last pointer was a finger, which decides how a row hears
    /// it: a mouse by its press, a finger by its tap.
    pub touch_mode: Cell<bool>,
    /// Whether the last finger went down on a grab handle: the tap Blitz
    /// sends when it comes up without moving is then not a tap on the text.
    pub gripped: Cell<bool>,
    /// Which ways a sideways drag may turn the page: back, and forward.
    pub turns: Cell<(bool, bool)>,
    /// Every visual line on screen, as Blitz laid it out — what
    /// [`Editor::drawn_rect_of`] measures.
    pub lines: crate::row::LineBoxes,
    /// The finger's recent positions while it scrolls, for its speed when
    /// it lets go; see [`unit::scroll`].
    pub tracker: RefCell<unit::scroll::Tracker>,
    /// Which glide is the page's: each one started, or a finger landing,
    /// moves it on, and a glide that finds it moved stops.
    pub glide: Cell<u64>,
    /// Whether the page was gliding when the last finger went down: that
    /// finger caught it, and the tap Blitz sends as it comes up is not one.
    pub caught: Cell<bool>,
    /// The runtime and the component the editor was made in, where a glide
    /// runs: it ends with them.
    pub home: Option<(Rc<dioxus::core::Runtime>, dioxus::core::ScopeId)>,
    /// The text as assistive tech was last given it, whose positions its
    /// requests name; see `access.rs`.
    pub(crate) described: RefCell<Option<Rc<crate::access::Cached>>>,
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
    /// What the rows are drawn with, which the stylesheet writes out.
    pub look: Signal<Look>,
    /// Where the editor's box starts in the window; see [`Bounds`]. With
    /// `height` and `win_w`, the box.
    pub origin: Signal<(f64, f64)>,
    /// How far the page stands off sideways under a finger turning it.
    pub slide: Signal<f64>,
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
            look: use_signal(Look::default),
            origin: use_signal(|| (0.0, 0.0)),
            slide: use_signal(|| 0.0),
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

/// The editor, as every event source holds it: a cheap handle, cloned into
/// each one.
#[derive(Clone)]
pub struct Editor {
    pub(crate) st: Rc<State>,
    pub(crate) sig: Signals,
}

/// The editor, made as hooks of the calling component, in the window the
/// component is in: on the document `doc` makes, drawn with the [`Look`]
/// `look` makes, asking `host` for what it cannot do itself. Each is called
/// once, on the first render; [`Editor::set_look`] and [`Editor::install`]
/// change the look and the document after.
///
/// The host puts [`Editor::render`] in its page and hands the editor the
/// window's events: keys through [`crate::pressed`] to [`Editor::key`], and
/// the rest to [`Editor::window_event`].
pub fn use_editor(
    doc: impl FnOnce() -> Doc,
    look: impl FnOnce() -> Look,
    host: impl FnOnce() -> Rc<dyn Host>,
) -> Editor {
    use_shared_editor(|| Rc::new(RefCell::new(doc())), look, host)
}

/// [`use_editor`], on a document the host keeps a hold of too — to save it
/// as it changes, or to change it from controls of its own (a formatting
/// bar) and then [`refresh`](Editor::refresh) the editor. The host must not
/// hold it borrowed across a call into the editor.
pub fn use_shared_editor(
    doc: impl FnOnce() -> Rc<RefCell<Doc>>,
    look: impl FnOnce() -> Look,
    host: impl FnOnce() -> Rc<dyn Host>,
) -> Editor {
    let window = dioxus_native::use_window();
    let sig = Signals::new(&window);
    let ed = use_hook(|| {
        let ed = Editor::new(doc(), window.clone(), sig, host());
        // The first paint, and the input method allowed on the window —
        // but under a finger, where allowing it raises the keyboard, that
        // waits for a tap on the page.
        ed.set_look(look());
        if !crate::touch::KEYBOARD_ON_TAP {
            ed.enable_ime();
        }
        // A diagram drawn in the background, arrived: repaint, while the
        // editor lives.
        #[cfg(all(target_arch = "wasm32", feature = "wasm-split"))]
        {
            let (st, sig) = (Rc::downgrade(&ed.st), ed.sig);
            let (runtime, scope) = (
                dioxus::core::Runtime::current(),
                dioxus::core::current_scope_id(),
            );
            crate::picture::on_arrival(move || {
                let Some(st) = st.upgrade() else {
                    return false;
                };
                runtime.in_scope(scope, || Editor { st, sig }.refresh());
                true
            });
        }
        ed
    });
    // Leaving the page lowers the keyboard, and lets another editor in the
    // window ask for the input method.
    use_drop({
        let ed = ed.clone();
        move || ed.disable_ime()
    });
    // The pointer, from every line of every row and block.
    use_context_provider({
        let ed = ed.clone();
        move || PointerSink(Rc::new(move |p| ed.pointer(p)))
    });
    // And where each line was put, from every line of every row.
    use_context_provider({
        let ed = ed.clone();
        move || ed.st.lines.clone()
    });
    ed
}

impl Editor {
    fn new(
        doc: Rc<RefCell<Doc>>,
        window: Arc<dyn Window>,
        sig: Signals,
        host: Rc<dyn Host>,
    ) -> Self {
        let mut fonts = Fonts::new(Type::default());
        let measures = Measures::of(Look::default().measure, &mut fonts);
        let size = window.surface_size();
        let scale = window.scale_factor();
        let st = State {
            doc,
            window,
            host,
            frame: RefCell::default(),
            srows: RefCell::default(),
            shown: Cell::new(None),
            fonts: RefCell::new(fonts),
            layouts: RefCell::default(),
            block_cache: RefCell::default(),
            composer: RefCell::default(),
            find: RefCell::default(),
            last_press: Cell::new(None),
            goal: Cell::new(None),
            host_lines: RefCell::new(None),
            drawn: Cell::new(true),
            measures: Cell::new(measures),
            misspelled: RefCell::default(),
            surface: Cell::new((size.width as f64 / scale, size.height as f64 / scale)),
            bounds: Cell::new(None),
            touch: Cell::new(None),
            touch_mode: Cell::new(false),
            gripped: Cell::new(false),
            turns: Cell::new((false, false)),
            lines: Default::default(),
            tracker: RefCell::default(),
            glide: Cell::new(0),
            caught: Cell::new(false),
            home: dioxus::core::Runtime::try_current()
                .and_then(|rt| rt.try_current_scope_id().map(|id| (rt, id))),
            described: RefCell::default(),
        };
        Self {
            st: Rc::new(st),
            sig,
        }
    }

    /// The first paint, and every repaint after a key, a click or a resize:
    /// the document's rows, their layouts, the blocks drawn in place of some
    /// of them, and the status. Answers the caret's row, for the viewport to
    /// follow.
    pub fn repaint(&self) -> Option<usize> {
        let st = &*self.st;
        let t0 = Instant::now();
        let mut doc = st.doc.borrow_mut();
        // Not drawn: core's map, which the keys move the caret by, and the
        // status, which the host shows; not the rows.
        if !st.drawn.get() {
            // Core's map is built when a key or a press next moves by it
            // (`ensure_map`): a run of letters typed builds none, and in a
            // long document a build is most of what a letter costs.
            set(self.sig.status, status::line(&doc));
            st.host.repainted(&doc);
            if self
                .sig
                .words
                .peek()
                .is_none_or(|(r, _)| r != doc.revision())
            {
                status::recount(self.st.clone(), doc.revision(), self.sig.words);
            }
            trace(|| format!("repaint, not drawn: {:.1?}", t0.elapsed()));
            return None;
        }
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
                let (h, (row, _)) = srows.update(
                    &doc,
                    caret,
                    doc.selection(),
                    composer.marked(),
                    st.touch_mode.get(),
                );
                let h = h.to_vec();
                let kinds = vec![Kind::Source; h.len()];
                (h, kinds, Some(row), srows.rebuilt)
            }
            View::Wysiwyg => {
                // Inline formulas as atoms this editor draws as pictures;
                // a no-op once set.
                doc.set_inline_pictures(true);
                doc.build_visual_unwrapped();
                let shown = shown_caret(&doc, caret);
                let misspelled = st.misspelled.borrow();
                let current = match &*misspelled {
                    (revision, ranges) if *revision == doc.revision() => ranges.as_slice(),
                    _ => &[],
                };
                let marks = Marks::of(&doc.vmap, shown, doc.anchor)
                    .composing(composer.marked())
                    .misspelled(current)
                    .highlighted(doc.highlights())
                    .gripped(&doc.vmap, st.touch_mode.get());
                let h = frame.update(&doc.vmap, &marks).to_vec();
                // A table's picture is box-drawn in character cells, so its
                // rows stay monospace, as code does.
                let mut kinds: Vec<Kind> = h
                    .iter()
                    .map(|r| if r.0.code { Kind::Cells } else { Kind::Prose })
                    .collect();
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
            let blank = RowView {
                caret_col: Some(0),
                ..RowView::default()
            };
            let k = if doc.view == View::Source {
                Kind::Source
            } else {
                Kind::Prose
            };
            (vec![RowHandle::new(blank)], vec![k], Some(0))
        } else {
            (handles, kinds, caret_row)
        };
        let t1 = Instant::now();

        // Inline formulas are boxes in the rows' layout; say which first.
        crate::picture::set_inline(&doc, self.dark());
        let mut fonts = st.fonts.borrow_mut();
        let w = column_width(
            *self.sig.win_w.peek(),
            st.measures.get().of_view(doc.view),
            fonts.ty().size,
        );
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
        let blocks = blocks::build(
            &doc,
            &*st.host,
            caret,
            composer.marked(),
            &mut fonts,
            w,
            self.dark(),
            &st.block_cache,
        );
        let mut blocks = blocks;
        blocks.extend(blocks::directives(
            &doc, &*st.host, &handles, &row_lays, &kinds,
        ));
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
        st.host.repainted(&doc);
        if self
            .sig
            .words
            .peek()
            .is_none_or(|(r, _)| r != doc.revision())
        {
            status::recount(self.st.clone(), doc.revision(), self.sig.words);
        }
        trace(|| {
            format!(
                "repaint: rows {:.1?} text {:.1?} ({laid} rows laid out); {rebuilt} rows rebuilt",
                t1 - t0,
                t2 - t1
            )
        });
        caret_row
    }

    /// Bring the caret's row into view after it moved: core's `follow_caret`,
    /// in pixels, because rows here are not all one line tall. Only after a
    /// key — a scroll with the caret sitting still is the reader's, and leaving
    /// it alone is what lets them scroll past the caret.
    pub fn follow(&self, caret_row: Option<usize>) {
        let Some(r) = caret_row else { return };
        let to = self
            .sig
            .layout
            .peek()
            .reveal(r, *self.sig.top.peek(), self.view_height());
        trace(|| format!("follow caret row {r} -> {to:?}"));
        if let Some(y) = to {
            set(self.sig.top, y);
        }
    }

    /// Whether the editor draws dark: the look's appearance, or the
    /// window's when it follows the system.
    pub fn dark(&self) -> bool {
        use dioxus_native::winit::window::Theme;
        match self.sig.look.peek().appearance {
            Appearance::Dark => true,
            Appearance::Light => false,
            Appearance::System => self.st.window.theme() == Some(Theme::Dark),
        }
    }

    /// Every row's bottom, in CSS pixels from the top of the first — where
    /// a printed page may break. A row a block stands for ends where the
    /// block does.
    pub fn row_bottoms(&self) -> Vec<f64> {
        let l = self.sig.layout.peek();
        (0..l.rows()).map(|r| l.bottom(r)).collect()
    }

    /// The words the host's spelling checker found misspelled in the text at
    /// `revision`, as source ranges; drawn underlined until the next edit.
    pub fn set_misspellings(&self, revision: u64, mut ranges: Vec<(usize, usize)>) {
        ranges.sort_unstable();
        *self.st.misspelled.borrow_mut() = (revision, ranges);
        self.refresh();
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
        let mut s = self.scroller();
        s.wheel(0.0, dy);
        self.scroll_to(s.y);
    }

    /// The viewport over the rows, as every view of leaf's scrolls: with
    /// half a viewport of room under the last row, so that it can come up
    /// to the middle.
    pub(crate) fn scroller(&self) -> unit::scroll::Scroll {
        let height = *self.sig.height.peek();
        unit::scroll::Scroll {
            y: *self.sig.top.peek(),
            view: (0.0, height),
            content: (0.0, self.sig.layout.peek().height()),
            slack: height / 2.0,
            ..Default::default()
        }
    }

    /// Where the viewport starts, in CSS pixels down the document.
    pub fn top(&self) -> f64 {
        *self.sig.top.peek()
    }

    /// Start the viewport `y` down the document, unclamped: a host that
    /// scrolls the editor (an `NSScrollView`) takes it past either end and
    /// back, as its elasticity does.
    pub fn scroll_to(&self, y: f64) {
        if y != *self.sig.top.peek() {
            set(self.sig.top, y);
        }
    }

    /// The furthest down the viewport goes: to where the last row sits
    /// mid-viewport.
    pub fn max_top(&self) -> f64 {
        self.scroller().range().1.1
    }

    /// How tall the scrolled content is, for a host that scrolls the editor:
    /// as tall as the whole surface at the furthest scroll, so that a
    /// scroll view's range is the editor's own.
    pub fn scroll_height(&self) -> f64 {
        self.max_top() + *self.sig.height.peek() + self.status_height()
    }

    /// How tall the status line is: [`STATUS`], or a row on a grid, or
    /// nothing when the look hides it.
    pub fn status_height(&self) -> f64 {
        match self.sig.look.peek().status {
            Status::Hidden => 0.0,
            Status::Full | Status::Words => self.chrome_row(STATUS),
        }
    }

    /// How tall a row of the editor's own chrome is: `pixels` in a window,
    /// one cell on a grid ([`Type::grid`](crate::Type::grid)).
    fn chrome_row(&self, pixels: f64) -> f64 {
        self.sig
            .look
            .peek()
            .ty
            .grid
            .map_or(pixels, |g| g.height as f64)
    }

    /// Where the text starts in the window, in CSS pixels: past the box's
    /// left edge and the side padding, and past half of whatever room the
    /// column leaves, as it is centred in the box.
    pub fn left(&self) -> f64 {
        self.column_left() + crate::row::PAD
    }

    /// Where the column — the text and its side padding — starts in the
    /// window, in CSS pixels.
    pub fn column_left(&self) -> f64 {
        let column = *self.sig.width.peek() + 2.0 * crate::row::PAD as f32;
        self.sig.origin.peek().0 + ((*self.sig.win_w.peek() - column) / 2.0).max(0.0) as f64
    }

    /// The editor's box in the window, as it stands: the host's, or the
    /// window's surface.
    pub fn bounds(&self) -> Bounds {
        self.st.bounds.get().unwrap_or_else(|| {
            let (width, height) = self.st.surface.get();
            Bounds {
                left: 0.0,
                top: 0.0,
                width,
                height,
            }
        })
    }

    /// The window the editor is in: for a host with more than one, to find
    /// which the editor's requests come from.
    pub fn window_id(&self) -> dioxus_native::winit::window::WindowId {
        self.st.window.id()
    }

    /// Move by the host's lines rather than the rows (see
    /// [`crate::host::Lines`]), or, with `None`, by the rows again.
    pub fn set_lines(&self, lines: Option<Rc<dyn crate::host::Lines>>) {
        *self.st.host_lines.borrow_mut() = lines;
        self.st.goal.set(None);
    }

    /// Bring core's map of the text up to date, if the editor is not drawn
    /// and left it behind: before a key or a press that moves by it. Drawn,
    /// every repaint keeps it, and this is nothing.
    pub(crate) fn ensure_map(&self) {
        if self.st.drawn.get() {
            return;
        }
        let mut doc = self.st.doc.borrow_mut();
        match doc.view {
            View::Source => doc.build_source(),
            View::Wysiwyg => doc.build_visual_unwrapped(),
        }
    }

    /// Draw the document, or not: a host that draws it its own way over the
    /// editor turns the editor's drawing off, and a keystroke no longer lays
    /// out the editor's rows, which in a long document is most of what one
    /// costs. Drawn again, the rows are brought up to date.
    pub fn set_drawn(&self, drawn: bool) {
        if self.st.drawn.replace(drawn) != drawn && drawn {
            self.repaint();
        }
    }

    /// Lay the editor out in `bounds` from now on — a host with a sidebar
    /// or a toolbar beside it, or a keyboard over its bottom — or, with
    /// `None`, in the whole window again. See [`Bounds`] for what the host
    /// owes it. A box that grows narrower or wider lays the rows out again;
    /// one that grows shorter keeps the caret in view.
    pub fn set_bounds(&self, bounds: Option<Bounds>) {
        if self.st.bounds.get() != bounds {
            self.st.bounds.set(bounds);
            self.place();
        }
    }

    /// Bring the viewport's signals up to date with the box.
    fn place(&self) {
        let b = self.bounds();
        if *self.sig.origin.peek() != (b.left, b.top) {
            set(self.sig.origin, (b.left, b.top));
        }
        let height = (b.height - self.status_height()).max(LINE);
        let shorter = height < *self.sig.height.peek();
        if height != *self.sig.height.peek() {
            set(self.sig.height, height);
        }
        let w = b.width as f32;
        if w != *self.sig.win_w.peek() {
            set(self.sig.win_w, w);
            self.refresh();
        } else if shorter {
            self.follow(self.caret_at().map(|(row, ..)| row));
        }
    }

    /// The document, as the editor and any host that shares it hold it.
    pub fn shared_doc(&self) -> Rc<RefCell<Doc>> {
        self.st.doc.clone()
    }

    /// Which ways a sideways drag of a finger may turn the page — back, and
    /// forward. A drag let go far enough that way is [`Host::turn`].
    pub fn set_turns(&self, back: bool, forward: bool) {
        self.st.turns.set((back, forward));
    }

    /// The find bar's height, open or not: what covers the top of the
    /// viewport, as [`STATUS`] covers its bottom.
    pub fn bar_height(&self) -> f64 {
        self.sig.bar.peek().height()
    }

    /// The window changed size. With no box of the host's, the editor's
    /// changes with it.
    pub fn resize(&self, physical_width: u32, physical_height: u32) {
        let scale = self.st.window.scale_factor();
        self.st.surface.set((
            window_width(physical_width, scale) as f64,
            physical_height as f64 / scale,
        ));
        self.place();
    }

    /// Refresh what the find bar draws from its state.
    pub fn sync_bar(&self) {
        let row = self.chrome_row(crate::find::ROW);
        let bar = BarView::of(&mut self.st.find.borrow_mut(), &self.st.doc.borrow(), row);
        set(self.sig.bar, bar);
    }

    /// The document, to read or change. A change is drawn on the next
    /// [`repaint`](Self::repaint) or [`refresh`](Self::refresh).
    pub fn doc(&self) -> RefMut<'_, Doc> {
        self.st.doc.borrow_mut()
    }

    /// Draw with `look` from now on: the stylesheet and every row's layout
    /// made again for it in one repaint, so the rows are laid out for exactly
    /// the faces, size and leading Blitz then draws them in — which is what
    /// keeps a click on the glyph under it.
    pub fn set_look(&self, look: Look) {
        let st = &*self.st;
        {
            let mut fonts = st.fonts.borrow_mut();
            fonts.set_ty(look.ty.clone());
            st.measures.set(Measures::of(look.measure, &mut fonts));
        }
        // Tables keep their layouts by width alone; a new type is new ones.
        st.block_cache.borrow_mut().clear();
        trace(|| format!("look {look:?}, measures {:?}", st.measures.get()));
        // A new appearance re-inks the formulas in every row.
        if self.sig.look.peek().appearance != look.appearance {
            self.reink();
        }
        set(self.sig.look, look);
        // A status line shown or hidden is a taller or shorter viewport.
        self.place();
        self.refresh();
    }

    /// Draw every row again, inked for the appearance as it now stands: an
    /// inline formula's picture is part of its row's layout, and a row whose
    /// text has not changed keeps both its layout and its markup.
    pub(crate) fn reink(&self) {
        *self.st.layouts.borrow_mut() = Layouts::default();
        self.st.shown.set(None);
    }

    /// Whether this machine has a font family by `name` — for a host's font
    /// menu to offer only those installed.
    pub fn has_family(&self, name: &str) -> bool {
        self.st.fonts.borrow_mut().has_family(name)
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

#[cfg(test)]
mod tests {
    use leaf_core::{Doc, Format, InlineKind};

    use super::shown_caret;

    #[test]
    fn a_caret_past_a_hidden_closing_delimiter_stays_on_its_line() {
        let src = "Hello there! X\n\nhello\n";
        let mut d = Doc::from_source(src.into(), Format::Markdown).expect("a document");
        d.build_visual_unwrapped();
        let x = src.find('X').unwrap();
        d.select_range(x + 1, x);
        d.toggle(InlineKind::Strong);
        d.build_visual_unwrapped();
        let shown = shown_caret(&d, d.caret);
        assert_eq!(
            d.vmap.pos_of_offset(shown).0,
            0,
            "the caret is drawn on the bolded line"
        );
        // And a caret already at a stop is drawn where it is.
        assert_eq!(shown_caret(&d, 3), 3);
    }
}
