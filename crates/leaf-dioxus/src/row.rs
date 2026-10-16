//! One row: its visual lines, the caret spliced in among their runs, and the
//! pointer that lands on them.

use std::ops::Range;
use std::rc::Rc;

use crate::bridge::rows::{RowHandle, RowView, Run};
use dioxus::html::Modifiers;
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;

use crate::table::{AlignKind, BORDER, Block};
use crate::text::{Kind, RowLayout, byte_of_col};

/// The column's side padding, in CSS pixels: the document's left edge in a
/// window just wide enough for it. A line reports a click's client x less
/// this, as if the column stood at the window's left; the editor takes off
/// the margin that centres it in a wider one ([`Editor::left`]).
///
/// [`Editor::left`]: crate::Editor::left
pub const PAD: f64 = 16.0;

/// What a line belongs to: a row, a cell of the table whose first row is
/// `first`, or a block with nothing finer to hit (a picture).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Row(usize),
    Cell { first: usize, r: usize, c: usize },
    Block { first: usize },
}

/// What the pointer did on a line: at visual line `line` of `at`, `x` CSS
/// pixels from the start of its text. The editor turns that into a position
/// through the row's or the cell's [`RowLayout`], which is the only thing that
/// knows where the glyphs are.
#[derive(Clone, Copy, Debug)]
pub enum Pointer {
    /// The button went down; ⇧ extends the selection.
    Press {
        at: Target,
        line: usize,
        x: f32,
        extend: bool,
        mode: PressMode,
    },
    /// It moved with the button held: a drag-select.
    Drag { at: Target, line: usize, x: f32 },
    /// A finger went down and came up without panning — Blitz's `click`
    /// for a touch, which sends no `mousedown` (`touch.rs`).
    Tap { at: Target, line: usize, x: f32 },
}

impl Pointer {
    /// The same event, `dx` CSS pixels further along its line.
    pub fn shifted(self, dx: f32) -> Self {
        match self {
            Pointer::Press {
                at,
                line,
                x,
                extend,
                mode,
            } => Pointer::Press {
                at,
                line,
                x: x + dx,
                extend,
                mode,
            },
            Pointer::Drag { at, line, x } => Pointer::Drag {
                at,
                line,
                x: x + dx,
            },
            Pointer::Tap { at, line, x } => Pointer::Tap {
                at,
                line,
                x: x + dx,
            },
        }
    }
}

/// What a press is for, beyond placing the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressMode {
    /// A plain press (or ⇧-press).
    Place,
    /// ⌘-press: follow the link or footnote under it.
    Follow,
    /// The secondary button, or ⌃-press: the context menu.
    Context,
}

/// What a press asks for, from its button and modifiers — shared by every
/// line and every block, so a picture right-clicks like text does. Answers
/// whether it extends the selection (⇧), and its mode.
pub fn press_mode(e: &MouseEvent) -> (bool, PressMode) {
    let m = e.modifiers();
    let extend = m.contains(Modifiers::SHIFT);
    // ⌘ arrives as `SUPER`, as it does on a key.
    let mode =
        if e.trigger_button() == Some(MouseButton::Secondary) || m.contains(Modifiers::CONTROL) {
            PressMode::Context
        } else if m.intersects(Modifiers::SUPER | Modifiers::META) {
            PressMode::Follow
        } else {
            PressMode::Place
        };
    (extend, mode)
}

/// Where a row sends what the pointer did on it. A context rather than a prop
/// so [`Row`]'s props stay cheap to compare — the memoization the
/// virtual-DOM spike (`spikes/dioxus-widget`) measured.
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
                Line { sink: sink.clone(), at: Target::Row(index), origin: PAD, line: n, indent: if n > 0 && !lay.repeat { indent } else { 0.0 }, segs }
            }
        }
    }
}

/// One visual line. `origin` is where its text starts in the window, for
/// turning a click's x into one within the text.
#[component]
pub fn Line(
    sink: PointerSink,
    at: Target,
    origin: f64,
    line: usize,
    indent: f32,
    segs: Vec<Seg>,
) -> Element {
    let (drag, tap) = (sink.clone(), sink.clone());
    rsx! {
        // A wrapped list item's lines hang under its text by padding, not by
        // a spacer child: a child that is only sometimes there is a Dioxus
        // placeholder the rest of the time, and Blitz gave that a few
        // pixels of width at the start of the line.
        div { class: "line", style: "padding-left: {indent}px",
            onmousedown: move |e: MouseEvent| {
                let x = (e.client_coordinates().x - origin) as f32;
                let (extend, mode) = press_mode(&e);
                (sink.0)(Pointer::Press { at, line, x, extend, mode });
            },
            onmousemove: move |e: MouseEvent| {
                if e.held_buttons().contains(MouseButton::Primary) {
                    let x = (e.client_coordinates().x - origin) as f32;
                    (drag.0)(Pointer::Drag { at, line, x });
                }
            },
            onclick: move |e: MouseEvent| {
                let x = (e.client_coordinates().x - origin) as f32;
                (tap.0)(Pointer::Tap { at, line, x });
            },
            for seg in segs {
                {seg.render()}
            }
        }
    }
}

