//! What a keystroke costs through Dioxus's virtual DOM — headless, with no
//! browser and no `dx`.
//!
//! A virtual DOM's output is a list of mutations for whatever draws the real
//! tree, and `dioxus_core` hands them to a [`WriteMutations`] of the caller's
//! choosing. So the cost of a keystroke can be both *timed* and *counted*
//! exactly, on any machine, by handing it a counter: the mutation count is
//! what would cross into JS, and the time is the diff itself.
//!
//! Three renderers over the same documents, the same edits, and the same
//! leaf-core frame:
//!
//! | mode | what it is |
//! |------|-----------|
//! | `rows` | a memoized component per row, no offsets in the DOM — the renderer to use |
//! | `flat` | the same markup inline, no per-row component: a virtual DOM unaided |
//! | `offsets` | `rows`, but each run carries its `data-src` |
//!
//! ```sh
//! cargo run --release --bin bench            # the table in README.md
//! cargo run --release --bin bench -- --ssr   # …and the DOM of a small document
//! ```

use std::time::{Duration, Instant};

use dioxus_core::{ElementId, Template, VirtualDom, WriteMutations};
use dioxus_widget_spike::editor::{Document, FlatDocument, SharedRows, UnkeyedDocument};
use dioxus_widget_spike::frame::Frame;
use dioxus_widget_spike::rows::Marks;
use leaf_core::{Doc, Format};

/// The soft-wrap width every measurement is taken at.
const WRAP: usize = 80;
/// Keystrokes per measurement. Enough to average over, few enough that the
/// document does not change size materially while typing.
const KEYSTROKES: usize = 200;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let corpus = include_str!("../../corpus/document.md");

    if args.iter().any(|a| a == "--ssr") {
        print_dom(corpus);
        return;
    }

    println!(
        "leaf through Dioxus {}: {KEYSTROKES} keystrokes per row, wrap {WRAP}, \
         release build\n",
        dioxus_version()
    );
    println!(
        "{:<9} {:>6} {:<8} {:>7} {:>8} {:>9} {:>8} {:>8} {:>8}",
        "mode", "rows", "gesture", "rebuilt", "core ms", "frame ms", "dx ms", "muts", "first ms"
    );
    println!("{}", "-".repeat(80));

    // Three document sizes: a page, a chapter, and a document long enough that
    // anything O(document) per keystroke is unmistakable.
    for copies in [1, 20, 200] {
        let source = corpus.repeat(copies);
        for mode in [
            Mode::Rows,
            Mode::Unkeyed,
            Mode::Flat,
            Mode::Offsets,
            Mode::Window,
        ] {
            let r = measure(&source, mode);
            for (i, (gesture, t)) in r.by.iter().enumerate() {
                println!(
                    "{:<9} {:>6} {:<8} {:>7} {:>8.2} {:>9.2} {:>8.2} {:>8} {:>8}",
                    if i == 0 { mode.name() } else { "" },
                    if i == 0 {
                        r.rows.to_string()
                    } else {
                        String::new()
                    },
                    format!("{gesture:?}").to_lowercase(),
                    t.rebuilt,
                    ms(t.core),
                    ms(t.frame),
                    ms(t.dioxus),
                    t.mutations,
                    if i == 0 {
                        format!("{:.1}", ms(r.first))
                    } else {
                        String::new()
                    },
                );
            }
        }
        println!();
    }

    println!(
        "gesture   type: a character, the row count unchanged. reflow: a \
         character that\n\
         \x20         wrapped, so a row was inserted or removed. move: an \
         arrow key, no edit.\n\
         rebuilt   rows whose handle the frame renewed — what Dioxus had to \
         render.\n\
         core ms   leaf-core's own work: the edit and the AST→glyph rebuild.\n\
         frame ms  this crate's bookkeeping: a fingerprint per row, the delta, \
         the row views.\n\
         dx ms     Dioxus: render the dirty scopes and diff them. The column \
         the spike is about.\n\
         muts      mutations the diff emitted — what would cross into JS.\n\
         first ms  the initial rebuild, for scale."
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Rows,
    Flat,
    Offsets,
    Window,
    Unkeyed,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Rows => "rows",
            Mode::Flat => "flat",
            Mode::Offsets => "offsets",
            Mode::Window => "window",
            Mode::Unkeyed => "unkeyed",
        }
    }
}

