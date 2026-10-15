//! The same leaf rows through Dioxus's **experimental native backend**
//! (`dioxus-native` 0.8.0-alpha.2, Blitz): an HTML/CSS renderer on wgpu, with
//! no webview and no browser engine.
//!
//! ```sh
//! cargo run --release -- ../corpus/document.md
//! ```
//!
//! An editor, not just a first paint, working toward the Swift editor
//! (`apps/leaf-editor`): its key bindings ([`input`]), the clipboard and the
//! file ([`host`]), click, double- and triple-click, drag-select and task
//! boxes, a status line — over the parent's [`Frame`], so a keystroke renders
//! one row and memoizes the rest, and only the rows in view ([`viewport`]).
//!
//! The markup is duplicated from the parent's `editor.rs` rather than shared,
//! because `rsx!` builds types from *its* Dioxus and the two versions cannot
//! meet. Everything underneath it — the glyph→run coalescing, the class
//! mapping, the frame delta — is the parent crate's, which is the point: the
//! bridge is independent of which Dioxus renders it.

mod host;
mod input;
mod keys;
mod row;
mod viewport;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use dioxus::html::geometry::WheelDelta;
use dioxus::prelude::*;
use dioxus::html::{Code, Key, Modifiers};
use dioxus_native::winit;
use dioxus_native::winit::event::WindowEvent;
use dioxus_widget_spike::frame::Frame;
use dioxus_widget_spike::rows::{Marks, RowHandle};
use dioxus_widget_spike::style::STYLESHEET;
use leaf_core::{Doc, Format};

use crate::input::{Action, apply_key};
use crate::row::{Pointer, PointerSink, Row};
use crate::viewport::{LINE, Layout};

const WRAP: usize = 80;

/// The status line's height, in CSS pixels — `.status` in [`LAYOUT`].
const STATUS: f64 = 24.0;

/// Presses closer together than this, on the same row, count as one double-
/// or triple-click — the system default on macOS.
const MULTI_CLICK: Duration = Duration::from_millis(500);

/// What this binary adds to the parent's stylesheet: `.leaf` as a fixed
/// viewport the editor scrolls itself (see `App`), a known left edge for
/// [`row::PAD`], the inline caret `row.rs` draws, and the status line.
// Two Blitz behaviours shape this, both found the hard way:
//
// - It paints the first frame of an `overflow: hidden` element and not the
//   changes after it, so the clip is on `body`, never on `.leaf`.
// - It skips painting a child that hangs past its parent's box, so `.leaf`
//   has no fixed height: its box is as tall as the rows in it, shifted up by
//   the scroll, and the window cuts it off. The status line is pinned to the
//   window's bottom over it, in a stacking layer of its own.
const LAYOUT: &str = "html, body { margin: 0; overflow: hidden; height: 100vh; }
body { position: relative; }
.leaf { padding: 0 16px; box-sizing: border-box; }
.leaf .caret-in { display: inline-block; width: 0; height: 1.2em;
                  border-left: 2px solid #1a73e8; margin-right: -2px;
                  vertical-align: text-bottom; }
