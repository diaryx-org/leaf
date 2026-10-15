//! One row: its visual lines, the caret spliced in among their runs, and the
//! pointer that lands on them.

use std::ops::Range;
use std::rc::Rc;

use dioxus::html::Modifiers;
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use dioxus_widget_spike::rows::{RowHandle, RowView, Run};

use crate::text::{Kind, RowLayout, byte_of_col};

/// The document's left edge in the window, in CSS pixels: `body` has no
/// margin and `.leaf` this much side padding (see `LAYOUT` in `main.rs`), so a
/// click's client x less this is its x within a row.
pub const PAD: f64 = 16.0;

/// What the pointer did on a row: at visual line `line`, `x` CSS pixels from
/// the row's left edge. The editor turns that into a column through the row's
/// [`RowLayout`], which is the only thing that knows where the glyphs are.
#[derive(Clone, Copy, Debug)]
pub enum Pointer {
    /// The button went down; ⇧ extends the selection.
    Press { row: usize, line: usize, x: f32, extend: bool },
    /// It moved with the button held: a drag-select.
    Drag { row: usize, line: usize, x: f32 },
}

/// Where a row sends what the pointer did on it. A context rather than a prop
/// so [`Row`]'s props stay cheap to compare — the memoization the parent
/// measured.
#[derive(Clone)]
pub struct PointerSink(pub Rc<dyn Fn(Pointer)>);

