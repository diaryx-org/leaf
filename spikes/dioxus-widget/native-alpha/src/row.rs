//! One row: its runs, the caret spliced in among them, and the click that
//! lands on it.

use std::rc::Rc;

use dioxus::html::Modifiers;
use dioxus::prelude::*;
use dioxus_widget_spike::rows::{RowHandle, RowView};

use crate::viewport::scale;

/// The document's left edge in the window, in CSS pixels: `body` has no
/// margin and `.leaf` this much side padding (see `LAYOUT` in `main.rs`), so a
/// click's client x less this is its x within a row.
pub const PAD: f64 = 16.0;

/// One monospace cell at the stylesheet's 15px. SF Mono and Menlo both
/// advance about 0.6em; a click divides by this to find its column. An
/// approximation — Blitz could measure it, but the spike does not ask.
const CELL: f64 = 15.0 * 0.6;

/// Where a row sends the click it received, as (row, column, extend). A
/// context rather than a prop so [`Row`]'s props stay its index and handle,
/// both of which compare cheaply — the memoization the parent measured.
#[derive(Clone)]
pub struct ClickSink(pub Rc<dyn Fn(usize, usize, bool)>);

#[derive(Props, Clone, PartialEq)]
pub struct RowProps {
    pub index: usize,
    pub row: RowHandle,
}

/// One row, memoized by its handle: a row the frame did not rebuild is the
/// same `Rc` as last time, compares equal, and is not rendered again.
#[allow(non_snake_case)]
pub fn Row(props: RowProps) -> Element {
    let sink = use_context::<ClickSink>();
    let v: &RowView = &props.row.0;
    let index = props.index;
    let class = match (v.code, v.decoration) {
        (true, _) => "row code",
        (_, true) => "row decoration",
        _ => "row",
    };
    // A heading's glyphs are wider than a body cell by its font scale.
    let scale = scale(v);
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