.status { position: absolute; left: 0; right: 0; bottom: 0; z-index: 1;
          height: 24px; line-height: 24px; padding: 0 16px; box-sizing: border-box;
          font: 12px ui-monospace, monospace; color: #5f6368; background: #f6f8fa;
          border-top: 1px solid #e1e4e8; display: flex; gap: 16px; white-space: pre; }
.status .grow { flex: 1; }";

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

/// The last press, for counting a double- and triple-click.
#[derive(Clone, Copy)]
struct Press {
    at: Instant,
    row: usize,
    col: usize,
    count: u8,
}

#[allow(non_snake_case)]
fn App() -> Element {
    let path = use_context::<Opened>().0;
    let doc = use_hook(|| {
        let mut d = open(path);
        d.build_visual(WRAP);
        Rc::new(RefCell::new(d))
    });
    let frame = use_hook(|| Rc::new(RefCell::new(Frame::default())));
    let last_press = use_hook(|| Rc::new(Cell::new(None::<Press>)));
    // The word count, as of a revision; see `recount`.
    let words = use_signal(|| None::<(u64, usize)>);
    let mut rows = use_signal(Vec::<RowHandle>::new);
    let mut layout = use_signal(Layout::default);
    let mut status = use_signal(String::new);
    // The viewport: how far down the document it starts, and how tall it is.
    //
    // The editor scrolls itself rather than letting `.leaf` overflow. Blitz
    // can scroll an element, but only from a task — `MountedData::scroll` —
    // and dioxus-native polls tasks while Blitz holds its document borrowed,
    // so following the caret that way panicked ("RefCell already borrowed").
    // Owning the offset makes following the caret a signal write, and makes
    // the virtualized window exact: the rows drawn are the rows in view.
    //
    // The height is the window's, from winit: dioxus-native implements
    // neither `onresize` nor a measurement outside a task.
    let window = dioxus_native::use_window();
    let mut top = use_signal(|| 0.0_f64);
    let mut height = use_signal(|| viewport_height(window.surface_size().height, window.scale_factor()));
    // The first paint, and every repaint after a key or a click. Answers the
    // caret's row, for the viewport to follow.
    let repaint = {
        let doc_rc = doc.clone();
        let (doc, frame) = (doc.clone(), frame.clone());
        move || -> Option<usize> {
            let t0 = Instant::now();
            let mut doc = doc.borrow_mut();
            doc.build_visual(WRAP);
            let t1 = Instant::now();
            let marks = Marks::of(&doc.vmap, doc.caret, doc.anchor);
            let mut frame = frame.borrow_mut();
            let handles = frame.update(&doc.vmap, &marks).to_vec();
            let t2 = Instant::now();
            layout.set(Layout::of(handles.iter().map(|h| &*h.0)));
            rows.set(handles);
            let t3 = Instant::now();
            status.set(status_line(&doc));
            if words.peek().is_none_or(|(r, _)| r != doc.revision()) {
                recount(doc_rc.clone(), doc.revision(), words);
            }
            let t4 = Instant::now();
            trace(|| format!("repaint: build {:.1?} frame {:.1?} layout {:.1?} status {:.1?}; {} rows rebuilt",
                t1 - t0, t2 - t1, t3 - t2, t4 - t3, frame.last.rebuilt));
            marks.caret.map(|(r, _)| r)
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
        let to = layout.peek().reveal(r, *top.peek(), *height.peek());
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
        move || {
            PointerSink(Rc::new(move |p| {
                pointer(&mut doc.borrow_mut(), &last_press, p);
                trace(|| format!("{p:?} -> caret {} anchor {:?}", doc.borrow().caret, doc.borrow().anchor));
                (repaint.clone())();
            }))
        }
    });

    // The keyboard, from winit — see `keys.rs` for why not `onkeydown`.
    let mut on_key = {
        let doc = doc.clone();
        let mut repaint = repaint.clone();
        move |key: Key, code: Code, mods: Modifiers| {
            let action = apply_key(&mut doc.borrow_mut(), &key, code, mods);
            trace(|| format!("key {key:?} {code:?} {mods:?} -> {action:?}"));
            let changed = match action {
                Action::Ignored => return,
                Action::Handled => true,
                Action::Host(h) => host::run(&mut doc.borrow_mut(), h),
                Action::Page { down, extend } => {
                    // A page is the viewport less a line of overlap, in rows
                    // of body height — Swift's and gpui's rule.
                    let rows = ((*height.peek() / LINE) as usize).saturating_sub(1).max(1);
                    let mut doc = doc.borrow_mut();
                    for _ in 0..rows {
                        if down {
                            doc.move_down(extend);
                        } else {
                            doc.move_up(extend);
                        }
                    }
                    true
                }
            };
            if changed {
                follow(repaint());
            }
        }
    };
    let mut mods = winit::keyboard::ModifiersState::empty();
    dioxus_native::use_window_event(move |event, _| match event {
        WindowEvent::SurfaceResized(size) => {
            height.set(viewport_height(size.height, window.scale_factor()));
        }
        WindowEvent::ModifiersChanged(m) => mods = m.state(),
        WindowEvent::KeyboardInput { event, .. } => {
            if let Some((key, code, m)) = keys::pressed(event, mods) {
                on_key(key, code, m);
            }
        }
        _ => {}
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

    let layout = layout.read();
    let (first, last) = layout.window(*top.read(), *height.read());
    trace(|| format!("render rows {first}..{last} of {}, top {:.0}, height {:.0}", layout.rows(), *top.peek(), *height.peek()));
    // The first drawn row is `top - above` above the viewport's top edge.
    let shift = layout.top(first) - *top.read();
    let rows = rows.read();

    rsx! {
        style { {STYLESHEET} }
        style { {LAYOUT} }
        // The mouse's default actions are cancelled: they are Blitz's own
        // text selection, drawn over leaf's, and the focus changes that made
        // `onkeydown` unreliable (see `keys.rs`). Not `onpointerdown`:
        // cancelling the pointer event stops Blitz dispatching `mousedown`.
        div { class: "leaf",
            onwheel: on_wheel,
            onmousedown: move |e: MouseEvent| e.prevent_default(),
            onclick: move |e: MouseEvent| e.prevent_default(),
            div { style: "height: 0; margin-top: {shift}px" }
            for i in first..last {
                Row { key: "{i}", index: i, row: rows[i].clone() }
            }
            // Room under the last row, as a page has, so there is somewhere
            // to click past the end.
            div { style: "height: {*height.read() / 2.0}px",
                onmousedown: on_past_end,
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

/// Apply a pointer gesture: a press places the caret, or on a task box ticks
/// it; a second and third press in quick succession select the word and the
/// block; a drag extends the selection from wherever the press left it.
fn pointer(doc: &mut Doc, last: &Cell<Option<Press>>, p: Pointer) {
    match p {
        Pointer::Press { row, col, extend } => {
            let now = Instant::now();
            let count = match last.get() {
                Some(l) if !extend && l.row == row && now - l.at < MULTI_CLICK => l.count % 3 + 1,
                _ => 1,
            };
            last.set(Some(Press { at: now, row, col, count }));
            if count == 1
                && !extend
                && let Some(offset) = doc.vmap.task_box_at(row, col)
            {
                // A tick, not a caret placement: the caret stays where it was,
                // as it does in the Swift editor.
                doc.toggle_task_at(offset);
                return;
            }
            match count {
                2 => {
                    let at = doc.vmap.offset_of_pos(row, col);
                    doc.select_word_at(at);
                }
                3 => {
                    let at = doc.vmap.offset_of_pos(row, col);
                    doc.select_block_at(at);
                }
                _ => doc.click(row, col, extend),
            }
        }
        Pointer::Drag { row, col } => {
            // A move that has not left the pressed cell is the hand settling,
            // not a drag — and would shrink a double-click's word back to the
            // pointer. A drag after a double- or triple-click should extend
            // by words or blocks, as Swift's does; until it does, it is left
            // alone rather than undoing the selection.
            match last.get() {
                Some(p) if (p.row, p.col) == (row, col) || p.count > 1 => {}
                _ => doc.click(row, col, true),
            }
        }
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
