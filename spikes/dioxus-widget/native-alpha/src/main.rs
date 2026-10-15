//! The same leaf rows through Dioxus's **experimental native backend**
//! (`dioxus-native` 0.8.0-alpha.2, Blitz): an HTML/CSS renderer on wgpu, with
//! no webview and no browser engine.
//!
//! ```sh
//! cargo run --release -- ../corpus/document.md
//! ```
//!
//! An editor, not just a first paint, working toward the Swift editor
//! (`apps/leaf-editor`): proportional text wrapped and hit-tested by parley
//! ([`text`]), its key bindings ([`input`]), the clipboard and the file
//! ([`host`]), click, double- and triple-click, drag-select and task boxes,
//! a status line — over the parent's [`Frame`], so a keystroke renders one
//! row and memoizes the rest, and only the rows in view ([`viewport`]).
//!
//! `LEAF_TRACE=1` logs keys, pointer events and repaint timings to stderr;
//! `LEAF_OUTLINE=1` outlines every visual line and run.
//!
//! The markup is duplicated from the parent's `editor.rs` rather than shared,
//! because `rsx!` builds types from *its* Dioxus and the two versions cannot
//! meet. Everything underneath it — the glyph→run coalescing, the class
//! mapping, the frame delta — is the parent crate's, which is the point: the
//! bridge is independent of which Dioxus renders it.

mod file;
mod find;
mod host;
mod ime;
mod input;
mod keys;
mod menu;
mod row;
mod source;
mod text;
mod viewport;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dioxus::html::geometry::WheelDelta;
use dioxus::prelude::*;
use dioxus::html::{Code, Key, Modifiers};
use dioxus_native::winit;
use dioxus_native::winit::dpi::{LogicalPosition, LogicalSize};
use dioxus_native::winit::event::{Ime, WindowEvent};
use dioxus_native::winit::window::{
    ImeCapabilities, ImeEnableRequest, ImeRequest, ImeRequestData, ImeSurroundingText,
};
use dioxus_widget_spike::frame::Frame;
use dioxus_widget_spike::rows::{Marks, RowHandle, RowView};
use dioxus_widget_spike::style::STYLESHEET;
use leaf_core::{Doc, Format, View};

use crate::file::Unsaved;
use crate::find::{BarView, Find, Focus, Outcome};
use crate::input::Host;
use crate::menu::Command as MenuCommand;
use crate::ime::Composer;
use crate::input::{Action, FindCommand, apply_key};
use crate::row::{LayoutHandle, Pointer, PointerSink, Row};
use crate::source::SourceRows;
use crate::text::{BODY, Fonts, Kind, MONO, SIZE};
use crate::viewport::{LINE, Layout};

/// The status line's height, in CSS pixels — `.status` in [`layout_css`].
const STATUS: f64 = 24.0;

/// Presses closer together than this, on the same row, count as one double-
/// or triple-click — the system default on macOS.
const MULTI_CLICK: Duration = Duration::from_millis(500);

