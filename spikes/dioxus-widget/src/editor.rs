//! The widget itself: leaf's rows as a Dioxus component tree.
//!
//! Two renderers of the same rows, because the difference between them is the
//! spike's central measurement:
//!
//! - [`Document`] renders **a component per row**, keyed, taking a
//!   [`RowHandle`] as its props. A row whose handle is the same `Rc` as last
//!   frame's memoizes: Dioxus does not call its render function and does not
//!   walk its runs. See [`crate::rows::RowHandle`].
//! - [`FlatDocument`] renders the same markup with the rows **inline**, no
//!   child components, which is the obvious way to write it and what a virtual
//!   DOM does unaided: every row is rendered and every run diffed, every
//!   frame.
//!
//! [`LeafEditor`] is the widget a host would actually mount: it owns the
//! document, takes the keys, and draws through [`Document`].

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use leaf_core::{Doc, Format};

use crate::frame::Frame;
use crate::input::{Action, apply_key};
use crate::rows::{Marks, RowHandle, RowView};
use crate::style::STYLESHEET;

/// The rows to draw, as the host and the component share them.
///
/// A `RefCell` rather than a `Signal` so the bench can drive the same
/// components from outside the runtime — and so this crate's measurements are
/// of Dioxus's diff and nothing else. [`LeafEditor`] below is the signal-held
/// version a host would write.
#[derive(Clone, Default)]
pub struct SharedRows(pub Rc<RefCell<Vec<RowHandle>>>);

impl SharedRows {
    pub fn set(&self, rows: &[RowHandle]) {
        self.0.replace(rows.to_vec());
    }
}

/// The document as one component per row. The renderer to use.
#[allow(non_snake_case)]
pub fn Document() -> Element {
    let rows = use_context::<SharedRows>();
    let rows = rows.0.borrow();
    rsx! {
        div { class: "leaf", tabindex: "0",
            for (i, row) in rows.iter().enumerate() {
                Row { key: "{i}", row: row.clone() }
            }
        }
    }
}

/// [`Document`] without the `key`, to find out whether the keys are what makes
/// a reflow expensive. They are not: see README.md's "A row inserted in the
/// middle". Keyed and unkeyed cost the same, because a row has no identity to
/// key *by* — a visual row is a position, and an inserted row shifts every
/// position below it either way.
#[allow(non_snake_case)]
pub fn UnkeyedDocument() -> Element {
    let rows = use_context::<SharedRows>();
    let rows = rows.0.borrow();
    rsx! {
        div { class: "leaf", tabindex: "0",
            for row in rows.iter() {
                Row { row: row.clone() }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct RowProps {
    pub row: RowHandle,
}

/// One row. Memoized by its handle: see [`crate::rows::RowHandle`].
#[allow(non_snake_case)]
pub fn Row(props: RowProps) -> Element {
    row_markup(&props.row.0)
}

/// The same document with no per-row component, so nothing memoizes.
#[allow(non_snake_case)]
pub fn FlatDocument() -> Element {
    let rows = use_context::<SharedRows>();
    let rows = rows.0.borrow();
    rsx! {
        div { class: "leaf", tabindex: "0",
            for row in rows.iter() {
                {row_markup(&row.0)}
            }
        }
    }
}

/// What a row draws, shared by both renderers so they differ only in where the
/// component boundary is.
fn row_markup(v: &RowView) -> Element {
    let class = match (v.code, v.decoration) {
        (true, _) => "row code",
        (_, true) => "row decoration",
        _ => "row",
    };
    rsx! {
        div { class,
            for run in v.runs.iter() {
                if let Some(src) = run.src {
                    span { class: "{run.class}", "data-src": "{src}", "{run.text}" }
                } else {
                    span { class: "{run.class}", "{run.text}" }
                }
            }
            if let Some(col) = v.caret_col {
                div { class: "caret", style: "left: {col}ch" }
            }
        }
    }
}

/// The widget a host mounts: the document, the keys, and the rows.
///
/// What it does *not* do is the point of README.md's "What it would take": no
/// IME composition, no clipboard, no scrolling, no mouse. Those are the parts
/// a DOM editor cannot get from `onkeydown`, and they are what stands between
/// this and a `leaf-dioxus` crate.
#[component]
pub fn LeafEditor(source: String) -> Element {
    // The document, the frame bookkeeping, and the rows all live for the
    // mount. `use_hook` rather than `use_signal` for the document: the rows
    // are what the view reads, and they are a signal of their own below, so
    // the document itself never needs to be diffed or cloned.
    let doc = use_hook(|| {
        let mut d = Doc::from_source(source.clone(), Format::Markdown)
            .unwrap_or_else(|_| Doc::blank().expect("a blank document"));
        d.build_visual(WRAP);
        Rc::new(RefCell::new(d))
    });
    let frame = use_hook(|| Rc::new(RefCell::new(Frame::default())));
    let mut rows = use_signal(Vec::<RowHandle>::new);

    // The first paint, and every repaint after a key.
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
            // Action::Host is where a real widget would answer the host with
            // an `Outcome`, the way leaf-ratatui does.
        }
    };

    rsx! {
        style { {STYLESHEET} }
        div { class: "leaf", tabindex: "0", onkeydown: on_key,
            for (i, row) in rows.read().iter().enumerate() {
                Row { key: "{i}", row: row.clone() }
            }
        }
    }
}

/// The soft-wrap width, in character cells. A monospace row is what makes the
/// `Nch` caret exact; a proportional one is README.md's biggest open question.
const WRAP: usize = 80;
