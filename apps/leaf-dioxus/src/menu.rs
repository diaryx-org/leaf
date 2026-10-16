//! The menu bar: the app menu, File, Edit, Format, View and Window, as the
//! Swift editor's `EditorCommands.swift` lays them out — through muda, which
//! builds the native `NSMenu` on macOS.
//!
//! Every editor item *is* one of the editor's key bindings (leaf-dioxus's
//! `input.rs`). It is given as the chord it stands for; the menu shows the
//! chord as its shortcut, and choosing the item — with the mouse or by
//! pressing the shortcut, which macOS hands to the menu before the view ever
//! sees it — replays the chord through the same key handler. So a command is defined once, the menu cannot disagree with
//! the keyboard, and the find bar's fields still get ⌘V while they have focus.
//! Only what no key binding does — New, Open, Save As, the Table menu, the
//! context menu's link and footnote items — is an action of its own.
//!
//! The app menu's Quit, Hide, Services and the Window menu are AppKit's own
//! (`PredefinedMenuItem`): they act on the application, not the document.

use std::collections::HashMap;
use std::str::FromStr;

use dioxus::html::{Code, Key, Modifiers};
use leaf_core::Alignment;
use muda::accelerator::Accelerator;
use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};

use leaf_dioxus::{Context, Footnote, TableCommand};

/// What choosing a menu item does.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Press this chord: an editor command, as the keyboard has it.
    Chord(Key, Code, Modifiers),
    New,
    Open,
    SaveAs,
    /// A Table menu item; see [`TableCommand`].
    Table(TableCommand),
    /// The context menu's: follow or copy the link at the caret, follow the
    /// footnote there.
    OpenLink,
    CopyLink,
    FollowFootnote,
}

/// The menu bar, kept alive for as long as the app, and what its items do.
pub struct AppMenu {
    _menu: Menu,
    commands: HashMap<MenuId, Command>,
    /// The Table menu's items that act on the caret's table, and whether they
    /// are enabled now.
    table_items: Vec<MenuItem>,
    in_table: std::cell::Cell<bool>,
}

impl AppMenu {
    /// What an item chosen since the last call asked for, if anything — from
    /// the menu bar or from a context menu.
    pub fn next(&self) -> Option<Command> {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if let Some(c) = self
                .commands
                .get(&event.id)
                .cloned()
                .or_else(|| context_command(&event.id))
            {
                return Some(c);
            }
        }
        None
    }

    /// Enable the Table menu's row, column and alignment items only while the
    /// caret is in a table, as Swift's `.disabled(!editor.caretInTable)`.
    pub fn set_in_table(&self, yes: bool) {
        if self.in_table.replace(yes) != yes {
            for item in &self.table_items {
                item.set_enabled(yes);
            }
        }
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
    // Leaf ▸ Settings…, where a Mac app keeps it.
    let settings = b.item(
        "Settings…",
        Command::Chord(ch(","), Code::Comma, CMD),
        Some((CMD, Code::Comma)),
    );
    let _ = app.insert(&settings, 2);
    let _ = app.insert(&PredefinedMenuItem::separator(), 3);

    let file = Submenu::new("File", true);
    b.add(&file, "New", Command::New, Some((CMD, Code::KeyN)));
    b.add(&file, "Open…", Command::Open, Some((CMD, Code::KeyO)));
    sep(&file);
    b.chord(&file, "Save", ch("s"), Code::KeyS, CMD);
    b.add(
        &file,
        "Save As…",
        Command::SaveAs,
        Some((CMD | SHIFT, Code::KeyS)),
    );
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
    b.chord(
        &edit,
        "Paste and Match Style",
        ch("v"),
        Code::KeyV,
        CMD | SHIFT | ALT,
    );
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
        b.chord(
            &format,
            &format!("Heading {n}"),
            ch(&n.to_string()),
            code,
            CTRL,
        );
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
    b.chord(
        &format,
        "Move Block Down",
        Key::ArrowDown,
        Code::ArrowDown,
        ALT,
    );
    sep(&format);
    let _ = format.append(&table_menu(&mut b));

    let view = Submenu::new("View", true);
    b.chord(&view, "Source View", ch("e"), Code::KeyE, CMD);
    sep(&view);
    // Items of their own, not just keys: macOS hands the menu a chord it
    // names before the window sees it, which keeps Blitz's page zoom (on the
    // same three chords in blitz-shell's window.rs) from also acting.
    b.chord(&view, "Make Text Bigger", ch("="), Code::Equal, CMD);
    b.chord(&view, "Make Text Smaller", ch("-"), Code::Minus, CMD);
    b.chord(&view, "Actual Size", ch("0"), Code::Digit0, CMD);
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

    let menu =
        Menu::with_items(&[&app, &file, &edit, &format, &view, &window]).expect("the menu bar");
    menu.init_for_nsapp();
    window.set_as_windows_menu_for_nsapp();
    AppMenu {
        _menu: menu,
        commands: b.commands,
        table_items: b.table_items,
        in_table: true.into(),
    }
}

