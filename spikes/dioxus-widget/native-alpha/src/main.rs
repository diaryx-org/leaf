//! The same leaf rows through Dioxus's **experimental native backend**
//! (`dioxus-native` 0.8.0-alpha.2, Blitz): an HTML/CSS renderer on wgpu, with
//! no webview and no browser engine.
//!
//! ```sh
//! cargo run --release -- ../corpus/document.md
//! ```
//!
//! An editor, not just a first paint: it takes keys through the parent's key
//! mapping (copied as [`input`]), repaints through the parent's [`Frame`] so a
//! keystroke renders one row and memoizes the rest, places the caret on a
//! click, and renders only the rows in view ([`viewport`]).
//!
//! The markup is duplicated from the parent's `editor.rs` rather than shared,
//! because `rsx!` builds types from *its* Dioxus and the two versions cannot
//! meet. Everything underneath it — the glyph→run coalescing, the class
//! mapping, the frame delta — is the parent crate's, which is the point: the
//! bridge is independent of which Dioxus renders it.

mod input;
mod row;
mod viewport;

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::html::geometry::PixelsVector2D;
use dioxus::prelude::*;
use dioxus_widget_spike::frame::Frame;
use dioxus_widget_spike::rows::{Marks, RowHandle};
use dioxus_widget_spike::style::STYLESHEET;
use leaf_core::{Doc, Format};

use crate::input::{Action, apply_key};
use crate::row::{ClickSink, Row};
use crate::viewport::Layout;

const WRAP: usize = 80;

/// What this binary adds to the parent's stylesheet: `.leaf` as the window's
/// scroll container (so its `onscroll` fires — Blitz sends the root's scroll
/// to the root element, which no component owns), a known left edge for
/// [`row::PAD`], and the inline caret `row.rs` draws.
const LAYOUT: &str = "body { margin: 0; }
.leaf { padding: 0 16px; height: 100vh; overflow-y: scroll; box-sizing: border-box; }
.leaf .caret-in { display: inline-block; width: 0; height: 1.2em;
                  border-left: 2px solid #1a73e8; margin-right: -2px;
                  vertical-align: text-bottom; }";

fn main() {
    let source = std::env::args()
        .nth(1)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_else(|| include_str!("../../corpus/document.md").to_string());

    dioxus_native::launch_cfg(
        App,
        vec![Box::new(move || Box::new(Source(source.clone())))],
        vec![],
    );
}

#[derive(Clone)]
struct Source(String);

#[allow(non_snake_case)]
fn App() -> Element {
    let source = use_context::<Source>().0;
    let doc = use_hook(|| {
        let mut d = Doc::from_source(source, Format::Markdown)
            .unwrap_or_else(|_| Doc::blank().expect("a blank document"));
        d.build_visual(WRAP);
        d.place_caret(0, false);
        Rc::new(RefCell::new(d))
    });
    let frame = use_hook(|| Rc::new(RefCell::new(Frame::default())));
    let mut rows = use_signal(Vec::<RowHandle>::new);
    let mut layout = use_signal(Layout::default);
    // The viewport: where `.leaf` is scrolled to and how tall it is, both
    // from its `onscroll`. The height starts as a guess and is corrected by
    // the first scroll event or the mount below.
    let mut top = use_signal(|| 0.0_f64);
    let mut height = use_signal(|| 900.0_f64);
    let mut scroller = use_signal(|| None::<Rc<MountedData>>);

    // The first paint, and every repaint after a key or a click. Answers the
    // caret's row, for the viewport to follow.
    let repaint = {
        let (doc, frame) = (doc.clone(), frame.clone());
        move || -> Option<usize> {
            let mut doc = doc.borrow_mut();
            doc.build_visual(WRAP);
            let marks = Marks::of(&doc.vmap, doc.caret, doc.anchor);
            let mut frame = frame.borrow_mut();
            let handles = frame.update(&doc.vmap, &marks).to_vec();
            layout.set(Layout::of(handles.iter().map(|h| &*h.0)));
            rows.set(handles);
            marks.caret.map(|(r, _)| r)
        }
    };
    use_hook({
        let mut repaint = repaint.clone();
        move || repaint()
    });

    // Bring the caret's row into view after it moved: README's
    // `follow_caret`, in pixels, because rows here are not all one line tall.
    // Only after a key — a scroll with the caret sitting still is the
    // reader's, and leaving it alone is what lets them scroll past the caret.
    let mut follow = move |caret_row: Option<usize>| {
        let Some(r) = caret_row else { return };
        let Some(y) = layout.peek().reveal(r, *top.peek(), *height.peek()) else {
            return;
        };
        top.set(y);
        if let Some(el) = scroller.peek().clone() {
            spawn(async move {
                let _ = el.scroll(PixelsVector2D::new(0.0, y), ScrollBehavior::Instant).await;
            });
        }
    };

    use_context_provider({
        let doc = doc.clone();
        let repaint = repaint.clone();
        move || {
            ClickSink(Rc::new(move |row, col, extend| {
                doc.borrow_mut().click(row, col, extend);
                (repaint.clone())();
            }))
        }
    });

    let on_key = {
        let doc = doc.clone();
        let mut repaint = repaint.clone();
        move |e: KeyboardEvent| {
            let action = apply_key(&mut doc.borrow_mut(), &e.key(), e.modifiers());
            if action != Action::Ignored {
                e.prevent_default();
            }
            if action == Action::Handled {
                follow(repaint());
            }
        }
    };

    let layout = layout.read();
    let (first, last) = layout.window(*top.read(), *height.read());
    let above = layout.top(first);
    let below = layout.height() - layout.top(last);
    let rows = rows.read();

    rsx! {
        style { {STYLESHEET} }
        style { {LAYOUT} }
        // Blitz focuses only form controls on a click, and *clears* focus on
        // a click anywhere else, so keys would go to the root and never
        // reach this div. So the editor takes focus itself once mounted, and
        // cancels the mouse's default actions — which are also Blitz's own
        // text selection, drawn over leaf's. Not `onpointerdown`: cancelling
        // the pointer event stops Blitz dispatching `mousedown` at all.
        div { class: "leaf", tabindex: "0", onkeydown: on_key,
            onmounted: move |e: MountedEvent| async move {
                let el = e.data();
                let _ = el.set_focus(true).await;
                if let Ok(rect) = el.get_client_rect().await {
                    height.set(rect.height());
                }
                scroller.set(Some(el));
            },
            onscroll: move |e: ScrollEvent| {
                top.set(e.scroll_top());
                height.set(e.client_height() as f64);
            },
            onmousedown: move |e: MouseEvent| e.prevent_default(),
            onclick: move |e: MouseEvent| e.prevent_default(),
            div { style: "height: {above}px" }
            for i in first..last {
                Row { key: "{i}", index: i, row: rows[i].clone() }
            }
            div { style: "height: {below}px" }
        }
    }
}