/// The viewport [`Mode::Window`] renders, in rows. A screen's worth.
const WINDOW: usize = 50;

/// What kind of gesture a measurement is of. The distinction matters more than
/// anything else the bench reports: a keystroke that reflows its paragraph
/// changes the *number* of rows, and a virtual DOM cannot splice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Gesture {
    /// A character typed, the row count unchanged.
    Type,
    /// A character typed that wrapped, so a row was inserted or removed.
    Reflow,
    /// An arrow key: no text change at all.
    Move,
}

/// One gesture's averages.
#[derive(Clone, Copy, Default)]
struct Tally {
    n: usize,
    rebuilt: usize,
    core: Duration,
    frame: Duration,
    dioxus: Duration,
    mutations: usize,
}

impl Tally {
    fn avg(&self) -> Self {
        let n = self.n.max(1);
        Self {
            n: self.n,
            rebuilt: self.rebuilt / n,
            core: self.core / n as u32,
            frame: self.frame / n as u32,
            dioxus: self.dioxus / n as u32,
            mutations: self.mutations / n,
        }
    }
}

struct Result {
    rows: usize,
    first: Duration,
    by: Vec<(Gesture, Tally)>,
}

fn measure(source: &str, mode: Mode) -> Result {
    let mut doc = Doc::from_source(source.to_string(), Format::Markdown).expect("a document");
    doc.build_visual(WRAP);

    let mut frame = if mode == Mode::Offsets {
        Frame::with_offsets()
    } else {
        Frame::default()
    };
    let rows = SharedRows::default();
    let marks = Marks::of(&doc.vmap, doc.caret, doc.anchor);
    let handles = frame.update(&doc.vmap, &marks).to_vec();
    let row_count = doc.vmap.rows.len();
    // Type into the middle of the document, where there are rows above and
    // below to be wrongly invalidated.
    let middle = row_count / 2;
    set_rows(&rows, &handles, mode, middle);

    let app = match mode {
        Mode::Flat => FlatDocument,
        Mode::Unkeyed => UnkeyedDocument,
        _ => Document,
    };
    let mut vdom = VirtualDom::new(app).with_root_context(rows.clone());

    let mut counter = Counter::default();
    let start = Instant::now();
    vdom.rebuild(&mut counter);
    let first = start.elapsed();

    let offset = doc.vmap.offset_of_pos(middle, 0);
    doc.place_caret(offset, false);

    let mut typed = Tally::default();
    let mut reflow = Tally::default();
    let mut moved = Tally::default();

    for i in 0..KEYSTROKES {
        // A word's worth of characters, then a space, so some keystrokes wrap
        // the paragraph and most do not.
        let ch = if i % 6 == 5 { " " } else { "a" };
        let before = doc.vmap.rows.len();

        let t = Instant::now();
        doc.insert(ch);
        doc.build_visual(WRAP);
        let core = t.elapsed();

        let t = Instant::now();
        let marks = Marks::of(&doc.vmap, doc.caret, doc.anchor);
        let handles = frame.update(&doc.vmap, &marks).to_vec();
        let caret_row = marks.caret.map_or(middle, |(r, _)| r);
        set_rows(&rows, &handles, mode, caret_row);
        let frame_time = t.elapsed();

        counter.reset();
        let t = Instant::now();
        vdom.mark_dirty(dioxus_core::ScopeId::APP);
        vdom.render_immediate(&mut counter);
        let dx = t.elapsed();

        let into = if doc.vmap.rows.len() == before {
            &mut typed
        } else {
            &mut reflow
        };
        into.n += 1;
        into.rebuilt += frame.last.rebuilt;
        into.core += core;
        into.frame += frame_time;
        into.dioxus += dx;
        into.mutations += counter.total;
    }

    // Arrow keys: the document does not change at all, so whatever this costs
    // is what a virtual DOM charges for a document merely being long.
    for _ in 0..KEYSTROKES {
        let t = Instant::now();
        doc.move_left(false);
        doc.build_visual(WRAP);
        let core = t.elapsed();

        let t = Instant::now();
        let marks = Marks::of(&doc.vmap, doc.caret, doc.anchor);
        let handles = frame.update(&doc.vmap, &marks).to_vec();
        let caret_row = marks.caret.map_or(middle, |(r, _)| r);
        set_rows(&rows, &handles, mode, caret_row);
        let frame_time = t.elapsed();

        counter.reset();
        let t = Instant::now();
        vdom.mark_dirty(dioxus_core::ScopeId::APP);
        vdom.render_immediate(&mut counter);
        let dx = t.elapsed();

        moved.n += 1;
        moved.rebuilt += frame.last.rebuilt;
        moved.core += core;
        moved.frame += frame_time;
        moved.dioxus += dx;
        moved.mutations += counter.total;
    }

    Result {
        rows: row_count,
        first,
        by: vec![
            (Gesture::Type, typed.avg()),
            (Gesture::Reflow, reflow.avg()),
            (Gesture::Move, moved.avg()),
        ],
    }
}