impl PartialEq for PointerSink {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// A row's layout behind a pointer, compared by pointer like [`RowHandle`]:
/// the editor reuses it for every row whose handle and width did not change.
#[derive(Clone)]
pub struct LayoutHandle(pub Rc<RowLayout>);

impl PartialEq for LayoutHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct RowProps {
    pub index: usize,
    pub row: RowHandle,
    pub layout: LayoutHandle,
    /// How the row is set: body text, character cells, or a source line.
    pub kind: Kind,
}

/// One row, memoized by its handle and its layout: a row neither changed is
/// not rendered again.
#[allow(non_snake_case)]
pub fn Row(props: RowProps) -> Element {
    let sink = use_context::<PointerSink>();
    let v: &RowView = &props.row.0;
    let lay: &RowLayout = &props.layout.0;
    let index = props.index;
    let class = match (props.kind, v.code, v.decoration) {
        (Kind::Source, ..) => "row source",
        (_, true, _) => "row code",
        (_, _, true) => "row decoration",
        (Kind::Cells, ..) => "row table",
        _ => "row",
    };
    let lines = lines(v, lay);
    let indent = lay.prefix_w;
    rsx! {
        div { class,
            for (n, segs) in lines.into_iter().enumerate() {
                Line { sink: sink.clone(), row: index, line: n, indent: if n > 0 && !lay.repeat { indent } else { 0.0 }, segs }
            }
        }
    }
}

#[component]
fn Line(sink: PointerSink, row: usize, line: usize, indent: f32, segs: Vec<Seg>) -> Element {
    let drag = sink.clone();
    rsx! {
        // A wrapped list item's lines hang under its text by padding, not by
        // a spacer child: a child that is only sometimes there is a Dioxus
        // placeholder the rest of the time, and Blitz gave that a few
        // pixels of width at the start of the line.
        div { class: "line", style: "padding-left: {indent}px",
            onmousedown: move |e: MouseEvent| {
                let x = (e.client_coordinates().x - PAD) as f32;
                let extend = e.modifiers().contains(Modifiers::SHIFT);
                (sink.0)(Pointer::Press { row, line, x, extend });
            },
            onmousemove: move |e: MouseEvent| {
                if e.held_buttons().contains(MouseButton::Primary) {
                    let x = (e.client_coordinates().x - PAD) as f32;
                    (drag.0)(Pointer::Drag { row, line, x });
                }
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

/// A piece of a visual line: a run's text, or the caret.
#[derive(Clone, PartialEq, Debug)]
pub enum Seg {
    Text(String, String),
    Caret,
}

impl Seg {
    fn text(r: &Run, s: &str) -> Self {
        Seg::Text(r.class.clone(), s.to_owned())
    }
}

/// The row cut into its visual lines, the caret spliced in where it falls.
///
/// The caret is an inline, zero-width span between glyphs — not the parent's
/// absolutely-positioned `div.caret` at `left: Nch`, which Blitz lays out on
/// the first paint and not when a later frame inserts it, and which is exact
/// only in a monospace row. Inline, it sits after the glyph it follows in any
/// font, at any size.
fn lines(v: &RowView, lay: &RowLayout) -> Vec<Vec<Seg>> {
    let (prefix, body) = v.runs.split_at(lay.prefix_runs.min(v.runs.len()));
    let caret = v.caret_col;

    // The prefix: on the first line always, and on every line for a quote.
    let in_prefix = caret.filter(|&c| c < lay.prefix_cols);
    let pre = splice(prefix, 0..usize::MAX, in_prefix.map(|c| byte_of_col(&concat(prefix), c)));
    let bare = splice(prefix, 0..usize::MAX, None);

    // The caret's byte in the body, and which line holds it.
    let in_body = caret
        .filter(|&c| c >= lay.prefix_cols)
        .map(|c| byte_of_col(&concat(body), c - lay.prefix_cols));
    let caret_line = in_body.map(|b| lay.line_of(b));

    lay.lines
        .iter()
        .enumerate()
        .map(|(n, l)| {
            let mut segs = match n {
                0 => pre.clone(),
                _ if lay.repeat => bare.clone(),
                _ => Vec::new(),
            };
            let at = in_body.filter(|_| caret_line == Some(n));
            segs.extend(splice(body, l.bytes.clone(), at));
            segs
        })
        .collect()
}

fn concat(runs: &[Run]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

/// The runs' text within `window` (bytes of their concatenation), with the
/// caret inserted at byte `caret` — before the glyph it precedes, or at the
/// window's end.
fn splice(runs: &[Run], window: Range<usize>, caret: Option<usize>) -> Vec<Seg> {
    let mut out = Vec::new();
    let mut caret = caret.filter(|c| window.contains(c) || *c == window.end);
    let mut start = 0;
    for r in runs {
        let end = start + r.text.len();
        let (a, b) = (start.max(window.start), end.min(window.end));
        if a < b {
            let s = &r.text[a - start..b - start];
            match caret.filter(|&c| (a..b).contains(&c)) {
                Some(c) => {
                    let (x, y) = s.split_at(c - a);
                    if !x.is_empty() {
                        out.push(Seg::text(r, x));
                    }
                    out.push(Seg::Caret);
                    out.push(Seg::text(r, y));
                    caret = None;
                }
                None => out.push(Seg::text(r, s)),
            }
        }
        start = end;
    }
    if caret.is_some() {
        out.push(Seg::Caret);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> Run {
        Run { text: text.into(), class: "g".into(), src: None }
    }

    fn texts(segs: &[Seg]) -> String {
        segs.iter()
            .map(|s| match s {
                Seg::Text(_, t) => t.as_str(),
                Seg::Caret => "|",
            })
            .collect()
    }

    #[test]
    fn the_caret_splits_the_run_it_falls_in() {
        let runs = [run("hello "), run("world")];
        assert_eq!(texts(&splice(&runs, 0..11, Some(8))), "hello wo|rld");
        assert_eq!(texts(&splice(&runs, 0..11, Some(6))), "hello |world");
        assert_eq!(texts(&splice(&runs, 0..11, Some(0))), "|hello world");
        assert_eq!(texts(&splice(&runs, 0..11, Some(11))), "hello world|");
    }

    #[test]
    fn a_window_cuts_runs_at_a_line_break() {
        let runs = [run("hello "), run("world")];
        assert_eq!(texts(&splice(&runs, 3..8, None)), "lo wo");
        assert_eq!(texts(&splice(&runs, 6..11, Some(6))), "|world");
    }

    #[test]
    fn an_empty_row_still_draws_its_caret() {
        assert_eq!(texts(&splice(&[], 0..0, Some(0))), "|");
    }
}

