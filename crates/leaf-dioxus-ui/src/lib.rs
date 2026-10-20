//! leaf-dioxus-ui — menus, a dialog with a text field, a select and chips
//! for a Dioxus page, that work on dioxus-native (Blitz) as they do on the
//! web, and so on every platform Dioxus draws to.
//!
//! Dioxus's own primitives (`dioxus-primitives`) are written for a page in
//! a browser, and three things they lean on are not there under Blitz:
//!
//! | The primitives | Here |
//! |---|---|
//! | close on Escape and on a press outside through `document::eval`, a JavaScript listener — which dioxus-native answers with a no-op | a backdrop under an open menu takes the press outside; the host gives the layer its keys first ([`Layer::key`]) |
//! | move DOM focus with `MountedData::set_focus` from a task, which on dioxus-native finds Blitz's document borrowed and panics | a highlighted item, held in the menu's state, that the arrow keys move |
//! | read keys from the focused element's `onkeydown` | the host's keys: a page on Blitz reads them from winit, since Blitz drops DOM focus on any press that is not on a form control |
//!
//! and two Blitz behaviours shape where things are drawn: it lays out an
//! absolutely positioned element on the first paint but not when a later
//! frame inserts it, and it skips painting a child that hangs past its
//! parent's box. So every overlay is part of one [`Layer`] at the page's
//! root, mounted from the first frame and shown or hidden — never inside the
//! button that opened it. The layer itself is never hidden: showing a box in
//! the same frame as rows arrive in an absolutely positioned child of it
//! leaves Blitz 0.3.0-beta.4 a freed node in its layout tree, and every
//! layout after panics. It lets the pointer through instead
//! (`pointer-events: none`), and its backdrop and panels are shown and
//! hidden.
//!
//! None of that is particular to Blitz in a way that breaks a browser: a
//! backdrop, a highlighted index and keys routed by the host behave the same
//! on the web.
//!
//! # Using it
//!
//! ```ignore
//! fn Page() -> Element {
//!     let layer = use_layer_provider();
//!     rsx! {
//!         style { {leaf_dioxus_ui::CSS} }
//!         MenuButton { entries: vec![Item::new(1, "Bold").shortcut("⌘B").into()],
//!                      onpick: move |id| { /* … */ }, "Format" }
//!         {layer.render()}
//!     }
//! }
//! ```
//!
//! and in the host's key handler, before anything else has the key:
//! `if layer.key(&key, mods) { return }` — and `layer.text(&s)` for the
//! input method's text. [`Layer::set_viewport`] keeps menus inside the
//! window.

mod controls;
mod dialog;
mod layer;
mod menu;

pub use controls::{Chips, Choice, MenuButton, Select, left_top};
pub use dialog::{Answer, Dialog, Field};
pub use layer::{Clipboard, Layer, use_layer, use_layer_provider};
pub use menu::{Entry, Item};

/// The stylesheet: every colour a custom property set for the light and the
/// dark appearance, as leaf-dioxus's own stylesheet does, so a host that
/// forces one appearance rewrites both the same way.
pub const CSS: &str = r#"
:root {
  --ui-bg: #f6f6f6;
  --ui-text: #1f1f1f;
  --ui-dim: #a0a0a0;
  --ui-border: #c9c9c9;
  --ui-hot-bg: #2f6fde;
  --ui-hot-text: #ffffff;
  --ui-scrim: rgba(0, 0, 0, 0.18);
  --ui-field-bg: #ffffff;
  --ui-select-bg: #b9d2f7;
  --ui-chip-bg: #ffffff;
  --ui-chip-on: #2f6fde;
  --ui-chip-on-text: #ffffff;
}
@media (prefers-color-scheme: dark) {
  :root {
    --ui-bg: #2c2c2e;
    --ui-text: #e8e8e8;
    --ui-dim: #6e6e72;
    --ui-border: #4a4a4d;
    --ui-hot-bg: #2f6fde;
    --ui-hot-text: #ffffff;
    --ui-scrim: rgba(0, 0, 0, 0.35);
    --ui-field-bg: #1e1e1e;
    --ui-select-bg: #2b4a7a;
    --ui-chip-bg: #1e1e1e;
    --ui-chip-on: #3b7ff0;
    --ui-chip-on-text: #ffffff;
  }
}
.ui-layer { position: fixed; left: 0; top: 0; right: 0; bottom: 0; z-index: 100;
            pointer-events: none;
            font: 13px system-ui, -apple-system, sans-serif; white-space: nowrap; }
.ui-backdrop, .ui-menu, .ui-dialog { pointer-events: auto; }
.ui-backdrop { position: absolute; left: 0; top: 0; right: 0; bottom: 0; }
.ui-backdrop.ui-scrim { background: var(--ui-scrim); }
.ui-menu { position: absolute; box-sizing: border-box; padding: 5px 0;
           background: var(--ui-bg); color: var(--ui-text);
           border: 1px solid var(--ui-border); border-radius: 7px; }
