//! The menu bar: the app menu, File, Edit, Format, View and Window, as the
//! Swift editor's `EditorCommands.swift` lays them out — through muda, which
//! builds the native `NSMenu` on macOS.
//!
//! Every editor item *is* one of `input.rs`'s key bindings. It is given as the
//! chord it stands for; the menu shows the chord as its shortcut, and choosing
//! the item — with the mouse or by pressing the shortcut, which macOS hands to
//! the menu before the view ever sees it — replays the chord through the same
//! key handler. So a command is defined once, the menu cannot disagree with
//! the keyboard, and the find bar's fields still get ⌘V while they have focus.
//! Only what no key binding does — New, Open, Save As — is an action of its
//! own.
//!
//! The app menu's Quit, Hide, Services and the Window menu are AppKit's own
//! (`PredefinedMenuItem`): they act on the application, not the document.

use std::collections::HashMap;
use std::str::FromStr;

use dioxus::html::{Code, Key, Modifiers};
use muda::accelerator::Accelerator;
use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};

/// What choosing a menu item does.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Press this chord: an editor command, as the keyboard has it.
    Chord(Key, Code, Modifiers),
    New,
    Open,
    SaveAs,
}

/// The menu bar, kept alive for as long as the app, and what its items do.
pub struct AppMenu {
    _menu: Menu,
    commands: HashMap<MenuId, Command>,
}

impl AppMenu {
    /// What an item chosen since the last call asked for, if anything.
    pub fn next(&self) -> Option<Command> {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if let Some(c) = self.commands.get(&event.id) {
                return Some(c.clone());
            }
        }
        None
    }
}

/// ⌘ in Dioxus's terms (`SUPER`, as winit reports it).
const CMD: Modifiers = Modifiers::SUPER;
const SHIFT: Modifiers = Modifiers::SHIFT;
const ALT: Modifiers = Modifiers::ALT;
const CTRL: Modifiers = Modifiers::CONTROL;