/// A piece of a visual line: a run's text, or the caret.
#[derive(Clone, PartialEq, Debug)]
pub enum Seg {
    Text(String, String),
    /// An inline formula's atom: drawn as its picture, at the source offset
    /// `picture.rs` keeps it under.
    Math(String, usize, String),
    Caret,
}

impl Seg {
    fn text(r: &Run, s: &str) -> Self {
        match r.src {
            Some(src) if r.class.split(' ').any(|c| c == "math") => {
                Seg::Math(r.class.clone(), src, s.to_owned())
            }
            _ => Seg::Text(r.class.clone(), s.to_owned()),
        }
    }

    /// The segment as markup.
    pub fn render(self) -> Element {
        match self {
            Seg::Text(class, text) => rsx! { span { class, "{text}" } },
            Seg::Math(class, src, text) => match crate::picture::inline_at(src) {
                Some(p) => rsx! {
                    img { class, src: "{p.url}",
                          style: "width: {p.size.0}px; height: {p.size.1}px; vertical-align: -{p.depth}px;" }
                },
                None => rsx! { span { class, "{text}" } },
            },
            Seg::Caret => rsx! { span { class: "caret-in" } },
        }
    }
}

/// The row cut into its visual lines, the caret spliced in where it falls.
///
/// The caret is an inline, zero-width span between glyphs — not the spike's
/// absolutely-positioned `div.caret` at `left: Nch`, which Blitz lays out on
/// the first paint and not when a later frame inserts it, and which is exact
/// only in a monospace row. Inline, it sits after the glyph it follows in any
/// font, at any size.
pub fn lines(v: &RowView, lay: &RowLayout) -> Vec<Vec<Seg>> {
    let (prefix, body) = v.runs.split_at(lay.prefix_runs.min(v.runs.len()));
    let caret = v.caret_col;

    // The prefix: on the first line always, and on every line for a quote.
    let in_prefix = caret.filter(|&c| c < lay.prefix_cols);
    let pre = splice(
        prefix,
        0..usize::MAX,
        in_prefix.map(|c| byte_of_col(&concat(prefix), c)),
    );
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
        Run {
            text: text.into(),
            class: "g".into(),
            src: None,
        }
    }

    fn texts(segs: &[Seg]) -> String {
        segs.iter()
            .map(|s| match s {
                Seg::Text(_, t) | Seg::Math(_, _, t) => t.as_str(),
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

/// A table, drawn as a grid from core's `TableInfo` (see `table.rs`) in place
/// of the box-drawn rows that spell it. Memoized like a row: a table whose
/// view and layout did not change is not rendered again.
#[component]
pub fn TableBlock(block: Block) -> Element {
    let sink = use_context::<PointerSink>();
    let (v, l) = (&*block.view, &*block.layout);
    let first = block.first;
    let prefix = lines(&v.prefix, &l.prefix)
        .into_iter()
        .next()
        .unwrap_or_default();
    let heights: Vec<f32> = l
        .rows
        .iter()
        .enumerate()
        .map(|(r, g)| g.height + if r == 0 { BORDER } else { 0.0 })
        .collect();
    rsx! {
        div { class: "table", style: "height: {l.height}px",
            for (r, row) in v.rows.iter().enumerate() {
                // The first row carries the table's top border, on its cells
                // so it starts where the grid does, past a quote's gutter.
                div { class: if row.head { "trow head" } else { "trow" },
                    style: "height: {heights[r]}px",
                    span { class: "tprefix", style: "width: {l.indent}px",
                        for seg in prefix.clone() {
                            {seg.render()}
                        }
                    }
                    for c in 0..l.cols.len() {
                        div {
                            class: match row.cells.get(c).map(|x| x.align) {
                                Some(AlignKind::Center) => "tcell center",
                                Some(AlignKind::End) => "tcell end",
                                _ => "tcell",
                            },
                            style: "width: {l.cols[c]}px",
                            if let (Some(cell), Some(Some(cl))) = (row.cells.get(c), l.rows[r].cells.get(c)) {
                                for (n, segs) in lines(&cell.view, cl).into_iter().enumerate() {
                                    Line {
                                        sink: sink.clone(),
                                        at: Target::Cell { first, r, c },
                                        origin: PAD + l.text_left(c) as f64,
                                        line: n,
                                        indent: 0.0,
                                        segs,
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