/// What this binary adds to the parent's stylesheet: proportional body text
/// with code and tables in cells, in the families `text.rs` measures with;
/// `.leaf` as a viewport the editor scrolls itself (see `App`); a known left
/// edge for [`row::PAD`]; the inline caret `row.rs` draws; the status line.
///
/// A function only because the families are `text.rs`'s constants, and the
/// two must never disagree: a face Blitz draws in and parley does not
/// measure is every click landing a few glyphs off.
//
// Two Blitz behaviours shape this, both found the hard way:
//
// - It paints the first frame of an `overflow: hidden` element and not the
//   changes after it, so the clip is on `body`, never on `.leaf`.
// - It skips painting a child that hangs past its parent's box, so `.leaf`
//   has no fixed height: its box is as tall as the rows in it, shifted up by
//   the scroll, and the window cuts it off. The status line is pinned to the
//   window's bottom over it, in a stacking layer of its own.
fn layout_css() -> String {
    format!(
        "html, body {{ margin: 0; overflow: hidden; height: 100vh; }}
body {{ position: relative; }}
.leaf {{ padding: 0 16px; box-sizing: border-box; font-family: {BODY}; font-size: {SIZE}px; }}
.leaf .row.code, .leaf .row.table, .leaf .row.source, .leaf .code {{ font-family: {MONO}; }}
.leaf .row.source .g {{ font-size: 1em; vertical-align: baseline; }}
.leaf .line {{ min-height: 1.5em; }}
.leaf .ime {{ text-decoration: underline; text-decoration-color: #1a73e8; }}
{REST}"
    )
}

const REST: &str = "
.leaf .caret-in { display: inline-block; width: 0; height: 1.2em;
                  border-left: 2px solid #1a73e8; margin-right: -2px;
                  vertical-align: text-bottom; }
.status { position: absolute; left: 0; right: 0; bottom: 0; z-index: 1;
          height: 24px; line-height: 24px; padding: 0 16px; box-sizing: border-box;
          font: 12px ui-monospace, monospace; color: #5f6368; background: #f6f8fa;
          border-top: 1px solid #e1e4e8; display: flex; gap: 16px; white-space: pre; }
.status .grow { flex: 1; }
.find { position: absolute; left: 0; right: 0; top: 0; z-index: 1; box-sizing: border-box;
        padding: 0 16px; background: #f6f8fa; border-bottom: 1px solid #e1e4e8;
        font: 13px system-ui, sans-serif; color: #24292f; }
.find .find-row { display: flex; align-items: center; gap: 8px; height: 34px; }
.find .label { width: 56px; color: #5f6368; }
.find .field { flex: 1; height: 22px; line-height: 22px; padding: 0 6px; white-space: pre;
               background: #fff; border: 1px solid #c8ccd0; border-radius: 5px; overflow: hidden; }
.find .field.focused { border-color: #1a73e8; }
.find .count { min-width: 72px; color: #5f6368; text-align: right; }
.find .btn { height: 22px; line-height: 22px; padding: 0 9px; background: #fff;
             border: 1px solid #c8ccd0; border-radius: 5px; }
.find .caret-in { display: inline-block; width: 0; height: 16px; vertical-align: text-bottom;
                  border-left: 2px solid #1a73e8; margin-right: -2px; }";

fn main() {
    let path = std::env::args().nth(1).map(PathBuf::from);
    dioxus_native::launch_cfg(
        App,
        vec![Box::new(move || Box::new(Opened(path.clone())))],
        vec![],
    );
}

/// The file named on the command line, if any.
#[derive(Clone)]
struct Opened(Option<PathBuf>);

/// The document opened, or the corpus as an untitled one when no file was
/// named or it would not open.
fn open(path: Option<PathBuf>) -> Doc {
    if let Some(p) = path {
        match Doc::open(p.clone()) {
            Ok(d) => return d,
            Err(e) => eprintln!("{}: {e}", p.display()),
        }
    }
    let source = include_str!("../../corpus/document.md").to_string();
    Doc::from_source(source, Format::Markdown).unwrap_or_else(|_| Doc::blank().expect("a blank document"))
}

/// The editor's own height in CSS pixels: the window's surface, less the
/// status line.
fn viewport_height(physical: u32, scale: f64) -> f64 {
    (physical as f64 / scale - STATUS).max(LINE)
}

/// The window's width in CSS pixels.
fn window_width(physical: u32, scale: f64) -> f32 {
    (physical as f64 / scale) as f32
}

/// The text column's width in CSS pixels: the window less the side padding,
/// up to a measure that reads as prose — the Swift editor's column width —
/// or, in the source view, one that fits a line wrapped at 80 columns, which
/// is how most markup is written.
fn column_width(window: f32, view: View) -> f32 {
    let measure = match view {
        View::Wysiwyg => MEASURE,
        View::Source => SOURCE_MEASURE,
    };
    (window - 2.0 * row::PAD as f32).clamp(SIZE * 10.0, measure)
}

/// The widest the text column grows, in CSS pixels.
const MEASURE: f32 = 680.0;

/// The source view's: 82 Menlo cells (0.602em each) at the body size.
const SOURCE_MEASURE: f32 = 82.0 * 0.602 * SIZE;

/// The last press, for counting a double- and triple-click.
#[derive(Clone, Copy)]
struct Press {
    at: Instant,
    row: usize,
    col: usize,
    count: u8,
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
    by_row: HashMap<*const RowView, (RowHandle, Kind, LayoutHandle)>,
}

#[allow(non_snake_case)]
fn App() -> Element {
    let path = use_context::<Opened>().0;
    let doc = use_hook(|| {
        let mut d = open(path);
        d.build_visual_unwrapped();
        Rc::new(RefCell::new(d))
    });
    let frame = use_hook(|| Rc::new(RefCell::new(Frame::default())));
    let fonts = use_hook(|| Rc::new(RefCell::new(Fonts::new())));
    let cache = use_hook(|| Rc::new(RefCell::new(Layouts::default())));
    let srows = use_hook(|| Rc::new(RefCell::new(SourceRows::default())));
    // The view the rows were last built for, to start afresh on a switch.
    let shown = use_hook(|| Rc::new(Cell::new(None::<View>)));
    let composer = use_hook(|| Rc::new(RefCell::new(Composer::default())));
    let find = use_hook(|| Rc::new(RefCell::new(Find::default())));
    // What the find bar draws, refreshed whenever it or the document changes.
    let mut bar = use_signal(BarView::default);
    // The window title last set, so it is only set when it changes.
    let title = use_hook(|| Rc::new(RefCell::new(String::new())));
    let last_press = use_hook(|| Rc::new(Cell::new(None::<Press>)));
    // The x a run of ↑/↓ aims for, kept across the lines it passes through
    // so a short line does not pull the caret left for good.
    let goal = use_hook(|| Rc::new(Cell::new(None::<f32>)));
    // The word count, as of a revision; see `recount`.
    let words = use_signal(|| None::<(u64, usize)>);
    let mut rows = use_signal(Vec::<RowHandle>::new);
    let mut lays = use_signal(Vec::<LayoutHandle>::new);
    let mut row_kinds = use_signal(Vec::<Kind>::new);
    let mut layout = use_signal(Layout::default);
    let mut status = use_signal(String::new);
    // The viewport: how far down the document it starts, how tall it is, and
    // how wide the text column is.
    //
    // The editor scrolls itself rather than letting `.leaf` overflow. Blitz
    // can scroll an element, but only from a task — `MountedData::scroll` —
    // and dioxus-native polls tasks while Blitz holds its document borrowed,
    // so following the caret that way panicked ("RefCell already borrowed").
    // Owning the offset makes following the caret a signal write, and makes
    // the virtualized window exact: the rows drawn are the rows in view.
    //
    // The size is the window's, from winit: dioxus-native implements neither
    // `onresize` nor a measurement outside a task.
    let window = dioxus_native::use_window();
    let mut top = use_signal(|| 0.0_f64);
    let mut height = use_signal(|| viewport_height(window.surface_size().height, window.scale_factor()));
    let mut win_w = use_signal(|| window_width(window.surface_size().width, window.scale_factor()));
    // The column the rows were last laid out for; set by `repaint`.
    let mut width = use_signal(|| MEASURE);

    // The first paint, and every repaint after a key, a click or a resize.
    // Answers the caret's row, for the viewport to follow.
    let repaint = {
        let doc_rc = doc.clone();
        let (doc, frame, fonts, cache) = (doc.clone(), frame.clone(), fonts.clone(), cache.clone());
        let (composer, srows, shown) = (composer.clone(), srows.clone(), shown.clone());
        let (window, title) = (window.clone(), title.clone());
        move || -> Option<usize> {
            let t0 = Instant::now();
            let mut doc = doc.borrow_mut();
            // While composing, the caret is drawn at the input method's
            // cursor and the marked text underlined; see `ime.rs`.
            let composer = composer.borrow();
            let caret = composer.caret().unwrap_or(doc.caret);
            let mut frame = frame.borrow_mut();
            let mut srows = srows.borrow_mut();
            // A switch of view shares no rows with the view before it.
            if shown.get() != Some(doc.view) {
                *frame = Frame::default();
                srows.clear();
                shown.set(Some(doc.view));
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
                    // A table's picture is box-drawn in character cells, so
                    // its rows stay monospace, as code does.
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
            let w = column_width(*win_w.peek(), doc.view);
            if w != *width.peek() {
                width.set(w);
            }
            let mut cache = cache.borrow_mut();
            let mut fonts = fonts.borrow_mut();
            let mut kept = HashMap::with_capacity(handles.len());
            let mut laid = 0;
            let row_lays: Vec<LayoutHandle> = handles
                .iter()
                .zip(&kinds)
                .map(|(h, &k)| {
                    let key = Rc::as_ptr(&h.0);
                    let l = match cache.by_row.get(&key) {
                        Some((_, kind, l)) if cache.width == w && *kind == k => l.clone(),
                        _ => {
                            laid += 1;
                            LayoutHandle(Rc::new(fonts.layout(&h.0, w, k)))
                        }
                    };
                    kept.insert(key, (h.clone(), k, l.clone()));
                    l
                })
                .collect();
            cache.by_row = kept;
            cache.width = w;
            let t2 = Instant::now();

            layout.set(Layout::of(row_lays.iter().map(|l| l.0.height as f64)));
            rows.set(handles);
            lays.set(row_lays);
            row_kinds.set(kinds);
            status.set(status_line(&doc));
            // The file's name, and "Edited" while it has unsaved changes, as
            // a Mac document window says it.
            let now = match doc.dirty {
                true => format!("{} — Edited", doc.file_name()),
                false => doc.file_name(),
            };
            if *title.borrow() != now {
                window.set_title(&now);
                *title.borrow_mut() = now;
            }
            if words.peek().is_none_or(|(r, _)| r != doc.revision()) {
                recount(doc_rc.clone(), doc.revision(), words);
            }
            trace(|| format!("repaint: rows {:.1?} text {:.1?} ({laid} rows laid out); {rebuilt} rows rebuilt",
                t1 - t0, t2 - t1));
            caret_row
        }
    };
    use_hook({
        let mut repaint = repaint.clone();
        move || repaint()
    });

    // Bring the caret's row into view after it moved: core's `follow_caret`,
    // in pixels, because rows here are not all one line tall. Only after a
    // key — a scroll with the caret sitting still is the reader's, and leaving
    // it alone is what lets them scroll past the caret.
    let mut follow = move |caret_row: Option<usize>| {
        let Some(r) = caret_row else { return };
        let to = layout.peek().reveal(r, *top.peek(), *height.peek() - bar.peek().height());
        trace(|| format!("follow caret row {r} -> {to:?}"));
        let Some(y) = to else { return };
        top.set(y);
    };

    // The wheel and the trackpad, in CSS pixels: Blitz's own convention, a
    // line being 20 of them and a positive delta moving toward the top. Down
    // to where the last row sits mid-viewport, as a page with room under it.
    let on_wheel = move |e: WheelEvent| {
        e.prevent_default();
        let dy = match e.delta() {
            WheelDelta::Pixels(v) => v.y,
            WheelDelta::Lines(v) => v.y * 20.0,
            WheelDelta::Pages(v) => v.y * *height.peek(),
        };
        let max = (layout.peek().height() - *height.peek() / 2.0).max(0.0);
        let to = (*top.peek() - dy).clamp(0.0, max);
        if to != *top.peek() {
            top.set(to);
        }
    };

    use_context_provider({
        let doc = doc.clone();
        let repaint = repaint.clone();
        let goal = goal.clone();
        let composer = composer.clone();
        let find = find.clone();
        move || {
            PointerSink(Rc::new(move |p| {
                goal.set(None);
                composer.borrow_mut().settle(&mut doc.borrow_mut());
                // A click in the document takes typing back from the find
                // bar, and leaves the bar open, as on a Mac.
                {
                    let mut f = find.borrow_mut();
                    if f.focus != Focus::Doc {
                        f.focus = Focus::Doc;
                        // A copy of the handle: this closure is `Fn`.
                        let mut bar = bar;
                        bar.set(BarView::of(&mut f, &doc.borrow()));
                    }
                }
                let (row, line, x) = match p {
                    Pointer::Press { row, line, x, .. } | Pointer::Drag { row, line, x } => (row, line, x),
                };
                let Some(col) = lays.peek().get(row).map(|l| l.0.col_at(line, x)) else {
                    return;
                };
                pointer(&mut doc.borrow_mut(), &last_press, p, row, col);
                trace(|| format!("{p:?} -> col {col}, caret {} anchor {:?}", doc.borrow().caret, doc.borrow().anchor));
                (repaint.clone())();
            }))
        }
    });

    // Where the caret is in the layout: its row, its visual line, its x.
    let caret_at = {
        let (doc, composer) = (doc.clone(), composer.clone());
        move || -> Option<(usize, usize, f32)> {
            let doc = doc.borrow();
            let at = composer.borrow().caret().unwrap_or(doc.caret);
            let (row, col) = pos_of(&doc, at);
            let lays = lays.peek();
            let (line, x) = lays.get(row)?.0.caret(col);
            Some((row, line, x))
        }
    };

    // One visual line up or down, by geometry: the line above or below in
    // this row, or the nearest caret-holding row's last or first line, at the
    // goal x. Swift's rule, which core cannot apply to rows it never wrapped.
    let vertical = {
        let (doc, goal, caret_at) = (doc.clone(), goal.clone(), caret_at.clone());
        move |down: bool, extend: bool| {
            let Some((row, line, x)) = caret_at() else { return };
            let aim = goal.get().unwrap_or(x);
            let lays = lays.peek();
            let rows = rows.peek();
            let holds = |r: usize| rows.get(r).is_some_and(|h| !h.0.decoration);
            let target = if down {
                if line + 1 < lays[row].0.lines.len() {
                    Some((row, line + 1))
                } else {
                    (row + 1..rows.len()).find(|&r| holds(r)).map(|r| (r, 0))
                }
            } else if line > 0 {
                Some((row, line - 1))
            } else {
                (0..row).rev().find(|&r| holds(r)).map(|r| (r, lays[r].0.lines.len() - 1))
            };
            let mut doc = doc.borrow_mut();
            match target {
                Some((r, l)) => doc.click(r, lays[r].0.col_at(l, aim), extend),
                // Past the first or last line: to the document's edge, as a
                // text view does.
                None if down => doc.move_doc_end(extend),
                None => doc.move_doc_start(extend),
            }
            goal.set(Some(aim));
        }
    };

    // Carry out what the find bar asked for: select a match, replace, close.
    let run_find = {
        let (doc, find) = (doc.clone(), find.clone());
        let repaint = repaint.clone();
        move |o: Outcome| {
            {
                let mut f = find.borrow_mut();
                let mut d = doc.borrow_mut();
                let select = |d: &mut Doc, m: Option<(usize, usize)>| {
                    if let Some((s, e)) = m {
                        d.select_range(s, e);
                    }
                };
                match o {
                    Outcome::Searched => {
                        let origin = f.origin;
                        let m = f.first_from(&d, origin);
                        select(&mut d, m);
                    }
                    Outcome::Step { forward } => {
                        let m = f.step(&d, forward);
                        select(&mut d, m);
                    }
                    Outcome::Replace => {
                        find::replace_current(&mut f, &mut d);
                        let m = f.step(&d, true);
                        select(&mut d, m);
                    }
                    Outcome::ReplaceAll => {
                        let n = find::replace_all(&mut f, &mut d);
                        d.status = Some(format!("{n} replaced"));
                    }
                    Outcome::Close => f.close(),
                    Outcome::Edited | Outcome::Ignored => {}
                }
            }
            follow((repaint.clone())());
            bar.set(BarView::of(&mut find.borrow_mut(), &doc.borrow()));
        }
    };

    // Put a different document in the editor — New, Open — and start every
    // view of it afresh.
    let install: Rc<dyn Fn(Doc)> = {
        let (doc, composer, find, shown, goal) = (doc.clone(), composer.clone(), find.clone(), shown.clone(), goal.clone());
        let repaint = repaint.clone();
        Rc::new(move |new: Doc| {
            *doc.borrow_mut() = new;
            *composer.borrow_mut() = Composer::default();
            find.borrow_mut().close();
            shown.set(None);
            goal.set(None);
            let (mut top, mut words, mut bar, mut follow) = (top, words, bar, follow);
            top.set(0.0);
            words.set(None);
            follow((repaint.clone())());
            bar.set(BarView::of(&mut find.borrow_mut(), &doc.borrow()));
        })
    };
    // New, Open, Save As: each a sheet on the window, so a task (see `file.rs`).
    let files: Rc<dyn Fn(FileAction)> = {
        let (doc, window, install) = (doc.clone(), window.clone(), install.clone());
        let repaint = repaint.clone();
        Rc::new(move |action: FileAction| {
            let (doc, window, install) = (doc.clone(), window.clone(), install.clone());
            let mut repaint = repaint.clone();
            spawn(async move {
                match action {
                    FileAction::SaveAs => {
                        save_as(&doc, &window).await;
                        repaint();
                    }
                    FileAction::New | FileAction::Open => {
                        if !settle_unsaved(&doc, &window).await {
                            repaint();
                            return;
                        }
                        let new = match action {
                            FileAction::New => Doc::blank().expect("a blank document"),
                            _ => {
                                let Some(path) = file::pick_open(&window).await else { return };
                                match Doc::open(path.clone()) {
                                    Ok(d) => d,
                                    Err(e) => {
                                        doc.borrow_mut().status = Some(format!("{}: {e}", path.display()));
                                        repaint();
                                        return;
                                    }
                                }
                            }
                        };
                        install(new);
                    }
                }
            });
        })
    };

    // The keyboard, from winit — see `keys.rs` for why not `onkeydown`.
    let on_key = {
        let doc = doc.clone();
        let mut repaint = repaint.clone();
        let goal = goal.clone();
        let composer = composer.clone();
        let caret_at = caret_at.clone();
        let (find, mut run_find) = (find.clone(), run_find.clone());
        let files = files.clone();
        move |key: Key, code: Code, mods: Modifiers| {
            composer.borrow_mut().settle(&mut doc.borrow_mut());
            // Typing goes to the find bar while it has focus; a key it does
            // not take (⌘G, ⌘Z, ⌘S…) falls through to the document's.
            if find.borrow().open && find.borrow().focus != Focus::Doc {
                let o = find.borrow_mut().key(&key, mods);
                if o != Outcome::Ignored {
                    trace(|| format!("find key {key:?} -> {o:?}"));
                    run_find(o);
                    return;
                }
            }
            let action = apply_key(&mut doc.borrow_mut(), &key, code, mods);
            trace(|| format!("key {key:?} {code:?} {mods:?} -> {action:?}"));
            if !matches!(action, Action::Vertical { .. } | Action::Page { .. } | Action::Ignored) {
                goal.set(None);
            }
            let changed = match action {
                Action::Ignored => return,
                Action::Handled => true,
                // ⌘S on a document with no file yet is a Save As, and only
                // the editor has a dialog to ask with.
                Action::Host(Host::Save) if doc.borrow().is_untitled() => {
                    files(FileAction::SaveAs);
                    return;
                }
                Action::Host(h) => host::run(&mut doc.borrow_mut(), h),
                Action::Vertical { down, extend } => {
                    vertical(down, extend);
                    true
                }
                Action::Page { down, extend } => {
                    // A page is the viewport less a line of overlap, in body
                    // lines — Swift's and gpui's rule.
                    let lines = ((*height.peek() / LINE) as usize).saturating_sub(1).max(1);
                    for _ in 0..lines {
                        vertical(down, extend);
                    }
                    true
                }
                Action::LineEdge { end, extend } => {
                    if let Some((row, line, _)) = caret_at() {
                        let col = lays.peek()[row].0.edge(line, end);
                        doc.borrow_mut().click(row, col, extend);
                    }
                    true
                }
                Action::Find(cmd) => {
                    let empty = find.borrow().query.text.is_empty();
                    let o = match cmd {
                        FindCommand::Open { replace } => {
                            find.borrow_mut().show(&doc.borrow(), replace);
                            Outcome::Searched
                        }
                        // ⌘G with nothing to look for opens the bar instead.
                        _ if empty => {
                            find.borrow_mut().show(&doc.borrow(), false);
                            Outcome::Edited
                        }
                        FindCommand::Next => Outcome::Step { forward: true },
                        FindCommand::Previous => Outcome::Step { forward: false },
                    };
                    run_find(o);
                    return;
                }
            };
            if changed {
                follow(repaint());
            }
        }
    };
    // Tell the input method where the caret is, so its candidate window and
    // the accent menu open beside it, and what text surrounds it, so
    // press-and-hold can see the letter it offers accents for.
    let ime_area = {
        let (window, caret_at) = (window.clone(), caret_at.clone());
        let (doc, composer) = (doc.clone(), composer.clone());
        move || {
            let Some((row, line, x)) = caret_at() else { return };
            let lays = lays.peek();
            let Some(l) = lays.get(row).and_then(|l| l.0.lines.get(line)) else { return };
            let y = layout.peek().top(row) - *top.peek() + l.top as f64 + bar.peek().height();
            let mut data = ImeRequestData::default().with_cursor_area(
                LogicalPosition::new(row::PAD + x as f64, y).into(),
                LogicalSize::new(2.0, l.height as f64).into(),
            );
            let around = ime::surrounding(&doc.borrow(), &composer.borrow());
            if let Some(text) = around.and_then(|(t, c, a)| ImeSurroundingText::new(t, c, a).ok()) {
                data = data.with_surrounding_text(text);
            }
            let _ = window.request_ime_update(ImeRequest::Update(data));
        }
    };
    // The input method, allowed on the window once; see `ime.rs` for why
    // the editor and not Blitz asks.
    use_hook({
        let window = window.clone();
        move || {
            // Surrounding text needs the patched winit-appkit to be heard
            // on macOS (see `patches/winit-appkit/PATCH.md`); winit asks
            // for some with the capability, so an empty line to start.
            let request = ImeEnableRequest::new(
                ImeCapabilities::new().with_cursor_area().with_surrounding_text(),
                ImeRequestData::default()
                    .with_cursor_area(LogicalPosition::new(0.0, 0.0).into(), LogicalSize::new(2.0, LINE).into())
                    .with_surrounding_text(ImeSurroundingText::new(String::new(), 0, 0).expect("an empty excerpt")),
            );
            if let Some(request) = request {
                let _ = window.request_ime_update(ImeRequest::Enable(request));
            }
        }
    });
    let mut on_ime = {
        let (doc, composer) = (doc.clone(), composer.clone());
        let (find, mut run_find) = (find.clone(), run_find.clone());
        let mut repaint = repaint.clone();
        let ime_area = ime_area.clone();
        move |ime: &Ime| {
            trace(|| format!("ime {ime:?}"));
            // An accent or an emoji committed while the find bar has focus
            // is the query's. A composition's preedit is not drawn there.
            if find.borrow().open && find.borrow().focus != Focus::Doc {
                if let Ime::Commit(text) = ime {
                    let o = find.borrow_mut().commit(text);
                    run_find(o);
                }
                return;
            }
            let mut c = composer.borrow_mut();
            let mut d = doc.borrow_mut();
            match ime {
                Ime::Preedit(text, cursor) => c.preedit(&mut d, text, *cursor),
                Ime::Commit(text) => c.commit(&mut d, text),
                Ime::DeleteSurrounding { before_bytes, after_bytes } => {
                    c.delete_surrounding(&mut d, *before_bytes, *after_bytes)
                }
                Ime::Disabled => c.settle(&mut d),
                _ => return,
            }
            drop((c, d));
            follow(repaint());
            ime_area();
        }
    };
    // Shared: the window's keys and the menu bar's items both press it.
    let on_key: Rc<RefCell<dyn FnMut(Key, Code, Modifiers)>> = Rc::new(RefCell::new(on_key));

    // The menu bar, installed once; a task takes what its items ask for. muda
    // queues each choice, and a shortcut that names an item is the item's —
    // macOS gives it to the menu before the view — so this is also where most
    // ⌘ chords now arrive.
    use_hook({
        let (on_key, files, ime_area) = (on_key.clone(), files.clone(), ime_area.clone());
        move || {
            let menu = menu::install();
            spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    while let Some(command) = menu.next() {
                        trace(|| format!("menu {command:?}"));
                        match command {
                            MenuCommand::Chord(key, code, mods) => {
                                (on_key.borrow_mut())(key, code, mods);
                                ime_area();
                            }
                            MenuCommand::New => files(FileAction::New),
                            MenuCommand::Open => files(FileAction::Open),
                            MenuCommand::SaveAs => files(FileAction::SaveAs),
                        }
                    }
                }
            });
        }
    });

    let mut mods = winit::keyboard::ModifiersState::empty();
    dioxus_native::use_window_event({
        let mut repaint = repaint.clone();
        let ime_area = ime_area.clone();
        move |event, _| match event {
            WindowEvent::Ime(ime) => on_ime(ime),
            WindowEvent::SurfaceResized(size) => {
                let scale = window.scale_factor();
                height.set(viewport_height(size.height, scale));
                let w = window_width(size.width, scale);
                if w != *win_w.peek() {
                    win_w.set(w);
                    repaint();
                }
            }
            WindowEvent::ModifiersChanged(m) => mods = m.state(),
            // A click moved the caret: on the button's release, which winit
            // reports after Blitz has handled the press, tell the input
            // method where it went and what is around it now.
            WindowEvent::PointerButton { state, .. } if !state.is_pressed() => ime_area(),
            // Blitz paints nothing while the window is covered, so a capture
            // of a covered window is its last frame — which looks exactly
            // like a hung editor. Said here so the trace tells them apart.
            WindowEvent::Occluded(o) => trace(|| format!("occluded {o}")),
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some((key, code, m)) = keys::pressed(event, mods) {
                    (on_key.borrow_mut())(key, code, m);
                    ime_area();
                }
            }
            _ => {}
        }
    });

    // A click in the blank space under the last row: core opens a paragraph
    // there if the document ends in a block Return cannot leave.
    let on_past_end = {
        let doc = doc.clone();
        let mut repaint = repaint.clone();
        move |_: MouseEvent| {
            doc.borrow_mut().click_past_end();
            follow(repaint());
        }
    };

    let bar_view = bar.read().clone();
    let bar_h = bar_view.height();
    let layout = layout.read();
    let (first, last) = layout.window(*top.read(), *height.read() - bar_h);
    trace(|| format!("render rows {first}..{last} of {}, top {:.0}, height {:.0}", layout.rows(), *top.peek(), *height.peek()));
    // The first drawn row is `top - above` above the viewport's top edge.
    let shift = layout.top(first) - *top.read() + bar_h;
    let rows = rows.read();
    let lays = lays.read();
    let row_kinds = row_kinds.read();
    let column = *width.read() + 2.0 * row::PAD as f32;
    // The find bar's buttons and fields.
    let on_bar = {
        let (find, mut run_find) = (find.clone(), run_find.clone());
        move |what: BarClick| {
            let o = {
                let mut f = find.borrow_mut();
                match what {
                    BarClick::Query => {
                        f.focus = Focus::Query;
                        Outcome::Edited
                    }
                    BarClick::Replacement => {
                        f.focus = Focus::Replacement;
                        Outcome::Edited
                    }
                    BarClick::Previous => Outcome::Step { forward: false },
                    BarClick::Next => Outcome::Step { forward: true },
                    BarClick::Replace => Outcome::Replace,
                    BarClick::All => Outcome::ReplaceAll,
                    BarClick::Done => Outcome::Close,
                }
            };
            run_find(o);
        }
    };
    let click = |what: BarClick| {
        let mut on_bar = on_bar.clone();
        move |e: MouseEvent| {
            e.prevent_default();
            on_bar(what);
        }
    };
    let display = if bar_view.open { "block" } else { "none" };
    let mut css = layout_css();
    if std::env::var_os("LEAF_OUTLINE").is_some() {
        css.push_str(".leaf .line { outline: 1px solid #f00; } .leaf .line > span { outline: 1px solid #0a0; }");
    }

    rsx! {
        style { {STYLESHEET} }
        style { {css} }
        // The mouse's default actions are cancelled: they are Blitz's own
        // text selection, drawn over leaf's, and the focus changes that made
        // `onkeydown` unreliable (see `keys.rs`). Not `onpointerdown`:
        // cancelling the pointer event stops Blitz dispatching `mousedown`.
        div { class: "leaf", style: "width: {column}px",
            onwheel: on_wheel,
            onmousedown: move |e: MouseEvent| e.prevent_default(),
            onclick: move |e: MouseEvent| e.prevent_default(),
            div { style: "height: 0; margin-top: {shift}px" }
            for i in first..last {
                Row { key: "{i}", index: i, row: rows[i].clone(), layout: lays[i].clone(), kind: row_kinds[i] }
            }
            // Room under the last row, as a page has, so there is somewhere
            // to click past the end.
            div { style: "height: {*height.read() / 2.0}px",
                onmousedown: on_past_end,
            }
        }
        div { class: "find", style: "display: {display}; height: {bar_h}px",
            onmousedown: move |e: MouseEvent| e.prevent_default(),
            div { class: "find-row",
                span { class: "label", "Find" }
                {field(&bar_view.query, bar_view.focus == Focus::Query, click(BarClick::Query))}
                span { class: "count", "{bar_view.count}" }
                span { class: "btn", onmousedown: click(BarClick::Previous), "‹" }
                span { class: "btn", onmousedown: click(BarClick::Next), "›" }
                span { class: "btn", onmousedown: click(BarClick::Done), "Done" }
            }
            if bar_view.replacing {
                div { class: "find-row",
                    span { class: "label", "Replace" }
                    {field(&bar_view.replacement, bar_view.focus == Focus::Replacement, click(BarClick::Replacement))}
                    span { class: "btn", onmousedown: click(BarClick::Replace), "Replace" }
                    span { class: "btn", onmousedown: click(BarClick::All), "All" }
                }
            }
        }
        div { class: "status",
            "{status}"
            span { class: "grow" }
            match *words.read() {
                Some((_, n)) => rsx! { "{n} words" },
                None => rsx! { "counting…" },
            }
        }
    }
}

/// Apply a pointer gesture at a resolved (row, column): a press places the
/// caret, or on a task box ticks it; a second and third press in quick
/// succession select the word and the block; a drag extends the selection
/// from wherever the press left it.
fn pointer(doc: &mut Doc, last: &Cell<Option<Press>>, p: Pointer, row: usize, col: usize) {
    match p {
        Pointer::Press { extend, .. } => {
            let now = Instant::now();
            let count = match last.get() {
                Some(l) if !extend && l.row == row && now - l.at < MULTI_CLICK => l.count % 3 + 1,
                _ => 1,
            };
            last.set(Some(Press { at: now, row, col, count }));
            if count == 1
                && !extend
                && doc.view == View::Wysiwyg
                && let Some(offset) = doc.vmap.task_box_at(row, col)
            {
                // A tick, not a caret placement: the caret stays where it was,
                // as it does in the Swift editor.
                doc.toggle_task_at(offset);
                return;
            }
            match count {
                2 => {
                    let at = offset_at(doc, row, col);
                    doc.select_word_at(at);
                }
                3 => {
                    let at = offset_at(doc, row, col);
                    doc.select_block_at(at);
                }
                _ => doc.click(row, col, extend),
            }
        }
        Pointer::Drag { .. } => {
            // A move that has not left the pressed column is the hand
            // settling, not a drag — and would shrink a double-click's word
            // back to the pointer. A drag after a double- or triple-click
            // should extend by words or blocks, as Swift's does; until it
            // does, it is left alone rather than undoing the selection.
            match last.get() {
                Some(p) if (p.row, p.col) == (row, col) || p.count > 1 => {}
                _ => doc.click(row, col, true),
            }
        }
    }
}

/// What a file command does.
#[derive(Clone, Copy, Debug)]
enum FileAction {
    New,
    Open,
    SaveAs,
}

/// Name a file for the document and save it there; answers whether it saved.
async fn save_as(doc: &Rc<RefCell<Doc>>, window: &Arc<dyn winit::window::Window>) -> bool {
    let name = match doc.borrow().is_untitled() {
        true => "Untitled.md".to_owned(),
        false => doc.borrow().file_name(),
    };
    let Some(path) = file::pick_save(window, &name).await else { return false };
    let mut d = doc.borrow_mut();
    d.save_as(path);
    !d.dirty
}

/// Before the document is replaced: save it, drop its changes, or stay —
/// answers whether to go ahead.
async fn settle_unsaved(doc: &Rc<RefCell<Doc>>, window: &Arc<dyn winit::window::Window>) -> bool {
    if !doc.borrow().dirty {
        return true;
    }
    let name = doc.borrow().file_name();
    match file::ask_unsaved(window, &name).await {
        Unsaved::Discard => true,
        Unsaved::Cancel => false,
        Unsaved::Save if doc.borrow().is_untitled() => save_as(doc, window).await,
        Unsaved::Save => {
            let mut d = doc.borrow_mut();
            d.save();
            !d.dirty
        }
    }
}

/// Where on the find bar a click landed.
#[derive(Clone, Copy, Debug)]
enum BarClick {
    Query,
    Replacement,
    Previous,
    Next,
    Replace,
    All,
    Done,
}

/// A find-bar field: its text, with the caret drawn in it when it has focus.
fn field(f: &find::Field, focused: bool, on_press: impl FnMut(MouseEvent) + 'static) -> Element {
    let (before, after) = f.text.split_at(f.caret.min(f.text.len()));
    let class = if focused { "field focused" } else { "field" };
    rsx! {
        span { class, onmousedown: on_press,
            "{before}"
            if focused {
                span { class: "caret-in" }
            }
            "{after}"
        }
    }
}

/// A source offset's (row, column) in whichever view is showing.
fn pos_of(doc: &Doc, offset: usize) -> (usize, usize) {
    match doc.view {
        View::Source => source::pos_of(doc, offset),
        View::Wysiwyg => doc.vmap.pos_of_offset(offset),
    }
}

/// The source offset at (row, column) of whichever view is showing.
fn offset_at(doc: &Doc, row: usize, col: usize) -> usize {
    match doc.view {
        View::Source => source::offset_at(doc, row, col),
        View::Wysiwyg => doc.vmap.offset_of_pos(row, col),
    }
}

/// With `LEAF_TRACE` set, say on stderr what each key and pointer event did.
/// The spike's own instrument for the input path, which is where Blitz and a
/// browser differ most.
fn trace(line: impl FnOnce() -> String) {
    thread_local! {
        static ON: bool = std::env::var_os("LEAF_TRACE").is_some();
    }
    if ON.with(|on| *on) {
        eprintln!("{}", line());
    }
}

/// How long the typing has to stop before the word count is redone.
const SETTLE: Duration = Duration::from_millis(400);

/// Count the words once the document has sat at `rev` for [`SETTLE`].
///
/// `counts()` reparses and lays out the whole document — 130 ms at 5 200 rows
/// — so it cannot run per keystroke, and core's own doc says to ask "when the
/// typing stops". Every edit schedules one of these; all but the last wake to
/// find the revision has moved on, and do nothing.
fn recount(doc: Rc<RefCell<Doc>>, rev: u64, mut words: Signal<Option<(u64, usize)>>) {
    spawn(async move {
        tokio::time::sleep(SETTLE).await;
        let doc = doc.borrow();
        if doc.revision() == rev {
            words.set(Some((rev, doc.counts().words)));
        }
    });
}

/// The status line: the file, whether it is saved, the view, and whatever
/// core last had to say — a failed save, an untitled ⌘S. The word count is
/// its own signal, drawn beside this; see [`recount`].
fn status_line(doc: &Doc) -> String {
    let name = doc.file_name();
    let dirty = if doc.dirty { " •" } else { "" };
    let note = doc.status.as_deref().unwrap_or("");
    format!("{name}{dirty}   {} view   {note}", doc.view_name())
}
