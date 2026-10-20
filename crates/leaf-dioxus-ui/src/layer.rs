//! The [`Layer`]: one overlay per page, over everything else, where its
//! menus and its dialog are drawn — and the one place keys go first while
//! one is open.
//!
//! It is mounted with the page and never unmounted: its backdrop, a fixed
//! number of menu levels and the dialog are always in the tree, shown or
//! hidden, and moved by their style. Blitz lays out an absolutely positioned
//! element on the first paint and not when a later frame inserts it, and
//! skips painting a child that hangs past its parent's box — so an overlay
//! made when a menu opens, inside the button that opened it, would not be
//! drawn. One made at the page's root from the start is.

use std::rc::Rc;

use dioxus::html::{Key, Modifiers};
use dioxus::prelude::*;

use crate::dialog::{Answer, Dialog, FieldKey};
use crate::menu::{Entry, Menu, Outcome};

/// How many levels deep a menu may go: the first and three submenus.
pub const LEVELS: usize = 4;

/// What the host answers for the layer: the clipboard, for a dialog's field.
pub trait Clipboard {
    fn read(&self) -> Option<String>;
    fn write(&self, text: &str);
}

type Pick = Rc<dyn Fn(usize)>;
type Done = Rc<dyn Fn(Answer)>;

/// What the layer shows.
#[derive(Clone, Default)]
struct State {
    menu: Option<(Menu, Pick)>,
    dialog: Option<(Dialog, Done)>,
    /// The page's size, which a menu is kept inside.
    viewport: (f64, f64),
}

/// A page's overlay, as every control and the host hold it: a cheap handle.
#[derive(Clone, Copy)]
pub struct Layer {
    state: Signal<State>,
    clipboard: Signal<Option<Rc<dyn Clipboard>>>,
}

/// Make the page's layer and give it to every component under this one.
/// Call it in the page's root component, and put [`Layer::render`] last in
/// its markup so that it is drawn over the rest.
pub fn use_layer_provider() -> Layer {
    let state = use_signal(State::default);
    let clipboard = use_signal(|| None);
    use_context_provider(|| Layer { state, clipboard })
}

/// The page's layer, from a component under the one that made it.
pub fn use_layer() -> Layer {
    use_context::<Layer>()
}

impl Layer {
    /// The page's size, in CSS pixels: set it whenever the window's changes.
    pub fn set_viewport(&self, width: f64, height: f64) {
        let mut s = self.state;
        if s.peek().viewport != (width, height) {
            s.write().viewport = (width, height);
        }
    }

    /// Where a dialog's field reads and writes the clipboard.
    pub fn set_clipboard(&self, clipboard: Rc<dyn Clipboard>) {
        let mut c = self.clipboard;
        c.set(Some(clipboard));
    }

    /// Whether a menu or a dialog is open — while one is, the host gives
    /// the layer its keys first ([`Layer::key`]).
    pub fn is_open(&self) -> bool {
        let s = self.state.peek();
        s.menu.is_some() || s.dialog.is_some()
    }

    /// Open `entries` with the first level's top-left at `at`; `above` is
    /// the height of the control it opened under, which it opens above
    /// instead where there is no room below. `pick` is given the chosen
    /// item's id, after the menu has closed.
    pub fn open_menu(
        &self,
        entries: Vec<Entry>,
        at: (f64, f64),
        above: f64,
        pick: impl Fn(usize) + 'static,
    ) {
        let mut s = self.state;
        let viewport = s.peek().viewport;
        let viewport = if viewport.0 > 0.0 {
            viewport
        } else {
            (1e5, 1e5)
        };
        s.write().menu = Some((Menu::open(entries, at, above, viewport), Rc::new(pick)));
    }

    /// Open `dialog`; `done` is given how it ended, after it has closed.
    pub fn open_dialog(&self, dialog: Dialog, done: impl Fn(Answer) + 'static) {
        let mut s = self.state;
        let mut w = s.write();
        w.menu = None;
        w.dialog = Some((dialog, Rc::new(done)));
    }

    /// Close whatever is open, choosing nothing.
    pub fn close(&self) {
        let mut s = self.state;
        if s.peek().menu.is_some() {
            s.write().menu = None;
        } else if s.peek().dialog.is_some() {
            self.answer(Answer::Cancel);
        }
    }

