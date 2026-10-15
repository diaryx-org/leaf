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
//! keystroke renders one row and memoizes the rest, and places the caret on a
//! click.
//!
//! The markup is duplicated from the parent's `editor.rs` rather than shared,
//! because `rsx!` builds types from *its* Dioxus and the two versions cannot
//! meet. Everything underneath it — the glyph→run coalescing, the class
//! mapping, the frame delta — is the parent crate's, which is the point: the
//! bridge is independent of which Dioxus renders it.

mod input;

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::html::Modifiers;
use dioxus::prelude::*;
use dioxus_widget_spike::frame::Frame;
use dioxus_widget_spike::rows::{Marks, RowHandle, RowView};
use dioxus_widget_spike::style::STYLESHEET;
use leaf_core::{Doc, Format};

use crate::input::{Action, apply_key};

const WRAP: usize = 80;

/// The document's left edge in the window, in CSS pixels: `body` has no
/// margin and `.leaf` this much padding (see [`LAYOUT`]), so a click's
/// client x less this is its x within a row.
const PAD: f64 = 16.0;

/// One monospace cell at the stylesheet's 15px. SF Mono and Menlo both
/// advance about 0.6em; a click divides by this to find its column. An
/// approximation — Blitz could measure it, but the spike does not ask.
const CELL: f64 = 15.0 * 0.6;

/// What this binary adds to the parent's stylesheet: a known left edge for
/// [`PAD`], and the inline caret [`segments`] draws.
const LAYOUT: &str = "body { margin: 0; } .leaf { padding: 16px; }
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

/// Where a row sends the click it received, as (row, column, extend). A
/// context rather than a prop so [`Row`]'s props stay its index and handle,
/// both of which compare cheaply — the memoization the parent measured.
#[derive(Clone)]
struct ClickSink(Rc<dyn Fn(usize, usize, bool)>);

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

    // The first paint, and every repaint after a key or a click.
    let repaint = {
        let (doc, frame) = (doc.clone(), frame.clone());
        move || {
            let mut doc = doc.borrow_mut();
            doc.build_visual(WRAP);
            let marks = Marks::of(&doc.vmap, doc.caret, doc.anchor);
            let mut frame = frame.borrow_mut();
            rows.set(frame.update(&doc.vmap, &marks).to_vec());
        }
    };
    use_hook({
        let mut repaint = repaint.clone();
        move || repaint()
    });

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
                repaint();
            }
        }
    };

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
                let _ = e.set_focus(true).await;
            },
            onmousedown: move |e: MouseEvent| e.prevent_default(),
            onclick: move |e: MouseEvent| e.prevent_default(),
            for (i, row) in rows.read().iter().enumerate() {
                Row { key: "{i}", index: i, row: row.clone() }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct RowProps {
    index: usize,
    row: RowHandle,
}

/// One row, memoized by its handle: a row the frame did not rebuild is the
/// same `Rc` as last time, compares equal, and is not rendered again.
#[allow(non_snake_case)]
fn Row(props: RowProps) -> Element {
    let sink = use_context::<ClickSink>();
    let v: &RowView = &props.row.0;
    let index = props.index;
    let class = match (v.code, v.decoration) {
        (true, _) => "row code",
        (_, true) => "row decoration",
        _ => "row",
    };
    // A heading's glyphs are wider than a body cell by its font scale.
    let scale = v
        .runs
        .iter()
        .find_map(|r| {
            r.class.split(' ').find_map(|c| match c {
                "h1" => Some(2.0),
                "h2" => Some(1.6),
                "h3" => Some(1.3),
                _ => None,
            })
        })
        .unwrap_or(1.0);
    let segs = segments(v);
    rsx! {
        div { class,
            onmousedown: move |e: MouseEvent| {
                let x = e.client_coordinates().x - PAD;
                let col = (x / (CELL * scale)).round().max(0.0) as usize;
                (sink.0)(index, col, e.modifiers().contains(Modifiers::SHIFT));
            },
            for seg in segs {
                match seg {
                    Seg::Text(class, text) => rsx! { span { class, "{text}" } },
                    Seg::Caret => rsx! { span { class: "caret-in" } },
                }
            }
        }
    }
}

/// A row's runs with the caret spliced in among them.
enum Seg<'a> {
    Text(&'a str, &'a str),
    Caret,
}

/// The caret as an inline, zero-width span at its column, splitting the run
/// it falls in. Not the parent's absolutely-positioned `div.caret` at
/// `left: Nch`: Blitz lays that out on the first paint and not when a later
/// frame inserts it, so a moved caret vanished. Inline also puts it after the
/// glyphs it follows, whatever their width, so a heading needs no scale.
///
/// Columns are counted in `char`s, which is a display column for everything
/// the corpus holds; a wide character would land it one cell short.
fn segments(v: &RowView) -> Vec<Seg<'_>> {
    let mut out = Vec::with_capacity(v.runs.len() + 2);
    let mut at = v.caret_col;
    let mut col = 0;
    for run in &v.runs {
        let len = run.text.chars().count();
        match at {
            Some(c) if c < col + len => {
                let split = run
                    .text
                    .char_indices()
                    .nth(c - col)
                    .map_or(run.text.len(), |(i, _)| i);
                let (a, b) = run.text.split_at(split);
                if !a.is_empty() {
                    out.push(Seg::Text(&run.class, a));
                }
                out.push(Seg::Caret);
                out.push(Seg::Text(&run.class, b));
                at = None;
            }
            _ => out.push(Seg::Text(&run.class, &run.text)),
        }
        col += len;
    }
    if at.is_some() {
        out.push(Seg::Caret);
    }
    out
}
