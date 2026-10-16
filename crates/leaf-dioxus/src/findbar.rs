//! The find bar on screen: carrying out what it asks for, its clicks, and
//! its markup. What it searches and how is `find.rs`.

use dioxus::prelude::*;
use leaf_core::Doc;

use crate::editor::Editor;
use crate::find::{self, BarView, Focus, Outcome};

/// Where on the find bar a click landed.
#[derive(Clone, Copy, Debug)]
pub enum BarClick {
    Query,
    Replacement,
    Previous,
    Next,
    Replace,
    All,
    Done,
}

impl Editor {
    /// Carry out what the find bar asked for: select a match, replace, close.
    pub fn run_find(&self, o: Outcome) {
        // A field's ⌘V, from the host's clipboard: one line of its text.
        if o == Outcome::Paste {
            let text = self.st.host.read_clipboard().text;
            let o = match text {
                Some(t) => self.st.find.borrow_mut().commit(&t.replace('\n', " ")),
                None => Outcome::Edited,
            };
            return self.run_find(o);
        }
        {
            let mut f = self.st.find.borrow_mut();
            let mut d = self.st.doc.borrow_mut();
            let select = |d: &mut Doc, m: Option<(usize, usize)>| {
                if let Some((s, e)) = m {
                    d.select_range(s, e);
                }
            };
            match o {
                Outcome::Searched => {
                    let origin = f.origin;
                    let m = f.first_from(&d, origin);
                    select(&mut d, m);
                }
                Outcome::Step { forward } => {
                    let m = f.step(&d, forward);
                    select(&mut d, m);
                }
                Outcome::Replace => {
                    find::replace_current(&mut f, &mut d);
                    let m = f.step(&d, true);
                    select(&mut d, m);
                }
                Outcome::ReplaceAll => {
                    let n = find::replace_all(&mut f, &mut d);
                    d.status = Some(format!("{n} replaced"));
                }
                Outcome::Close => f.close(),
                Outcome::Edited | Outcome::Paste | Outcome::Ignored => {}
            }
        }
        self.refresh();
        self.sync_bar();
    }

    /// A click on the find bar.
    pub fn bar_click(&self, what: BarClick) {
        let o = {
            let mut f = self.st.find.borrow_mut();
            match what {
                BarClick::Query => {
                    f.focus = Focus::Query;
                    Outcome::Edited
                }
                BarClick::Replacement => {
                    f.focus = Focus::Replacement;
                    Outcome::Edited
                }
                BarClick::Previous => Outcome::Step { forward: false },
                BarClick::Next => Outcome::Step { forward: true },
                BarClick::Replace => Outcome::Replace,
                BarClick::All => Outcome::ReplaceAll,
                BarClick::Done => Outcome::Close,
            }
        };
        self.run_find(o);
    }
}

/// The find bar's markup. Always present, shown or hidden, because Blitz does
/// not lay out an absolutely positioned element inserted after the first
/// frame. It is `place`d — its left, top and width — at the top of the editor's
/// box.
pub fn render(ed: &Editor, bar: &BarView, place: &str) -> Element {
    let click = |what: BarClick| {
        let ed = ed.clone();
        move |e: MouseEvent| {
            e.prevent_default();
            ed.bar_click(what);
        }
    };
    let display = if bar.open { "block" } else { "none" };
    let h = bar.height();
    rsx! {
        div { class: "find", style: "display: {display}; height: {h}px; {place}",
            onmousedown: move |e: MouseEvent| e.prevent_default(),
            div { class: "find-row",
                span { class: "label", "Find" }
                {field(&bar.query, bar.focus == Focus::Query, click(BarClick::Query))}
                span { class: "count", "{bar.count}" }
                span { class: "btn", onmousedown: click(BarClick::Previous), "‹" }
                span { class: "btn", onmousedown: click(BarClick::Next), "›" }
                span { class: "btn", onmousedown: click(BarClick::Done), "Done" }
            }
            if bar.replacing {
                div { class: "find-row",
                    span { class: "label", "Replace" }
                    {field(&bar.replacement, bar.focus == Focus::Replacement, click(BarClick::Replacement))}
                    span { class: "btn", onmousedown: click(BarClick::Replace), "Replace" }
                    span { class: "btn", onmousedown: click(BarClick::All), "All" }
                }
            }
        }
    }
}

/// A find-bar field: its text, with the caret drawn in it when it has focus.
fn field(f: &find::Field, focused: bool, on_press: impl FnMut(MouseEvent) + 'static) -> Element {
    let (before, after) = f.text.split_at(f.caret.min(f.text.len()));
    let class = if focused { "field focused" } else { "field" };
    rsx! {
        span { class, onmousedown: on_press,
            "{before}"
            if focused {
                span { class: "caret-in" }
            }
            "{after}"
        }
    }
}