    /// A key, from the host, before anyone else has it: whether the layer
    /// took it. While a menu or a dialog is open it takes every key, as a
    /// system menu does, but those a menu has no use for: those close it
    /// and go on (⌘S with a menu open still saves).
    pub fn key(&self, key: &Key, mods: Modifiers) -> bool {
        let mut s = self.state;
        if s.peek().menu.is_some() {
            let outcome = s
                .write()
                .menu
                .as_mut()
                .map_or(Outcome::Ignored, |(m, _)| m.key(key, mods));
            return self.settle(outcome);
        }
        if s.peek().dialog.is_none() {
            return false;
        }
        use dioxus::html::NamedKey::*;
        match key {
            Key::Named(Escape) => self.answer(Answer::Cancel),
            Key::Named(Enter) => {
                let text = s
                    .peek()
                    .dialog
                    .as_ref()
                    .and_then(|(d, _)| d.field.as_ref().map(|f| f.text.clone()))
                    .unwrap_or_default();
                self.answer(Answer::Ok(text));
            }
            _ => {
                let outcome = s
                    .write()
                    .dialog
                    .as_mut()
                    .and_then(|(d, _)| d.field.as_mut())
                    .map(|f| f.key(key, mods));
                match outcome {
                    Some(FieldKey::Paste) => {
                        if let Some(t) = self.clipboard.peek().as_ref().and_then(|c| c.read()) {
                            self.type_text(&t);
                        }
                    }
                    Some(FieldKey::Copy(t)) => {
                        if let Some(c) = self.clipboard.peek().as_ref() {
                            c.write(&t);
                        }
                    }
                    _ => {}
                }
            }
        }
        true
    }

    /// Text from the input method — an accent, a composition's commit, the
    /// soft keyboard's — while a dialog is open: whether the layer took it.
    pub fn text(&self, text: &str) -> bool {
        if self.state.peek().dialog.is_none() {
            return self.state.peek().menu.is_some();
        }
        self.type_text(text);
        true
    }

    fn type_text(&self, text: &str) {
        let mut s = self.state;
        if let Some(f) = s
            .write()
            .dialog
            .as_mut()
            .and_then(|(d, _)| d.field.as_mut())
        {
            f.insert(text);
        }
    }

    /// Carry out what the menu said.
    fn settle(&self, outcome: Outcome) -> bool {
        let mut s = self.state;
        match outcome {
            Outcome::Ignored => false,
            Outcome::Handled => true,
            Outcome::Close => {
                s.write().menu = None;
                true
            }
            Outcome::Pick(id) => {
                let pick = s.write().menu.take().map(|(_, p)| p);
                if let Some(p) = pick {
                    p(id);
                }
                true
            }
        }
    }

    fn answer(&self, a: Answer) {
        let mut s = self.state;
        let done = s.write().dialog.take().map(|(_, d)| d);
        if let Some(d) = done {
            d(a);
        }
    }

    /// The overlay's markup: last in the page's.
    pub fn render(&self) -> Element {
        rsx! { LayerView { layer: *self } }
    }
}

impl PartialEq for Layer {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state
    }
}

#[component]
fn LayerView(layer: Layer) -> Element {
    let s = layer.state.read();
    let open = s.menu.is_some() || s.dialog.is_some();
    let levels: Vec<Option<crate::menu::Level>> = (0..LEVELS)
        .map(|i| s.menu.as_ref().and_then(|(m, _)| m.levels.get(i).cloned()))
        .collect();
    let dialog = s.dialog.as_ref().map(|(d, _)| d.clone());
    let (vw, _) = s.viewport;
    drop(s);
    let display = if open { "block" } else { "none" };
    let scrim = if dialog.is_some() { " ui-scrim" } else { "" };
    // The layer itself is always displayed, and lets the pointer through to
    // the page (`pointer-events: none`); what it holds is shown and hidden.
    // Showing the layer in the same frame as a menu's items arrive leaves
    // Blitz (0.3.0-beta.4) a layout child it has already freed, and every
    // later layout panics on it.
    rsx! {
        div { class: "ui-layer",
            // A press anywhere outside a menu closes it, and is not also a
            // press on whatever is under it.
            div { class: "ui-backdrop{scrim}", style: "display: {display}",
                onmousedown: move |e: MouseEvent| {
                    e.prevent_default();
                    e.stop_propagation();
                    let mut st = layer.state;
                    if st.peek().menu.is_some() {
                        st.write().menu = None;
                    }
                },
            }
            for (depth, level) in levels.into_iter().enumerate() {
                MenuLevel { key: "{depth}", layer, depth, level }
            }
            DialogView { layer, dialog, page_width: vw }
        }
    }
}

#[component]
fn MenuLevel(layer: Layer, depth: usize, level: Option<crate::menu::Level>) -> Element {
    // One template whether the level is open or not: Blitz does not lay out
    // an absolutely positioned element a later frame inserts, so this one is
    // only ever restyled, and its items changed.
    let (style, entries, hot) = match &level {
        Some(l) => (
            format!(
                "display: block; left: {}px; top: {}px; width: {}px",
                l.left, l.top, l.width
            ),
            l.entries.clone(),
            l.hot,
        ),
        None => ("display: none".to_string(), Vec::new(), None),
    };
    rsx! {
        div { class: "ui-menu", style,
            onmousedown: move |e: MouseEvent| {
                e.prevent_default();
                e.stop_propagation();
                let mut st = layer.state;
                if let Some((m, _)) = st.write().menu.as_mut() {
                    m.press();
                }
            },
            for (i, entry) in entries.iter().enumerate() {
                {entry_view(layer, depth, i, entry, hot == Some(i))}
            }
        }
    }
}

