//! Controls a page puts in its own markup: a button that opens a menu, a
//! select, and chips — a row of choices, one or several on.
//!
//! Each acts on the press, not the click: a press is what dioxus-native
//! delivers to any element, and a menu opened on the press can be chosen
//! from by dragging to an item and letting go, as a Mac's can.

use dioxus::prelude::*;

use crate::layer::use_layer;
use crate::menu::{Entry, Item};

/// A button that opens `entries` under itself; `onpick` is given the chosen
/// item's id. `top` is where the menu's top goes, in the page — the bottom
/// edge of the bar the button sits in, where the host knows it — and by
/// default the button's own bottom.
#[component]
pub fn MenuButton(
    entries: Vec<Entry>,
    onpick: EventHandler<usize>,
    #[props(default)] class: String,
    #[props(default = 26.0)] height: f64,
    top: Option<f64>,
    children: Element,
) -> Element {
    let layer = use_layer();
    let class = format!("ui-menu-button {class}");
    rsx! {
        span { class,
            onmousedown: move |e: MouseEvent| {
                e.prevent_default();
                e.stop_propagation();
                let (x, y) = left_top(&e);
                let top = top.unwrap_or(y + height);
                layer.open_menu(entries.clone(), (x, top), height, move |id| onpick.call(id));
            },
            {children}
        }
    }
}

/// Where the element pressed starts, in the page: the press less its offset
/// into the element.
pub fn left_top(e: &MouseEvent) -> (f64, f64) {
    let page = e.client_coordinates();
    let inside = e.element_coordinates();
    // A renderer that reports no offset gives the press itself.
    let ok = |v: f64| (0.0..2000.0).contains(&v);
    let x = if ok(inside.x) {
        page.x - inside.x
    } else {
        page.x
    };
    let y = if ok(inside.y) {
        page.y - inside.y
    } else {
        page.y
    };
    (x, y)
}

/// One of a list of options, as a select or chips show it.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    /// A style for its label: a font's chip in that font.
    pub style: Option<String>,
}

impl Choice {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            style: None,
        }
    }

    pub fn styled(mut self, style: impl Into<String>) -> Self {
        self.style = Some(style.into());
        self
    }
}

/// A select: the chosen option's label, and a menu of them all, ticked at
/// the chosen one. `onpick` is given the index chosen.
#[component]
pub fn Select(
    choices: Vec<Choice>,
    selected: Option<usize>,
    onpick: EventHandler<usize>,
) -> Element {
    let label = selected
        .and_then(|i| choices.get(i))
        .map_or(String::new(), |c| c.label.clone());
    let entries: Vec<Entry> = choices
        .iter()
        .enumerate()
        .map(|(i, c)| {
            Item::new(i, c.label.clone())
                .checked(selected == Some(i))
                .into()
        })
        .collect();
    rsx! {
        MenuButton { class: "ui-select", entries, onpick, height: 22.0,
            span { class: "ui-select-label", "{label}" }
            span { class: "ui-select-arrows", "⌄" }
        }
    }
}

/// Chips: each option a pill, the ones `on` says lit. `onpick` is given the
/// index pressed.
#[component]
pub fn Chips(choices: Vec<Choice>, on: Vec<bool>, onpick: EventHandler<usize>) -> Element {
    rsx! {
        div { class: "ui-chips",
            for (i, c) in choices.into_iter().enumerate() {
                span {
                    key: "{i}",
                    class: if on.get(i).copied().unwrap_or(false) { "ui-chip on" } else { "ui-chip" },
                    style: c.style.clone().unwrap_or_default(),
                    onmousedown: move |e: MouseEvent| {
                        e.prevent_default();
                        e.stop_propagation();
                        onpick.call(i);
                    },
                    "{c.label}"
                }
            }
        }
    }
}
