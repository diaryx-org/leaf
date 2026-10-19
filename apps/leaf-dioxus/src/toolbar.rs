//! A document window's toolbar: the formatting tools of the toolbar the iOS
//! app on an iPad and Diaryx show (LeafToolbar, in `packages/leaf-swift`),
//! in its order, then the display controls.
//!
//! Style ▾ · **B** *I* U · Format ▾ · Lists ▾ · Insert ▾ · Undo Redo … Source · Aa ▾
//!
//! A tool that is a key binding presses its chord; a tool with a ▾ opens
//! its menu from `menus.rs` in the page's overlay, ticked and enabled by the
//! same rules as the menu bar's. Where the platform's title bar is
//! transparent over the page, the band runs up behind it, so the window's
//! controls and the document's title sit on the toolbar's colour.

use std::rc::Rc;

use dioxus::html::{Code, Key, Modifiers};
use dioxus::prelude::*;
use leaf_core::InlineKind;
use leaf_dioxus::Editor;
use leaf_dioxus_ui::MenuButton;

use crate::app::{self, AppEvent};
use crate::commands::{CMD, SHIFT};
use crate::menus::{self, Node};
use crate::page::{Chrome, DocInit, TOOLBAR, chrome_height};

/// The toolbar's look, beside the editor's stylesheet: its colours are the
/// editor's chrome's, set for both appearances.
pub const CSS: &str = "
:root {
  --tool-on-bg: rgba(0, 0, 0, 0.10);
  --tool-hover-bg: rgba(0, 0, 0, 0.05);
}
@media (prefers-color-scheme: dark) {
  :root {
    --tool-on-bg: rgba(255, 255, 255, 0.16);
    --tool-hover-bg: rgba(255, 255, 255, 0.08);
  }
}
.toolbar { position: fixed; left: 0; right: 0; top: 0; z-index: 3; box-sizing: border-box;
           background: var(--chrome-bg); color: var(--chrome-text);
           border-bottom: 1px solid var(--field-border);
           font: 13px system-ui, -apple-system, sans-serif; white-space: nowrap; }
.toolbar .tools { position: absolute; left: 0; right: 0; bottom: 0; display: flex;
                  align-items: center; gap: 2px; padding: 0 10px; box-sizing: border-box; }
.toolbar .tool, .toolbar .ui-menu-button { height: 26px; line-height: 26px; min-width: 28px;
                 padding: 0 7px; box-sizing: border-box; border-radius: 6px; text-align: center; }
.toolbar .tool:hover, .toolbar .ui-menu-button:hover { background: var(--tool-hover-bg); }
.toolbar .tool.on { background: var(--tool-on-bg); }
.toolbar .tool.off { opacity: 0.35; }
.toolbar .icon { display: flex; align-items: center; justify-content: center; }
.toolbar .gap { width: 10px; }
.toolbar .grow { flex: 1; }
.toolbar .b { font-weight: 700; font-family: Georgia, serif; font-size: 15px; }
.toolbar .i { font-style: italic; font-family: Georgia, serif; font-size: 15px; }
.toolbar .u { text-decoration: underline; font-family: Georgia, serif; font-size: 15px; }
.toolbar .caret { font-size: 9px; padding-left: 4px; opacity: 0.7; }";

/// An arrow turning back to the left, and its mirror.
const UNDO: &str = "M5.5 3.5 L2.5 6.5 L5.5 9.5 M2.5 6.5 H10 A3.5 3.5 0 0 1 10 13.5 H7";
const REDO: &str = "M10.5 3.5 L13.5 6.5 L10.5 9.5 M13.5 6.5 H6 A3.5 3.5 0 0 0 6 13.5 H9";

/// The toolbar's markup.
pub fn render(ed: &Editor, chrome: &Chrome) -> Element {
    rsx! { Toolbar { ed: ed.clone(), chrome: chrome.clone() } }
}

#[derive(Props, Clone)]
struct ToolbarProps {
    ed: Editor,
    chrome: Chrome,
}

impl PartialEq for ToolbarProps {
    fn eq(&self, other: &Self) -> bool {
        self.chrome == other.chrome
    }
}

