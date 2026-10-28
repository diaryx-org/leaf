//! One row: its visual lines, the caret spliced in among their runs, and the
//! pointer that lands on them.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use crate::bridge::rows::{RowHandle, RowView, Run};
use dioxus::html::Modifiers;
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;

use crate::table::{AlignKind, Block};
use crate::text::{Kind, QUOTE_BAR, RowLayout, byte_of_col};

/// The column's side padding, in CSS pixels: the document's left edge in a
/// window just wide enough for it. A line reports a click's client x less
/// this, as if the column stood at the window's left; the editor takes off
/// the margin that centres it in a wider one ([`Editor::left`]).
///
/// [`Editor::left`]: crate::Editor::left
pub const PAD: f64 = 16.0;

/// What a line belongs to: a row, a cell of the table whose first row is
/// `first`, or a block with nothing finer to hit (a picture). Or, from a
/// host that draws the document laid out its own way (Leaf's page view),
/// the byte of the source a press landed on, which the host found itself:
/// the [`Pointer`]'s `line` is then the host's line, which a second press
/// must be on to count as a double-click, and its `x` is not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Row(usize),
    Cell { first: usize, r: usize, c: usize },
    Block { first: usize },
    Offset(usize),
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
    // ⌘ arrives as `META`, as it does on a key.
    let mode =
        if e.trigger_button() == Some(MouseButton::Secondary) || m.contains(Modifiers::CONTROL) {
            PressMode::Context
        } else if m.contains(Modifiers::META) {
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

/// Every visual line mounted, by what it belongs to and its number there:
/// the element Blitz laid it out as, to ask where it is drawn
/// ([`Editor::drawn_rect_of`]). Each entry carries the id of the [`Line`]
/// that put it there, so a line going takes out its own entry and not one a
/// line now at its place has put in.
///
/// [`Editor::drawn_rect_of`]: crate::Editor::drawn_rect_of
#[derive(Clone, Default)]
pub struct LineBoxes(pub Rc<RefCell<HashMap<LineAt, LineBox>>>);

/// A visual line's place: what it belongs to, and its number there.
pub type LineAt = (Target, usize);

/// A mounted line: the id of the [`Line`] that mounted it, and its element.
pub type LineBox = (u64, Rc<MountedData>);

impl PartialEq for LineBoxes {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

thread_local! {
    static NEXT_LINE: Cell<u64> = const { Cell::new(0) };
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
    // Where this line is, kept under its place for as long as it is
    // mounted — and moved when the line is given another place, as a row
    // is when one above it goes.
    let boxes = try_use_context::<LineBoxes>();
    let me = use_hook(|| NEXT_LINE.with(|n| n.replace(n.get() + 1)));
    let kept = use_hook(|| Rc::new(RefCell::new(None::<(LineAt, Rc<MountedData>)>)));
    if let Some(b) = &boxes {
        let mut k = kept.borrow_mut();
        if let Some((key, data)) = k.as_mut()
            && *key != (at, line)
        {
            let mut map = b.0.borrow_mut();
            if map.get(key).is_some_and(|(id, _)| *id == me) {
                map.remove(key);
            }
            *key = (at, line);
            map.insert(*key, (me, data.clone()));
        }
    }
    use_drop({
        let (boxes, kept) = (boxes.clone(), kept.clone());
        move || {
            if let (Some(b), Some((key, _))) = (boxes, kept.borrow_mut().take()) {
                let mut map = b.0.borrow_mut();
                if map.get(&key).is_some_and(|(id, _)| *id == me) {
                    map.remove(&key);
                }
            }
        }
    });
    rsx! {
        // A wrapped list item's lines hang under its text by padding, not by
        // a spacer child: a child that is only sometimes there is a Dioxus
        // placeholder the rest of the time, and Blitz gave that a few
        // pixels of width at the start of the line.
        div { class: "line", style: "padding-left: {indent}px",
            onmounted: move |e: MountedEvent| {
                let data = e.data();
                if let Some(b) = &boxes {
                    b.0.borrow_mut().insert((at, line), (me, data.clone()));
                }
                *kept.borrow_mut() = Some(((at, line), data));
            },
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
    /// An inline formula's atom: its source offset, its TeX, and the
    /// picture it is drawn as, as laid out — carried here rather than looked
    /// up when drawn, so that a picture inked anew for another appearance is
    /// a changed line, which a memoized [`Line`] draws again.
    Math(String, usize, String, Option<Rc<crate::picture::Inline>>),
    Caret,
    /// A grab handle at one end of the selection: its knob above the start
    /// (`false`) or below the end (`true`).
    Grip(bool),
    /// One level of a quote's gutter, in place of the `│ ` core spells it
    /// with: a cell [`Type::quote_indent`] wide with a bar down its left
    /// edge, this many pixels tall — the whole line, so the bars of a
    /// quote's lines and rows meet in one unbroken rule, as Swift's
    /// `BlockChrome` draws it.
    ///
    /// [`Type::quote_indent`]: crate::text::Type::quote_indent
    Bar(String, f32),
}

impl Seg {
    fn text(r: &Run, s: &str) -> Self {
        match r.src {
            Some(src) if r.class.split(' ').any(|c| c == "math") => Seg::Math(
                r.class.clone(),
                src,
                s.to_owned(),
                crate::picture::inline_at(src),
            ),
            _ => Seg::Text(r.class.clone(), s.to_owned()),
        }
    }

    /// The same bar, `height` pixels tall: a table row's or a picture's
    /// height, which its prefix stands beside.
    pub fn tall(self, height: f32) -> Self {
        match self {
            Seg::Bar(class, _) => Seg::Bar(class, height),
            seg => seg,
        }
    }

    /// A prefix's text kept for its width and not drawn: a quoted list
    /// item's marker on the lines after its first, which hang under it.
    fn ghost(self) -> Self {
        match self {
            Seg::Text(class, text) if !is_gutter(&class) => Seg::Text(class + " ghost", text),
            seg => seg,
        }
    }

    /// The segment as markup.
    pub fn render(self) -> Element {
        match self {
            Seg::Text(class, text) => rsx! { span { class, "{text}" } },
            Seg::Math(class, _, text, picture) => match picture {
                Some(p) => rsx! {
                    img { class, src: "{p.url}",
                          style: "width: {p.size.0}px; height: {p.size.1}px; vertical-align: -{p.depth}px;" }
                },
                None => rsx! { span { class, "{text}" } },
            },
            Seg::Caret => rsx! { span { class: "caret-in" } },
            // The stem and the knob are blocks inside a box exactly their
            // size, which hangs above or below the line by negative margins:
            // the line keeps the height parley measured, and Blitz paints
            // nothing that hangs past its parent.
            Seg::Grip(end) => rsx! {
                span { class: if end { "grip end" } else { "grip start" },
                    if !end { span { class: "knob" } }
                    span { class: "stem" }
                    if end { span { class: "knob" } }
                }
            },
            Seg::Bar(class, height) => rsx! {
                span { class: "{class} qbar", style: "height: {height}px" }
            },
        }
    }
}

fn is_gutter(class: &str) -> bool {
    class.split(' ').any(|c| c == "gutter")
}

/// `segs` with each quote gutter's text as its bars, `height` pixels tall.
/// The gutter's spaces go: `text.rs` gives them no room.
fn barred(segs: Vec<Seg>, height: f32) -> Vec<Seg> {
    let mut out = Vec::with_capacity(segs.len());
    for seg in segs {
        match seg {
            Seg::Text(class, text) if is_gutter(&class) => out.extend(
                text.chars()
                    .filter(|&c| c == QUOTE_BAR)
                    .map(|_| Seg::Bar(class.clone(), height)),
            ),
            seg => out.push(seg),
        }
    }
    out
}

/// The row cut into its visual lines, the caret spliced in where it falls,
/// and the selection's grab handles at its ends ([`RowView::grips`]) — in
/// place of the caret, which a text view hides while it has them.
///
/// The caret is an inline, zero-width span between glyphs — not the spike's
/// absolutely-positioned `div.caret` at `left: Nch`, which Blitz lays out on
/// the first paint and not when a later frame inserts it, and which is exact
/// only in a monospace row. Inline, it sits after the glyph it follows in any
/// font, at any size. The handles are inline for the same reason.
pub fn lines(v: &RowView, lay: &RowLayout) -> Vec<Vec<Seg>> {
    let (prefix, body) = v.runs.split_at(lay.prefix_runs.min(v.runs.len()));
    let (prefix_text, body_text) = (concat(prefix), concat(body));
    let gripped = v.grips != (None, None);

    // Where a mark at column `c` goes: in the prefix (line 0) or the body,
    // at which byte, on which line. The end of a selection that stops where
    // a line wraps stays on the line it ends, with the glyphs it follows;
    // anything else goes on the line its byte starts.
    let place = |c: usize, after: bool| -> (bool, usize, usize) {
        if c < lay.prefix_cols {
            return (true, byte_of_col(&prefix_text, c), 0);
        }
        let b = byte_of_col(&body_text, c - lay.prefix_cols);
        let line = match after {
            true => lay
                .lines
                .iter()
                .rposition(|l| l.bytes.start < b)
                .unwrap_or(0),
            false => lay.line_of(b),
        };
        (false, b, line)
    };
    let mut marks: Vec<(bool, usize, usize, Seg)> = Vec::new();
    if let Some(c) = v.grips.0 {
        let (p, b, l) = place(c, false);
        marks.push((p, b, l, Seg::Grip(false)));
    }
    if let Some(c) = v.caret_col.filter(|_| !gripped) {
        let (p, b, l) = place(c, false);
        marks.push((p, b, l, Seg::Caret));
    }
    if let Some(c) = v.grips.1 {
        let (p, b, l) = place(c, true);
        marks.push((p, b, l, Seg::Grip(true)));
    }
    let on = |in_prefix: bool, line: usize| -> Vec<(usize, Seg)> {
        marks
            .iter()
            .filter(|m| m.0 == in_prefix && (in_prefix || m.2 == line))
            .map(|m| (m.1, m.3.clone()))
            .collect()
    };

    // The prefix: on the first line always. A quote's bars go down every
    // line; the rest of its prefix — a quoted list item's marker — is only
    // room on the lines after the first, so the item hangs under its text.
    let pre = splice(prefix, 0..usize::MAX, &on(true, 0));
    let bare: Vec<Seg> = splice(prefix, 0..usize::MAX, &[])
        .into_iter()
        .map(Seg::ghost)
        .collect();

    lay.lines
        .iter()
        .enumerate()
        .map(|(n, l)| {
            let mut segs = match n {
                0 => barred(pre.clone(), l.height),
                _ if lay.repeat => barred(bare.clone(), l.height),
                _ => Vec::new(),
            };
            segs.extend(splice(body, l.bytes.clone(), &on(false, n)));
            segs
        })
        .collect()
}

fn concat(runs: &[Run]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

/// The runs' text within `window` (bytes of their concatenation), with each
/// of `marks` — the caret, a grab handle — inserted at its byte: before the
/// glyph it precedes, or at the window's end. Marks at one byte go in the
/// order given.
fn splice(runs: &[Run], window: Range<usize>, marks: &[(usize, Seg)]) -> Vec<Seg> {
    let mut out = Vec::new();
    let mut marks: Vec<&(usize, Seg)> = marks
        .iter()
        .filter(|(c, _)| window.contains(c) || *c == window.end)
        .collect();
    marks.sort_by_key(|(c, _)| *c);
    let mut marks = marks.into_iter().peekable();
    let mut start = 0;
    for r in runs {
        let end = start + r.text.len();
        let (a, b) = (start.max(window.start), end.min(window.end));
        if a < b {
            let mut from = a;
            while let Some((c, seg)) = marks.next_if(|(c, _)| (a..b).contains(c)) {
                if *c > from {
                    out.push(Seg::text(r, &r.text[from - start..c - start]));
                    from = *c;
                }
                out.push(seg.clone());
            }
            out.push(Seg::text(r, &r.text[from - start..b - start]));
        }
        start = end;
    }
    out.extend(marks.map(|(_, seg)| seg.clone()));
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
                Seg::Text(_, t) | Seg::Math(_, _, t, _) => t.as_str(),
                Seg::Caret => "|",
                Seg::Grip(false) => "[",
                Seg::Grip(true) => "]",
                Seg::Bar(..) => "┃",
            })
            .collect()
    }

    fn styled(text: &str, class: &str) -> Run {
        Run {
            text: text.into(),
            class: class.into(),
            src: None,
        }
    }

    const PROSE: &str = "The quick brown fox jumps over the lazy dog, and then it does so again, \
                         at length, until the line has no choice but to wrap at least twice over.";

    #[test]
    fn a_quote_draws_a_bar_per_level_down_every_line() {
        use crate::text::{Fonts, Type};
        let v = RowView {
            runs: vec![styled("│ │ ", "g gutter"), run(PROSE)],
            caret_col: Some(4),
            ..RowView::default()
        };
        let lay = Fonts::new(Type::default()).layout(&v, 300.0, Kind::Prose);
        let lines = lines(&v, &lay);
        assert!(lines.len() >= 3, "{} lines", lines.len());
        for (n, segs) in lines.iter().enumerate() {
            let text = texts(segs);
            assert!(text.starts_with("┃┃"), "line {n}: {text:?}");
            assert!(!text.contains('│'), "line {n} draws the glyph: {text:?}");
            // Each bar is its line's height, so the lines' bars meet.
            for seg in segs {
                if let Seg::Bar(_, h) = seg {
                    assert_eq!(*h, lay.lines[n].height, "line {n}");
                }
            }
        }
        assert!(texts(&lines[0]).starts_with("┃┃|The"), "{:?}", lines[0]);
    }

    #[test]
    fn a_quoted_list_item_wears_its_marker_on_its_first_line_only() {
        use crate::text::{Fonts, Type};
        let v = RowView {
            runs: vec![
                styled("│ ", "g gutter"),
                styled("• ", "g marker"),
                run(PROSE),
            ],
            ..RowView::default()
        };
        let lay = Fonts::new(Type::default()).layout(&v, 300.0, Kind::Prose);
        let lines = lines(&v, &lay);
        assert!(lines[0].contains(&Seg::Text("g marker".into(), "• ".into())));
        for segs in &lines[1..] {
            assert!(matches!(segs[0], Seg::Bar(..)));
            // The marker stays as room, so the text hangs under the first
            // line's, but it is not drawn again.
            assert_eq!(segs[1], Seg::Text("g marker ghost".into(), "• ".into()));
        }
    }

    #[test]
    fn a_bar_beside_a_table_row_or_a_picture_is_as_tall_as_it() {
        assert_eq!(
            Seg::Bar("g gutter".into(), 24.0).tall(40.0),
            Seg::Bar("g gutter".into(), 40.0)
        );
        assert_eq!(Seg::Caret.tall(40.0), Seg::Caret);
    }

    #[test]
    fn the_caret_splits_the_run_it_falls_in() {
        let runs = [run("hello "), run("world")];
        assert_eq!(
            texts(&splice(&runs, 0..11, &[(8, Seg::Caret)])),
            "hello wo|rld"
        );
        assert_eq!(
            texts(&splice(&runs, 0..11, &[(6, Seg::Caret)])),
            "hello |world"
        );
        assert_eq!(
            texts(&splice(&runs, 0..11, &[(0, Seg::Caret)])),
            "|hello world"
        );
        assert_eq!(
            texts(&splice(&runs, 0..11, &[(11, Seg::Caret)])),
            "hello world|"
        );
    }

    #[test]
    fn a_window_cuts_runs_at_a_line_break() {
        let runs = [run("hello "), run("world")];
        assert_eq!(texts(&splice(&runs, 3..8, &[])), "lo wo");
        assert_eq!(texts(&splice(&runs, 6..11, &[(6, Seg::Caret)])), "|world");
    }

    #[test]
    fn a_selection_has_a_handle_at_each_end() {
        let runs = [run("hello "), run("world")];
        let both = [(6, Seg::Grip(false)), (11, Seg::Grip(true))];
        assert_eq!(texts(&splice(&runs, 0..11, &both)), "hello [world]");
        let inner = [(2, Seg::Grip(false)), (4, Seg::Grip(true))];
        assert_eq!(texts(&splice(&runs, 0..11, &inner)), "he[ll]o world");
    }

    #[test]
    fn an_empty_row_still_draws_its_caret() {
        assert_eq!(texts(&splice(&[], 0..0, &[(0, Seg::Caret)])), "|");
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
        .map(|(r, g)| g.height + if r == 0 { l.rules.down } else { 0.0 })
        .collect();
    // The first column's cells draw the table's left rule too, inside their
    // boxes, past which `text_left` starts the text.
    let widths: Vec<f32> = l
        .cols
        .iter()
        .enumerate()
        .map(|(c, w)| w + if c == 0 { l.rules.across } else { 0.0 })
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
                            {seg.tall(heights[r]).render()}
                        }
                    }
                    for c in 0..l.cols.len() {
                        div {
                            class: match row.cells.get(c).map(|x| x.align) {
                                Some(AlignKind::Center) => "tcell center",
                                Some(AlignKind::End) => "tcell end",
                                _ => "tcell",
                            },
                            style: "width: {widths[c]}px",
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
