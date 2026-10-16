//! One visual row as the nodes a virtual DOM will diff.
//!
//! A [`leaf_core::VRow`] is a glyph per character. A DOM node per character
//! would be absurd, so the glyphs are coalesced into **runs** — the longest
//! stretches sharing one style and one selection state — which is the same
//! shape `leaf-web` hands its renderer as `Run`.
//!
//! ## No source offsets in the DOM
//!
//! A glyph carries the source byte it came from, and the obvious thing is to
//! write that onto the run as `data-src` so a click can read it back. This
//! renderer deliberately does not, and README.md's "What it found" explains
//! why in numbers: a keystroke moves every offset after it, so rows that are
//! otherwise untouched would differ in their attributes, every row below the
//! caret would be rebuilt, and a document's worth of nodes would be diffed per
//! keystroke. Hit-testing goes through
//! [`VisualMap::offset_of_pos`](leaf_core::VisualMap::offset_of_pos) instead —
//! core already maps a (row, column) to an offset, which is what a click has.
//! The rows then hold no offsets, so an edit changes only the rows whose
//! *text* changed.

use std::rc::Rc;

use leaf_core::{Glyph, VRow, VisualMap};

use crate::bridge::style::class_of;

/// A stretch of one row sharing a style and a selection state.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Run {
    pub text: String,
    /// The class list from [`class_of`], plus `sel` when selected.
    pub class: String,
    /// The source offset the run starts at, written out as `data-src` —
    /// **only** under [`Opts::offsets`], which exists to measure what it
    /// costs. `None` is the renderer to use.
    pub src: Option<usize>,
}

/// One row, ready to render: its runs, and where the caret sits among them.
#[derive(Clone, PartialEq, Eq, Hash, Default, Debug)]
pub struct RowView {
    pub runs: Vec<Run>,
    /// The caret's display column in this row, when the caret is on it. The
    /// caret is drawn as one absolutely-positioned node at `left: Nch`, which
    /// is exact in a monospace row and is why this renderer is monospace; see
    /// README.md's "What it would take".
    pub caret_col: Option<usize>,
    /// A row of a fenced or indented code block — the host draws its chrome.
    pub code: bool,
    /// Drawn but holds no caret: a table rule, a block gap.
    pub decoration: bool,
}

/// A row view behind a pointer, compared **by pointer**.
///
/// This is the whole mechanism the memoized renderer turns on. Dioxus skips
/// re-rendering a component whose props compare equal, so a row whose handle
/// is the same `Rc` as last frame's costs nothing — no render, no diff of its
/// runs. [`crate::bridge::frame::Frame`] reuses the handle for every row
/// [`leaf_core::row_delta`] says did not change, which on a keystroke is all
/// of them but one.
///
/// Comparing by pointer rather than by value is what makes the saving real: a
/// by-value `PartialEq` on [`RowView`] would still walk every run of every row
/// to discover they are equal, which is the work being avoided.
#[derive(Clone, Debug)]
pub struct RowHandle(pub Rc<RowView>);

impl PartialEq for RowHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl RowHandle {
    pub fn new(view: RowView) -> Self {
        Self(Rc::new(view))
    }
}

/// How to build a row view.
#[derive(Clone, Copy, Default)]
pub struct Opts {
    /// Write each run's source offset into the DOM as `data-src`, the obvious
    /// way to make a click findable. Off is the renderer to use; on is the
    /// counterfactual README.md's "What it found" measures, because a
    /// keystroke moves every offset after it and so changes every row below
    /// the caret.
    pub offsets: bool,
}

/// Where the caret and the selection are, in the terms a row needs.
#[derive(Clone, Copy, Default)]
pub struct Marks {
    /// The caret's (row, column), from
    /// [`VisualMap::pos_of_offset`](leaf_core::VisualMap::pos_of_offset).
    pub caret: Option<(usize, usize)>,
    /// The selection as source offsets, low end first.
    pub selection: Option<(usize, usize)>,
    /// An IME's marked text — the composition not yet committed — as source
    /// offsets. Its glyphs carry `ime`, which the stylesheet underlines, as
    /// every text view marks a composition.
    pub composing: Option<(usize, usize)>,
}

impl Marks {
    /// The marks for a document, read off its map once per frame.
    pub fn of(vmap: &VisualMap, caret: usize, anchor: Option<usize>) -> Self {
        Self {
            caret: Some(vmap.pos_of_offset(caret)),
            selection: anchor.map(|a| (a.min(caret), a.max(caret))),
            composing: None,
        }
    }