/// Format ▸ Table, as Swift's `TableRows` lays it out — none of it with a
/// shortcut, there or here.
#[cfg(target_os = "macos")]
fn table_menu(b: &mut Builder) -> Submenu {
    use TableCommand::*;
    let t = |c| Command::Table(c);
    let table = Submenu::new("Table", true);
    b.add(&table, "Insert Table", t(Insert { cols: 2 }), None);
    let with = Submenu::new("Insert Table With", true);
    for n in 3..=5 {
        b.add(&with, &format!("{n} Columns"), t(Insert { cols: n }), None);
    }
    let _ = table.append(&with);
    // Everything below acts on the caret's table.
    let grid = |b: &mut Builder, m: &Submenu, label: &str, c| {
        let item = b.add(m, label, t(c), None);
        b.table_items.push(item);
    };
    sep(&table);
    grid(b, &table, "Insert Row Above", InsertRow { below: false });
    grid(b, &table, "Insert Row Below", InsertRow { below: true });
    grid(b, &table, "Delete Row", DeleteRow);
    sep(&table);
    grid(
        b,
        &table,
        "Insert Column Left",
        InsertColumn { right: false },
    );
    grid(
        b,
        &table,
        "Insert Column Right",
        InsertColumn { right: true },
    );
    grid(b, &table, "Delete Column", DeleteColumn);
    sep(&table);
    let align = Submenu::new("Align Column", true);
    grid(b, &align, "Left", Align(Alignment::Left));
    grid(b, &align, "Center", Align(Alignment::Center));
    grid(b, &align, "Right", Align(Alignment::Right));
    grid(b, &align, "Default", Align(Alignment::Default));
    let _ = table.append(&align);
    sep(&table);
    grid(b, &table, "Move Row Up", MoveRow { down: false });
    grid(b, &table, "Move Row Down", MoveRow { down: true });
    grid(b, &table, "Move Column Left", MoveColumn { right: false });
    grid(b, &table, "Move Column Right", MoveColumn { right: true });
    table
}

/// The context menu's items. Their ids are fixed, so [`AppMenu::next`] knows
/// them without the menu that showed them: it is built afresh for each
/// right-click and gone by the time its choice is read.
const CONTEXT: &[(&str, &str)] = &[
    ("leaf.context.note", "Go to Note"),
    ("leaf.context.reference", "Back to Reference"),
    ("leaf.context.open-link", "Open Link"),
    ("leaf.context.copy-link", "Copy Link"),
    ("leaf.context.cut", "Cut"),
    ("leaf.context.copy", "Copy"),
    ("leaf.context.paste", "Paste"),
    ("leaf.context.paste-plain", "Paste and Match Style"),
    ("leaf.context.select-all", "Select All"),
];

fn context_command(id: &MenuId) -> Option<Command> {
    let ch = |c: &str, code, mods| Some(Command::Chord(Key::Character(c.into()), code, mods));
    match id.as_ref().strip_prefix("leaf.context.")? {
        "note" | "reference" => Some(Command::FollowFootnote),
        "open-link" => Some(Command::OpenLink),
        "copy-link" => Some(Command::CopyLink),
        // The clipboard items are the Edit menu's chords, replayed.
        "cut" => ch("x", Code::KeyX, CMD),
        "copy" => ch("c", Code::KeyC, CMD),
        "paste" => ch("v", Code::KeyV, CMD),
        "paste-plain" => ch("v", Code::KeyV, CMD | SHIFT | ALT),
        "select-all" => ch("a", Code::KeyA, CMD),
        _ => None,
    }
}

/// The ids, in order, that a right-click with `cx` offers — Swift's
/// `menu(for:)`: a footnote or a link first, then Cut and Copy when there is
/// a selection, then Paste, Paste and Match Style and Select All. `None` is
/// a separator.
fn context_items(cx: &Context) -> Vec<Option<&'static str>> {
    let mut ids = Vec::new();
    match cx.footnote {
        Some(Footnote::GoToNote) => ids.push(Some("leaf.context.note")),
        Some(Footnote::BackToReference) => ids.push(Some("leaf.context.reference")),
        None => {}
    }
    if cx.link {
        ids.extend([
            Some("leaf.context.open-link"),
            Some("leaf.context.copy-link"),
        ]);
    }
    if !ids.is_empty() {
        ids.push(None);
    }
    if cx.selection {
        ids.extend([Some("leaf.context.cut"), Some("leaf.context.copy")]);
    }
    ids.extend([
        Some("leaf.context.paste"),
        Some("leaf.context.paste-plain"),
        Some("leaf.context.select-all"),
    ]);
    ids
}