fn entry_view(layer: Layer, depth: usize, i: usize, entry: &Entry, hot: bool) -> Element {
    let hover = move |_: MouseEvent| {
        let mut st = layer.state;
        let now = st
            .peek()
            .menu
            .as_ref()
            .and_then(|(m, _)| m.levels.get(depth).map(|l| l.hot));
        let armed = st.peek().menu.as_ref().is_some_and(|(m, _)| m.armed);
        if (now != Some(Some(i)) || !armed)
            && let Some((m, _)) = st.write().menu.as_mut()
        {
            m.hover(depth, i);
        }
    };
    let release = move |e: MouseEvent| {
        e.stop_propagation();
        let mut st = layer.state;
        let outcome = st
            .write()
            .menu
            .as_mut()
            .map_or(Outcome::Ignored, |(m, _)| m.release(depth, i));
        layer.settle(outcome);
    };
    match entry {
        Entry::Separator => rsx! { div { class: "ui-sep", div { class: "ui-rule" } } },
        Entry::Item(item) => {
            let class = format!(
                "ui-item{}{}",
                if hot { " hot" } else { "" },
                if item.enabled { "" } else { " off" }
            );
            let tick = if item.checked { "✓" } else { "" };
            let shortcut = item.shortcut.clone().unwrap_or_default();
            rsx! {
                div { class, onmousemove: hover, onmouseup: release,
                    span { class: "ui-tick", "{tick}" }
                    span { class: "ui-label", "{item.label}" }
                    span { class: "ui-key", "{shortcut}" }
                }
            }
        }
        Entry::Submenu { label, enabled, .. } => {
            let class = format!(
                "ui-item{}{}",
                if hot { " hot" } else { "" },
                if *enabled { "" } else { " off" }
            );
            rsx! {
                div { class, onmousemove: hover, onmouseup: release,
                    span { class: "ui-tick" }
                    span { class: "ui-label", "{label}" }
                    span { class: "ui-key", "▸" }
                }
            }
        }
    }
}

/// The dialog's width, in CSS pixels.
const DIALOG_WIDTH: f64 = 380.0;

#[component]
fn DialogView(layer: Layer, dialog: Option<Dialog>, page_width: f64) -> Element {
    // One template open or closed, for the reason a menu level has one.
    let left = ((page_width - DIALOG_WIDTH) / 2.0).max(8.0);
    let style = match dialog {
        Some(_) => format!("display: block; left: {left}px; width: {DIALOG_WIDTH}px"),
        None => "display: none".to_string(),
    };
    let d = dialog.unwrap_or_else(|| Dialog::new(""));
    let button = move |label: String, ok: bool| {
        let class = if ok {
            "ui-button ui-default"
        } else {
            "ui-button"
        };
        rsx! {
            span { class,
                onmousedown: move |e: MouseEvent| {
                    e.prevent_default();
                    e.stop_propagation();
                },
                onmouseup: move |e: MouseEvent| {
                    e.stop_propagation();
                    if ok {
                        let text = layer.state.peek().dialog.as_ref()
                            .and_then(|(d, _)| d.field.as_ref().map(|f| f.text.clone()))
                            .unwrap_or_default();
                        layer.answer(Answer::Ok(text));
                    } else {
                        layer.answer(Answer::Cancel);
                    }
                },
                "{label}"
            }
        }
    };
    rsx! {
        div { class: "ui-dialog", style,
            onmousedown: move |e: MouseEvent| {
                e.prevent_default();
                e.stop_propagation();
            },
            div { class: "ui-title", "{d.title}" }
            if let Some(m) = &d.message {
                div { class: "ui-message", "{m}" }
            }
            if let Some(f) = &d.field {
                {field_view(f)}
            }
            div { class: "ui-buttons",
                {button(d.cancel.clone(), false)}
                {button(d.ok.clone(), true)}
            }
        }
    }
}

fn field_view(f: &crate::dialog::Field) -> Element {
    let (before, after) = f.split();
    let class = if f.all { "ui-field all" } else { "ui-field" };
    if f.text.is_empty() {
        return rsx! {
            div { class,
                span { class: "ui-caret" }
                span { class: "ui-placeholder", "{f.placeholder}" }
            }
        };
    }
    rsx! {
        div { class,
            span { "{before}" }
            span { class: "ui-caret" }
            span { "{after}" }
        }
    }
}