    /// The same marks, with an IME composition over `range`.
    pub fn composing(self, range: Option<(usize, usize)>) -> Self {
        Self {
            composing: range,
            ..self
        }
    }
}

/// Build one row's view: coalesce its glyphs into runs, mark the selected ones,
/// and note the caret's column.
pub fn row_view(vrow: &VRow, row: usize, marks: &Marks, opts: Opts) -> RowView {
    let mut runs: Vec<Run> = Vec::new();
    let mut class = String::new();
    // The style, selection and composition state the open run was opened
    // with. A glyph extends it when all three still match.
    let mut open: Option<(leaf_core::Style, bool, bool)> = None;

    for g in &vrow.glyphs {
        let selected = within(g, marks.selection);
        let composing = within(g, marks.composing);
        // An inline formula's atom is a run of its own, with its offset,
        // which is how its picture is found (`picture.rs`).
        let math = g.style.role == leaf_core::Role::Math;
        let extends = !math && open == Some((g.style, selected, composing));
        if !extends {
            class.clear();
            class_of(&g.style, &mut class);
            if selected {
                class.push_str(" sel");
            }
            if composing {
                class.push_str(" ime");
            }
            runs.push(Run {
                text: String::new(),
                class: class.clone(),
                src: (opts.offsets || math).then_some(g.src),
            });
            open = (!math).then_some((g.style, selected, composing));
        }
        // `drawn()` rather than `ch`: a list item's indent on a continuation
        // row is spelled with the marker's characters for its width and drawn
        // blank, and only core knows that.
        runs.last_mut()
            .expect("a run was just opened")
            .text
            .push(g.drawn());
    }

    RowView {
        runs,
        caret_col: marks.caret.and_then(|(r, c)| (r == row).then_some(c)),
        code: vrow.code,
        decoration: vrow.decoration,
    }
}

fn within(g: &Glyph, range: Option<(usize, usize)>) -> bool {
    range.is_some_and(|(lo, hi)| g.src >= lo && g.src < hi)
}

#[cfg(test)]
mod tests {
    use leaf_core::{Doc, Format};

    use super::*;

    fn doc(src: &str) -> Doc {
        let mut d = Doc::from_source(src.to_string(), Format::Markdown).expect("a document");
        d.build_visual(80);
        d
    }

    #[test]
    fn plain_text_is_one_run() {
        let d = doc("hello world\n");
        let v = row_view(&d.vmap.rows[0], 0, &Marks::default(), Opts::default());
        assert_eq!(v.runs.len(), 1);
        assert_eq!(v.runs[0].text, "hello world");
        assert_eq!(v.runs[0].class, "g");
    }

    #[test]
    fn emphasis_splits_a_row_into_runs() {
        let d = doc("a **bold** c\n");
        let v = row_view(&d.vmap.rows[0], 0, &Marks::default(), Opts::default());
        let text: String = v.runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(text, "a bold c");
        // "a ", "bold", " c" — the delimiters are resolved away by the map.
        assert_eq!(v.runs.len(), 3);
        assert!(v.runs[1].class.contains('b'), "{}", v.runs[1].class);
    }

    #[test]
    fn a_selection_splits_a_run_and_classes_it() {
        let d = doc("hello world\n");
        let marks = Marks {
            caret: None,
            selection: Some((0, 5)),
            composing: None,
        };
        let v = row_view(&d.vmap.rows[0], 0, &marks, Opts::default());
        assert_eq!(v.runs[0].text, "hello");
        assert!(v.runs[0].class.ends_with("sel"));
        assert_eq!(v.runs[1].text, " world");
        assert!(!v.runs[1].class.contains("sel"));
    }

    #[test]
    fn a_composition_is_classed_ime() {
        let d = doc("hello world\n");
        let marks = Marks::default().composing(Some((6, 11)));
        let v = row_view(&d.vmap.rows[0], 0, &marks, Opts::default());
        assert_eq!(v.runs[1].text, "world");
        assert!(v.runs[1].class.ends_with("ime"), "{}", v.runs[1].class);
    }

    #[test]
    fn the_caret_row_carries_its_column() {
        let mut d = doc("hello\n");
        d.place_caret(3, false);
        let marks = Marks::of(&d.vmap, d.caret, d.anchor);
        assert_eq!(
            row_view(&d.vmap.rows[0], 0, &marks, Opts::default()).caret_col,
            Some(3)
        );
    }

    #[test]
    fn handles_compare_by_pointer() {
        let a = RowHandle::new(RowView::default());
        let b = RowHandle::new(RowView::default());
        assert_eq!(a, a.clone(), "the same Rc is the same row");
        assert_ne!(a, b, "equal contents behind different Rcs are not pooled");
    }
}