#[allow(non_snake_case)]
fn Toolbar(props: ToolbarProps) -> Element {
    let init = use_context::<DocInit>();
    let window = dioxus_native::use_window();
    let id = window.id();
    let c = &props.chrome;
    let shell = init.shell.clone();
    let ed = props.ed.clone();

    // A tool that presses a chord.
    let chord = |label: &'static str,
                 class: &'static str,
                 on: bool,
                 enabled: bool,
                 ch: &'static str,
                 code: Code,
                 mods: Modifiers| {
        let (ed, shell) = (ed.clone(), shell.clone());
        let class = format!(
            "tool {class}{}{}",
            if on { " on" } else { "" },
            if enabled { "" } else { " off" }
        );
        rsx! {
            span { class,
                onmousedown: move |e: MouseEvent| {
                    e.prevent_default();
                    e.stop_propagation();
                    if enabled {
                        shell.key(&ed, Key::Character(ch.into()), code, mods);
                    }
                },
                "{label}"
            }
        }
    };
    // A tool drawn as an icon, which presses a chord. The system's text faces
    // have no undo or redo arrows Blitz can find, so these are paths.
    let icon =
        |path: &'static str, enabled: bool, ch: &'static str, code: Code, mods: Modifiers| {
            let (ed, shell) = (ed.clone(), shell.clone());
            let class = if enabled {
                "tool icon"
            } else {
                "tool icon off"
            };
            rsx! {
                span { class,
                    onmousedown: move |e: MouseEvent| {
                        e.prevent_default();
                        e.stop_propagation();
                        if enabled {
                            shell.key(&ed, Key::Character(ch.into()), code, mods);
                        }
                    },
                    svg { width: "16", height: "16", view_box: "0 0 16 16",
                        path { d: path, fill: "none", stroke: "currentColor", stroke_width: "1.6",
                               stroke_linecap: "round", stroke_linejoin: "round" }
                    }
                }
            }
        };
    // A tool that opens one of `menus.rs`'s menus.
    let popup = |label: String, nodes: Vec<Node>| {
        let (entries, commands) = menus::entries(&nodes, Some(c), &app::app_state());
        let commands = Rc::new(commands);
        rsx! {
            MenuButton {
                entries,
                top: Some(chrome_height() + 2.0),
                height: 26.0,
                onpick: move |i: usize| {
                    if let Some(cmd) = commands.get(i).cloned() {
                        app::post(AppEvent::Command(id, cmd));
                    }
                },
                "{label}"
                span { class: "caret", "▾" }
            }
        }
    };
    let style = match (c.heading, c.code_block, c.blockquote, c.verse) {
        (n, ..) if n > 0 => format!("Heading {n}"),
        (_, true, ..) => "Code".into(),
        (_, _, true, _) => "Quote".into(),
        (.., true) => "Verse".into(),
        _ => "Body".into(),
    };
    let rich = !c.source_view;
    let m = |k| c.marks.contains(k);
    let height = chrome_height();
    rsx! {
        div { class: "toolbar", style: "height: {height}px",
            onmousedown: move |e: MouseEvent| e.prevent_default(),
            div { class: "tools", style: "height: {TOOLBAR}px",
                {popup(style, menus::style())}
                span { class: "gap" }
                {chord("B", "b", m(InlineKind::Strong), rich, "b", Code::KeyB, CMD)}
                {chord("I", "i", m(InlineKind::Emph), rich, "i", Code::KeyI, CMD)}
                {chord("U", "u", m(InlineKind::Insert), rich, "u", Code::KeyU, CMD)}
                {popup("Format".into(), menus::format())}
                span { class: "gap" }
                {popup("Lists".into(), menus::lists_menu())}
                {popup("Insert".into(), menus::insert())}
                span { class: "gap" }
                {icon(UNDO, c.can_undo, "z", Code::KeyZ, CMD)}
                {icon(REDO, c.can_redo, "z", Code::KeyZ, CMD | SHIFT)}
                span { class: "grow" }
                {chord("</>", "", c.source_view, true, "e", Code::KeyE, CMD)}
                {popup("Aa".into(), menus::display())}
            }
        }
    }
}

/// The commands a toolbar's chord stands for, for tests.
#[cfg(test)]
fn bold() -> crate::commands::Command {
    crate::commands::Command::chord("b", Code::KeyB, CMD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toolbar_is_drawn_in_colours_set_for_both_appearances() {
        let all = format!(
            "{}{CSS}",
            leaf_dioxus::stylesheet(&leaf_dioxus::Type::default())
        );
        leaf_dioxus::Palette::of(&all)
            .check()
            .expect("a whole palette");
    }

    #[test]
    fn the_bold_tool_is_the_menus_bold() {
        let (_, commands) = menus::entries(&menus::bar()[3].1, None, &Default::default());
        assert_eq!(commands[0], bold());
    }
}