/// Pop up the context menu at the pointer, over the window's view.
///
/// Not now but on the main queue's next turn: the click that asks for it
/// arrives inside winit's event handler, and a context menu runs a nested
/// event loop that would hand winit its next event while that handler is
/// still borrowed — a panic ("tried to handle event while another event is
/// currently being handled"). From the main queue it runs as the menu bar's
/// tracking does, between events. Its choice arrives as a [`MenuEvent`], read
/// by [`AppMenu::next`].
#[cfg(target_os = "macos")]
pub fn show_context(window: &dyn dioxus_native::winit::window::Window, cx: &Context) {
    use muda::ContextMenu;
    use raw_window_handle::RawWindowHandle;
    let Ok(handle) = window.rwh_06_window_handle().window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };
    let view = h.ns_view.as_ptr() as usize;

    let menu = Menu::new();
    for id in context_items(cx) {
        match id {
            Some(id) => {
                let label = CONTEXT
                    .iter()
                    .find(|(i, _)| *i == id)
                    .map_or(id, |(_, l)| l);
                let _ = menu.append(&MenuItem::with_id(id, label, true, None));
            }
            None => {
                let _ = menu.append(&PredefinedMenuItem::separator());
            }
        }
    }
    /// The menu, moved to the main queue: `exec_async` asks for `Send`, and
    /// the main queue runs it on the thread it was built on.
    struct OnMain(Menu);
    unsafe impl Send for OnMain {}
    let menu = OnMain(menu);
    dispatch2::DispatchQueue::main().exec_async(move || {
        let menu = menu;
        // SAFETY: the window's view, which outlives the app's only window;
        // `None` pops the menu up at the pointer.
        unsafe { menu.0.show_context_menu_for_nsview(view as *const _, None) };
    });
}

#[cfg(not(target_os = "macos"))]
pub fn show_context(_window: &dyn dioxus_native::winit::window::Window, _cx: &Context) {}

/// No menu bar but macOS's yet: the keyboard has every command regardless.
#[cfg(not(target_os = "macos"))]
pub fn install() -> AppMenu {
    AppMenu {
        _menu: Menu::new(),
        commands: HashMap::new(),
        table_items: Vec::new(),
        in_table: true.into(),
    }
}

fn sep(m: &Submenu) {
    let _ = m.append(&PredefinedMenuItem::separator());
}

#[derive(Default)]
struct Builder {
    commands: HashMap<MenuId, Command>,
    table_items: Vec<MenuItem>,
}

impl Builder {
    /// An item that presses `key` with `mods`, shown with it as the shortcut.
    fn chord(&mut self, menu: &Submenu, label: &str, key: Key, code: Code, mods: Modifiers) {
        self.add(
            menu,
            label,
            Command::Chord(key, code, mods),
            Some((mods, code)),
        );
    }

    /// An item, appended to `menu` — and handed back, for a caller that
    /// enables or disables it later (the Table items outside a table).
    fn add(
        &mut self,
        menu: &Submenu,
        label: &str,
        command: Command,
        shortcut: Option<(Modifiers, Code)>,
    ) -> MenuItem {
        let item = self.item(label, command, shortcut);
        let _ = menu.append(&item);
        item
    }

    /// An item, for a menu to take where it likes.
    fn item(
        &mut self,
        label: &str,
        command: Command,
        shortcut: Option<(Modifiers, Code)>,
    ) -> MenuItem {
        let id = MenuId::new(format!("leaf.{}", self.commands.len()));
        let accel = shortcut.and_then(|(m, c)| accelerator(m, c));
        let item = MenuItem::with_id(id.clone(), label, true, accel);
        self.commands.insert(id, command);
        item
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
        assert_eq!(
            a,
            Accelerator::from_str("Shift+Cmd+Digit8").expect("a parse")
        );
    }

    #[test]
    fn a_context_menu_leads_with_the_link_and_offers_cut_only_with_a_selection() {
        let ids = |cx| {
            context_items(&cx)
                .into_iter()
                .map(|i| i.unwrap_or("-"))
                .collect::<Vec<_>>()
        };
        let plain = ids(Context {
            selection: false,
            link: false,
            footnote: None,
        });
        assert_eq!(
            plain,
            [
                "leaf.context.paste",
                "leaf.context.paste-plain",
                "leaf.context.select-all"
            ]
        );
        let link = ids(Context {
            selection: true,
            link: true,
            footnote: None,
        });
        assert_eq!(
            &link[..4],
            [
                "leaf.context.open-link",
                "leaf.context.copy-link",
                "-",
                "leaf.context.cut"
            ]
        );
        let note = ids(Context {
            selection: false,
            link: false,
            footnote: Some(Footnote::BackToReference),
        });
        assert_eq!(&note[..2], ["leaf.context.reference", "-"]);
    }

    #[test]
    fn every_context_item_has_a_label_and_a_command() {
        for (id, _) in CONTEXT {
            assert!(context_command(&MenuId::new(*id)).is_some(), "{id}");
        }
        assert_eq!(
            context_command(&MenuId::new("leaf.context.paste-plain")),
            Some(Command::Chord(
                Key::Character("v".into()),
                Code::KeyV,
                CMD | SHIFT | ALT
            ))
        );
    }
}
