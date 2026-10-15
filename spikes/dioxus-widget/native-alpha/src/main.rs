//! The same leaf rows through Dioxus's **experimental native backend**
//! (`dioxus-native` 0.8.0-alpha.2, Blitz): an HTML/CSS renderer on wgpu, with
//! no webview and no browser engine.
//!
//! ```sh
//! cargo run -- ../corpus/document.md
//! ```
//!
//! The markup is duplicated from the parent's `editor.rs` rather than shared,
//! because `rsx!` builds types from *its* Dioxus and the two versions cannot
//! meet. Everything underneath it — the glyph→run coalescing, the class
//! mapping, the frame delta — is the parent crate's, which is the point: the
//! bridge is independent of which Dioxus renders it.

use dioxus::prelude::*;
use dioxus_widget_spike::rows::{Marks, Opts, RowView, row_view};
use dioxus_widget_spike::style::STYLESHEET;
use leaf_core::{Doc, Format};

const WRAP: usize = 80;

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
    // Built once: this screenshot is of the first paint, and the keystroke
    // measurements are the parent's bench, not this binary's.
    let views = use_hook(|| {
        let mut doc = Doc::from_source(source, Format::Markdown)
            .unwrap_or_else(|_| Doc::blank().expect("a blank document"));
        doc.build_visual(WRAP);
        doc.place_caret(0, false);
        let marks = Marks::of(&doc.vmap, doc.caret, doc.anchor);
        doc.vmap
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| row_view(r, i, &marks, Opts::default()))
            .collect::<Vec<_>>()
    });

    rsx! {
        style { {STYLESHEET} }
        div { class: "leaf",
            for v in views.iter() {
                {row(v)}
            }
        }
    }
}

fn row(v: &RowView) -> Element {
    let class = match (v.code, v.decoration) {
        (true, _) => "row code",
        (_, true) => "row decoration",
        _ => "row",
    };
    rsx! {
        div { class,
            for run in v.runs.iter() {
                span { class: "{run.class}", "{run.text}" }
            }
            if let Some(col) = v.caret_col {
                div { class: "caret", style: "left: {col}ch" }
            }
        }
    }
}