/// Build the menu bar and install it as the application's.
#[cfg(target_os = "macos")]
pub fn install() -> AppMenu {
    let mut b = Builder::default();
    let ch = |c: &str| Key::Character(c.into());

    let app = Submenu::with_items(
        "Leaf",
        true,
        &[
            &PredefinedMenuItem::about(None, None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::services(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::quit(None),
        ],
    )
    .expect("the app menu");

    let file = Submenu::new("File", true);
    b.add(&file, "New", Command::New, Some((CMD, Code::KeyN)));
    b.add(&file, "Open…", Command::Open, Some((CMD, Code::KeyO)));
    sep(&file);
    b.chord(&file, "Save", ch("s"), Code::KeyS, CMD);
    b.add(&file, "Save As…", Command::SaveAs, Some((CMD | SHIFT, Code::KeyS)));
    sep(&file);
    let _ = file.append(&PredefinedMenuItem::close_window(None));

    // Titled "Edit" so AppKit adds its own Emoji & Symbols and Start
    // Dictation items to it.
    let edit = Submenu::new("Edit", true);
    b.chord(&edit, "Undo", ch("z"), Code::KeyZ, CMD);
    b.chord(&edit, "Redo", ch("z"), Code::KeyZ, CMD | SHIFT);
    sep(&edit);
    b.chord(&edit, "Cut", ch("x"), Code::KeyX, CMD);
    b.chord(&edit, "Copy", ch("c"), Code::KeyC, CMD);
    b.chord(&edit, "Paste", ch("v"), Code::KeyV, CMD);
    b.chord(&edit, "Paste and Match Style", ch("v"), Code::KeyV, CMD | SHIFT | ALT);
    b.chord(&edit, "Select All", ch("a"), Code::KeyA, CMD);
    sep(&edit);
    let find = Submenu::new("Find", true);
    b.chord(&find, "Find…", ch("f"), Code::KeyF, CMD);
    b.chord(&find, "Find and Replace…", ch("ƒ"), Code::KeyF, CMD | ALT);
    b.chord(&find, "Find Next", ch("g"), Code::KeyG, CMD);
    b.chord(&find, "Find Previous", ch("g"), Code::KeyG, CMD | SHIFT);
    let _ = edit.append(&find);

    let format = Submenu::new("Format", true);
    b.chord(&format, "Bold", ch("b"), Code::KeyB, CMD);
    b.chord(&format, "Italic", ch("i"), Code::KeyI, CMD);
    b.chord(&format, "Underline", ch("u"), Code::KeyU, CMD);
    b.chord(&format, "Strikethrough", ch("x"), Code::KeyX, CMD | SHIFT);
    b.chord(&format, "Code", ch("c"), Code::KeyC, CMD | SHIFT);
    b.chord(&format, "Highlight", ch("m"), Code::KeyM, CMD | SHIFT);
    sep(&format);
    b.chord(&format, "Paragraph", ch("0"), Code::Digit0, CTRL);
    for (n, code) in [
        (1, Code::Digit1),
        (2, Code::Digit2),
        (3, Code::Digit3),
        (4, Code::Digit4),
        (5, Code::Digit5),
        (6, Code::Digit6),
    ] {
        b.chord(&format, &format!("Heading {n}"), ch(&n.to_string()), code, CTRL);
    }
    sep(&format);
    b.chord(&format, "Bulleted List", ch("*"), Code::Digit8, CMD | SHIFT);
    b.chord(&format, "Numbered List", ch("&"), Code::Digit7, CMD | SHIFT);
    b.chord(&format, "Checklist", ch("l"), Code::KeyL, CMD | SHIFT);
    b.chord(&format, "Toggle Checked", ch("u"), Code::KeyU, CMD | SHIFT);
    b.chord(&format, "Quote", ch("("), Code::Digit9, CMD | SHIFT);
    b.chord(&format, "Code Block", ch("ç"), Code::KeyC, CMD | ALT);
    sep(&format);
    b.chord(&format, "Indent", ch("]"), Code::BracketRight, CMD);
    b.chord(&format, "Outdent", ch("["), Code::BracketLeft, CMD);
    sep(&format);
    b.chord(&format, "Move Block Up", Key::ArrowUp, Code::ArrowUp, ALT);
    b.chord(&format, "Move Block Down", Key::ArrowDown, Code::ArrowDown, ALT);

    let view = Submenu::new("View", true);
    b.chord(&view, "Source View", ch("e"), Code::KeyE, CMD);
    sep(&view);
    let _ = view.append(&PredefinedMenuItem::fullscreen(None));

    let window = Submenu::with_items(
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(None),
            &PredefinedMenuItem::maximize(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::bring_all_to_front(None),
        ],
    )
    .expect("the window menu");

    let menu = Menu::with_items(&[&app, &file, &edit, &format, &view, &window]).expect("the menu bar");
    menu.init_for_nsapp();
    window.set_as_windows_menu_for_nsapp();
    AppMenu { _menu: menu, commands: b.commands }
}

/// No menu bar but macOS's yet: the keyboard has every command regardless.
#[cfg(not(target_os = "macos"))]
pub fn install() -> AppMenu {
    AppMenu { _menu: Menu::new(), commands: HashMap::new() }
}

fn sep(m: &Submenu) {
    let _ = m.append(&PredefinedMenuItem::separator());
}

#[derive(Default)]
struct Builder {
    commands: HashMap<MenuId, Command>,
}

impl Builder {
    /// An item that presses `key` with `mods`, shown with it as the shortcut.
    fn chord(&mut self, menu: &Submenu, label: &str, key: Key, code: Code, mods: Modifiers) {
        self.add(menu, label, Command::Chord(key, code, mods), Some((mods, code)));
    }

    fn add(&mut self, menu: &Submenu, label: &str, command: Command, shortcut: Option<(Modifiers, Code)>) {
        let id = MenuId::new(format!("leaf.{}", self.commands.len()));
        let accel = shortcut.and_then(|(m, c)| accelerator(m, c));
        let item = MenuItem::with_id(id.clone(), label, true, accel);
        let _ = menu.append(&item);
        self.commands.insert(id, command);
    }
}

/// A chord as muda's accelerator. muda takes `keyboard-types` 0.8 and Dioxus
/// 0.7, whose key codes share their names — the same conversion `keys.rs`
/// makes from winit — and muda calls ⌘ `META`.
fn accelerator(mods: Modifiers, code: Code) -> Option<Accelerator> {
    use muda::accelerator::{Code as MCode, Modifiers as MMods};
    let code = MCode::from_str(&format!("{code:?}")).ok()?;
    let mut m = MMods::empty();
    m.set(MMods::META, mods.contains(Modifiers::SUPER));
    m.set(MMods::SHIFT, mods.contains(Modifiers::SHIFT));
    m.set(MMods::ALT, mods.contains(Modifiers::ALT));
    m.set(MMods::CONTROL, mods.contains(Modifiers::CONTROL));
    Some(Accelerator::new(m, code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chord_becomes_its_menu_shortcut() {
        let a = accelerator(CMD | SHIFT, Code::Digit8).expect("an accelerator");
        assert_eq!(a, Accelerator::from_str("Shift+Cmd+Digit8").expect("a parse"));
    }
}
