//! What the terminal app can be asked to do, as one table: the command menu
//! (⌃P or F10) is built from it, each item showing the keys that do the same
//! thing, so the menu is the key reference too. leaf-tui's palette, menus
//! and key reference come from one table the same way.
//!
//! Most commands are the editor's own key bindings (leaf-dioxus's
//! `input.rs`), carried as the chord they stand for and replayed through
//! [`leaf_dioxus::Editor::key`] when chosen, as the Mac app's commands are:
//! a command is defined once, and no menu can disagree with the keyboard.

use dioxus::html::{Code, Key, Modifiers, NamedKey};
use unit::{Entry, Item};

use crate::term::PRIMARY;

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Press this chord: an editor command, as the keyboard has it.
    Chord(Key, Code, Modifiers),
    /// Ask for a link's destination, and make or change the link.
    Link,
    /// Ask for a path, and save the document there.
    SaveAs,
    /// Leave, asking first if there are unsaved changes.
    Quit,
}

const SHIFT: Modifiers = Modifiers::SHIFT;
const ALT: Modifiers = Modifiers::ALT;
/// On Apple the editor's paragraph chords are ⌃ with a digit, which no
/// terminal sends: they are the menu's alone.
const CTRL: Modifiers = Modifiers::CONTROL;

fn chord(c: &str, code: Code, mods: Modifiers) -> Command {
    Command::Chord(Key::Character(c.into()), code, mods)
}

/// The keys a terminal reader presses for `mods` and `key`: ⌃ for the
/// editor's primary modifier, which is the only one a terminal sends.
fn keys(mods: Modifiers, key: &str) -> String {
    let mut s = String::new();
    if mods.contains(ALT) {
        s.push('⌥');
    }
    if mods.contains(SHIFT) {
        s.push('⇧');
    }
    if mods.contains(PRIMARY) {
        s.push('⌃');
    }
    s.push_str(key);
    s
}

/// One line of the table: a label, the keys shown beside it, the command.
type Line = (&'static str, Option<String>, Command);

fn line(label: &'static str, c: &str, code: Code, mods: Modifiers) -> Line {
    let shown = (!mods.contains(CTRL) || PRIMARY == CTRL).then(|| keys(mods, &c.to_uppercase()));
    (label, shown, chord(c, code, mods))
}

/// The table: each submenu's title and its lines, `None` a separator.
pub fn table() -> Vec<(&'static str, Vec<Option<Line>>)> {
    let p = PRIMARY;
    vec![
        (
            "File",
            vec![
                Some(line("Save", "s", Code::KeyS, p)),
                Some(("Save As…", None, Command::SaveAs)),
                None,
                Some(("Quit", Some(keys(p, "Q")), Command::Quit)),
            ],
        ),
        (
            "Edit",
            vec![
                Some(line("Undo", "z", Code::KeyZ, p)),
                Some(line("Redo", "z", Code::KeyZ, p | SHIFT)),
                None,
                Some(line("Cut", "x", Code::KeyX, p)),
                Some(line("Copy", "c", Code::KeyC, p)),
                Some(line("Paste", "v", Code::KeyV, p)),
                Some(line("Paste as Plain Text", "v", Code::KeyV, p | SHIFT)),
                Some(line("Select All", "a", Code::KeyA, p)),
                None,
                Some(line("Find…", "f", Code::KeyF, p)),
                Some(line("Find and Replace…", "f", Code::KeyF, p | ALT)),
                Some(line("Find Next", "g", Code::KeyG, p)),
                Some(line("Find Previous", "g", Code::KeyG, p | SHIFT)),
            ],
        ),
        (
            "Format",
            vec![
                Some(line("Bold", "b", Code::KeyB, p)),
                Some(line("Italic", "i", Code::KeyI, p)),
                Some(line("Underline", "u", Code::KeyU, p)),
                Some(line("Code", "c", Code::KeyC, p | SHIFT)),
                Some(line("Highlight", "m", Code::KeyM, p | SHIFT)),
                Some(line("Strikethrough", "x", Code::KeyX, p | SHIFT)),
                None,
                Some(("Link…", None, Command::Link)),
            ],
        ),
        (
            "Paragraph",
            vec![
                Some(line("Body", "0", Code::Digit0, CTRL)),
                Some(line("Heading 1", "1", Code::Digit1, CTRL)),
                Some(line("Heading 2", "2", Code::Digit2, CTRL)),
                Some(line("Heading 3", "3", Code::Digit3, CTRL)),
                Some(line("Heading 4", "4", Code::Digit4, CTRL)),
                Some(line("Heading 5", "5", Code::Digit5, CTRL)),
                Some(line("Heading 6", "6", Code::Digit6, CTRL)),
                None,
                Some(line("Code Block", "c", Code::KeyC, p | ALT)),
                Some(line("Task", "l", Code::KeyL, p | SHIFT)),
                Some(line("Task Done", "u", Code::KeyU, p | SHIFT)),
                None,
                Some((
                    "Indent",
                    Some("Tab".into()),
                    Command::Chord(Key::Named(NamedKey::Tab), Code::Tab, Modifiers::empty()),
                )),
                Some((
                    "Outdent",
                    Some("⇧Tab".into()),
                    Command::Chord(Key::Named(NamedKey::Tab), Code::Tab, SHIFT),
                )),
            ],
        ),
        (
            "View",
            vec![Some(line("Source or WYSIWYG", "e", Code::KeyE, p))],
        ),
    ]
}

/// The command menu, and the command each item's id stands for.
pub fn menu() -> (Vec<Entry>, Vec<Command>) {
    let mut commands = Vec::new();
    let entries = table()
        .into_iter()
        .map(|(title, lines)| {
            let items = lines
                .into_iter()
                .map(|l| match l {
                    None => Entry::Separator,
                    Some((label, shown, c)) => {
                        commands.push(c);
                        let item = Item::new(commands.len() - 1, label);
                        match shown {
                            Some(k) => item.shortcut(k).into(),
                            None => item.into(),
                        }
                    }
                })
                .collect();
            Entry::submenu(title, items)
        })
        .collect();
    (entries, commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_has_its_command() {
        let (entries, commands) = menu();
        assert_eq!(entries.len(), table().len());
        let lines: usize = table()
            .iter()
            .map(|(_, l)| l.iter().flatten().count())
            .sum();
        assert_eq!(commands.len(), lines);
    }

    #[test]
    fn a_shortcut_reads_as_a_terminal_types_it() {
        assert_eq!(keys(PRIMARY | SHIFT, "Z"), "⇧⌃Z");
    }
}