.ui-item { display: flex; height: 22px; line-height: 22px; margin: 0 5px; padding: 0 8px 0 0;
           border-radius: 4px; }
.ui-item.hot { background: var(--ui-hot-bg); color: var(--ui-hot-text); }
.ui-item.off { color: var(--ui-dim); }
.ui-tick { width: 20px; flex: none; text-align: center; }
.ui-label { flex: 1; }
.ui-key { flex: none; padding-left: 18px; color: var(--ui-dim); }
.ui-item.hot .ui-key { color: var(--ui-hot-text); }
.ui-sep { height: 9px; padding: 4px 10px; box-sizing: border-box; }
.ui-rule { height: 1px; background: var(--ui-border); }
.ui-dialog { position: absolute; top: 72px; box-sizing: border-box; padding: 16px 18px 14px;
             background: var(--ui-bg); color: var(--ui-text);
             border: 1px solid var(--ui-border); border-radius: 10px; white-space: normal; }
.ui-title { font-weight: 700; margin-bottom: 6px; }
.ui-message { margin-bottom: 8px; color: var(--ui-text); }
.ui-field { height: 24px; line-height: 24px; padding: 0 6px; margin: 6px 0 12px;
            background: var(--ui-field-bg); border: 1px solid var(--ui-border); border-radius: 5px;
            white-space: pre; }
.ui-field.all { background: var(--ui-select-bg); }
.ui-caret { display: inline-block; width: 1px; height: 16px; vertical-align: middle;
            background: var(--ui-text); }
.ui-field.all .ui-caret { width: 0; }
.ui-placeholder { color: var(--ui-dim); }
.ui-buttons { display: flex; gap: 8px; justify-content: flex-end; }
.ui-button { height: 24px; line-height: 24px; padding: 0 14px; border-radius: 6px;
             background: var(--ui-field-bg); border: 1px solid var(--ui-border); }
.ui-button.ui-default { background: var(--ui-hot-bg); color: var(--ui-hot-text);
                        border-color: var(--ui-hot-bg); }
.ui-select { display: inline-flex; height: 22px; line-height: 22px; padding: 0 6px 0 9px;
             background: var(--ui-field-bg); border: 1px solid var(--ui-border);
             border-radius: 5px; }
.ui-select-label { flex: 1; padding-right: 10px; }
.ui-select-arrows { color: var(--ui-dim); }
.ui-chips { display: flex; flex-wrap: wrap; gap: 4px; }
.ui-chip { padding: 2px 8px; border-radius: 5px; background: var(--ui-chip-bg);
           border: 1px solid var(--ui-border); white-space: nowrap; }
.ui-chip.on { background: var(--ui-chip-on); color: var(--ui-chip-on-text);
              border-color: var(--ui-chip-on); }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every property the stylesheet uses is set for both appearances.
    #[test]
    fn every_colour_is_set_for_light_and_dark() {
        let dark_at = CSS.find("@media (prefers-color-scheme: dark)").unwrap();
        let (light, dark) = CSS.split_at(dark_at);
        let set = |block: &str, name: &str| block.contains(&format!("{name}:"));
        for used in CSS.split("var(").skip(1) {
            let name = &used[..used.find(')').unwrap()];
            assert!(set(light, name), "{name} unset in light");
            assert!(set(dark, name), "{name} unset in dark");
        }
    }

    #[test]
    fn a_page_with_a_layer_renders_with_nothing_open() {
        #[allow(non_snake_case)]
        fn Page() -> dioxus::prelude::Element {
            use dioxus::prelude::*;
            let layer = use_layer_provider();
            rsx! {
                MenuButton { entries: vec![Item::new(1, "Bold").into()], onpick: |_| {}, "Format" }
                Chips { choices: vec![Choice::new("Small"), Choice::new("Large")], on: vec![false, true], onpick: |_| {} }
                {layer.render()}
            }
        }
        let mut dom = dioxus::prelude::VirtualDom::new(Page);
        dom.rebuild_in_place();
        let html = dioxus_ssr::render(&dom);
        assert!(
            html.contains("ui-layer") && html.contains("ui-backdrop"),
            "{html}"
        );
        // The layer is displayed with nothing open; only what it holds hides.
        assert!(!html.contains("class=\"ui-layer\" style"), "{html}");
        assert!(html.contains("ui-chip on"), "{html}");
        // Every menu level is in the tree from the first frame.
        assert_eq!(
            html.matches("class=\"ui-menu\"").count(),
            layer::LEVELS,
            "{html}"
        );
    }
}