/// Hand the component the rows it draws: all of them, or — under
/// [`Mode::Window`] — a screen's worth around the caret, which is what a
/// virtualized renderer would do.
fn set_rows(
    rows: &SharedRows,
    handles: &[dioxus_widget_spike::RowHandle],
    mode: Mode,
    caret_row: usize,
) {
    if mode == Mode::Window {
        let top = caret_row.saturating_sub(WINDOW / 2).min(handles.len());
        let bottom = (top + WINDOW).min(handles.len());
        rows.set(&handles[top..bottom]);
    } else {
        rows.set(handles);
    }
}

/// Counts the mutations a frame emits, which is what would cross into JS.
#[derive(Default)]
struct Counter {
    total: usize,
}

impl Counter {
    fn reset(&mut self) {
        self.total = 0;
    }
}

impl WriteMutations for Counter {
    fn append_children(&mut self, _: ElementId, _: usize) {
        self.total += 1;
    }
    fn assign_node_id(&mut self, _: &'static [u8], _: ElementId) {
        self.total += 1;
    }
    fn create_placeholder(&mut self, _: ElementId) {
        self.total += 1;
    }
    fn create_text_node(&mut self, _: &str, _: ElementId) {
        self.total += 1;
    }
    fn load_template(&mut self, _: Template, _: usize, _: ElementId) {
        self.total += 1;
    }
    fn replace_node_with(&mut self, _: ElementId, _: usize) {
        self.total += 1;
    }
    fn replace_placeholder_with_nodes(&mut self, _: &'static [u8], _: usize) {
        self.total += 1;
    }
    fn insert_nodes_after(&mut self, _: ElementId, _: usize) {
        self.total += 1;
    }
    fn insert_nodes_before(&mut self, _: ElementId, _: usize) {
        self.total += 1;
    }
    fn set_attribute(
        &mut self,
        _: &'static str,
        _: Option<&'static str>,
        _: &dioxus_core::AttributeValue,
        _: ElementId,
    ) {
        self.total += 1;
    }
    fn set_node_text(&mut self, _: &str, _: ElementId) {
        self.total += 1;
    }
    fn create_event_listener(&mut self, _: &'static str, _: ElementId) {
        self.total += 1;
    }
    fn remove_event_listener(&mut self, _: &'static str, _: ElementId) {
        self.total += 1;
    }
    fn remove_node(&mut self, _: ElementId) {
        self.total += 1;
    }
    fn push_root(&mut self, _: ElementId) {
        self.total += 1;
    }
}

/// The DOM a small document renders to, so the markup in README.md is the
/// markup the code produces rather than a transcription of it.
fn print_dom(corpus: &str) {
    let mut doc = Doc::from_source(corpus.to_string(), Format::Markdown).expect("a document");
    doc.build_visual(WRAP);
    doc.place_caret(2, false);
    let mut frame = Frame::default();
    let rows = SharedRows::default();
    let marks = Marks::of(&doc.vmap, doc.caret, doc.anchor);
    rows.set(frame.update(&doc.vmap, &marks));

    let mut vdom = VirtualDom::new(Document).with_root_context(rows);
    vdom.rebuild_in_place();
    let html = dioxus_ssr::render(&vdom);
    // The first rows are enough to show the shape; the whole document is
    // thousands of spans.
    for (i, part) in html.split("<div class=\"row").take(8).enumerate() {
        if i == 0 {
            println!("{part}");
        } else {
            println!("<div class=\"row{part}");
        }
    }
    println!("…");
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn dioxus_version() -> &'static str {
    "0.7.10"
}
