//! A key to a [`Doc`] edit, with the Swift editor's bindings.
//!
//! The keyboard `apps/leaf-editor` ships (its `LeafTextView` and the menus in
//! `EditorCommands.swift`), mapped onto the same `Doc` calls Swift makes
//! through leaf-ffi. Every edit is core's, so a binding here is a line, not a
//! feature: what a frontend owns is only which chord means what.
//!
//! It stays a *function of the key*, not of Dioxus: the component hands it
//! the key, the physical key and the modifiers, so it is testable without a
//! virtual DOM or an event. Digits and brackets match on the physical [`Code`]
//! because ⇧ changes their [`Key`] — ⇧⌘8 arrives as `*` — and the binding is
//! to the key, not the character.
//!
//! This is a native app, so it knows its platform: ⌘ is Meta on Apple and
//! Control elsewhere, and ⌃ is a modifier of its own only on Apple, where the
//! Swift editor's ⌃0–⌃6 and ⌃K live.

use dioxus::html::{Code, Key, Modifiers};
use leaf_core::{Alignment, BlockKind, Doc, InlineKind};

pub use crate::prefs::ZoomCommand;

/// What a key did, for the host to act on what it does not own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// The document changed, or the caret moved: repaint.
    Handled,
    /// Not ours — let Blitz have it.
    Ignored,
    /// The host owns this one: the clipboard and the file.
    Host(Host),
    /// A page of motion: the host knows how many rows a page is.
    Page { down: bool, extend: bool },
    /// Up or down one *visual* line. Core's `move_up` moves by its own rows,
    /// which unwrapped are whole blocks; only the layout knows where a
    /// wrapped line is, so the host moves by geometry, as Swift does.
    Vertical { down: bool, extend: bool },
    /// To the start or end of the visual line — Home/End and ⌘←/→ — which
    /// again only the layout knows.
    LineEdge { end: bool, extend: bool },
    /// The find bar: open it (⌘F, or ⌥⌘F with replace), or step through its
    /// matches (⌘G, ⇧⌘G).
    Find(FindCommand),
    /// The text zoom: ⌘+ (or ⌘=), ⌘−, ⌘0.
    Zoom(ZoomCommand),
    /// ⌘, — the settings panel.
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FindCommand {
    Open { replace: bool },
    Next,
    Previous,
}

/// The gestures a document cannot complete by itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Host {
    Copy,
    Cut,
    Paste,
    /// ⇧⌘V and ⌥⇧⌘V: the clipboard's text, never its HTML.
    PastePlain,
    Save,
    /// ⌃⌘Space: the emoji and symbols picker, which AppKit hangs on an Edit
    /// menu item this app has no menu bar to hold.
    CharacterPalette,
}

/// The Table menu's items. The Swift editor gives none of them a shortcut,
/// so the menu sends each as itself rather than as a chord — but the edit is
/// still made here, beside the keys, where it can be tested.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TableCommand {
    /// A table of [`TABLE_ROWS`] body rows and `cols` columns at the caret.
    Insert { cols: usize },
    InsertRow { below: bool },
    DeleteRow,
    InsertColumn { right: bool },
    DeleteColumn,
    Align(Alignment),
    MoveRow { down: bool },
    MoveColumn { right: bool },
}

/// Body rows under the header of an inserted table — Swift's
/// `TableRows.defaultRows`.
pub const TABLE_ROWS: usize = 2;

/// Apply a Table menu item. Every item but Insert acts on the caret's table,
/// and outside one is [`Action::Ignored`] — the menu disables them there, but
/// a menu's state can lag the caret by a frame.
pub fn apply_table(doc: &mut Doc, cmd: TableCommand) -> Action {
    use TableCommand::*;
    if !matches!(cmd, Insert { .. }) && !doc.caret_in_table() {
        return Action::Ignored;
    }
    match cmd {
        Insert { cols } => doc.insert_table(TABLE_ROWS, cols),
        InsertRow { below } => doc.table_insert_row(below),
        DeleteRow => doc.table_delete_row(),
        InsertColumn { right } => doc.table_insert_column(right),
        DeleteColumn => doc.table_delete_column(),
        Align(a) => doc.table_set_alignment(a),
        MoveRow { down } => doc.table_move_row(down),
        MoveColumn { right } => doc.table_move_column(right),
    }
    Action::Handled
}

/// ⌘ arrives from winit as `SUPER` — not `META`, which `keyboard-types`
/// keeps for the legacy Meta key. Matching `META` alone, every ⌘ chord fell
/// through to the plain keys: ⌘I typed an `i`, ⌘↓ moved one row.
#[cfg(target_os = "macos")]
const CMD: Modifiers = Modifiers::SUPER;
#[cfg(not(target_os = "macos"))]
const CMD: Modifiers = Modifiers::CONTROL;

/// Apply one key press to the document.
///
/// `shift` extends the selection on every motion, which is why each motion
/// passes it rather than branching on it: core's caret API takes `extend` for
/// exactly this.
#[expect(clippy::unit_arg, reason = "`done(doc.edit())` reads as one step: do it, and it was handled")]
pub fn apply_key(doc: &mut Doc, key: &Key, code: Code, mods: Modifiers) -> Action {
    let shift = mods.contains(Modifiers::SHIFT);
    let alt = mods.contains(Modifiers::ALT);
    let cmd = mods.contains(CMD) || (cfg!(target_os = "macos") && mods.contains(Modifiers::META));
    let ctrl = cfg!(target_os = "macos") && mods.contains(Modifiers::CONTROL);

    if cmd && ctrl && code == Code::Space {
        return Action::Host(Host::CharacterPalette);
    }
    if cmd {
        return command(doc, key, code, shift, alt);
    }
    if ctrl {
        return control(doc, key, code, shift);
    }

    let done = |_: ()| Action::Handled;
    match key {
        // A printable key. `Key::Character` is already the composed result of
        // the layout and the modifiers — but an IME's composition never
        // arrives this way; see README.md.
        Key::Character(text) => done(doc.insert(text)),
        // Inside a table Return moves down a cell and ⇧Return breaks the line
        // in it; elsewhere both are a newline. Core answers whether it was a
        // table's, as it does for Swift.
        Key::Enter if shift => {
            if !doc.cell_line_break() {
                doc.newline();
            }
            Action::Handled
        }
        Key::Enter => {
            if !doc.cell_return() {
                doc.newline();
            }
            Action::Handled
        }
        Key::Tab => {
            if !doc.cell_tab(!shift) {
                if shift {
                    doc.outdent();
                } else {
                    doc.indent();
                }
            }
            Action::Handled
        }
        Key::Backspace if alt => done(doc.delete_word_back()),
        Key::Backspace => done(doc.backspace()),
        Key::Delete if alt => done(doc.delete_word_forward()),
        Key::Delete => done(doc.delete_forward()),
        Key::ArrowLeft if alt => done(doc.move_word_left(shift)),
        Key::ArrowRight if alt => done(doc.move_word_right(shift)),
        Key::ArrowUp if alt => done(doc.move_block_up()),
        Key::ArrowDown if alt => done(doc.move_block_down()),
        Key::ArrowLeft => done(doc.move_left(shift)),
        Key::ArrowRight => done(doc.move_right(shift)),
        Key::ArrowUp => Action::Vertical { down: false, extend: shift },
        Key::ArrowDown => Action::Vertical { down: true, extend: shift },
        Key::Home => Action::LineEdge { end: false, extend: shift },
        Key::End => Action::LineEdge { end: true, extend: shift },
        Key::PageUp => Action::Page { down: false, extend: shift },
        Key::PageDown => Action::Page { down: true, extend: shift },
        _ => Action::Ignored,
    }
}

/// ⌘ chords: the Edit and Format menus.
#[expect(clippy::unit_arg, reason = "`done(doc.edit())` reads as one step: do it, and it was handled")]
fn command(doc: &mut Doc, key: &Key, code: Code, shift: bool, alt: bool) -> Action {
    let done = |_: ()| Action::Handled;
    // The physical keys first, which ⇧ would otherwise rename.
    match code {
        Code::Digit8 if shift => return done(doc.toggle_list(false)),
        Code::Digit7 if shift => return done(doc.toggle_list(true)),
        Code::Digit9 if shift => return done(doc.toggle_blockquote()),
        Code::BracketRight if !shift => return done(doc.indent()),
        Code::BracketLeft if !shift => return done(doc.outdent()),
        // ⌘= and ⇧⌘= alike, as a Mac app takes ⌘+ on a layout where + is
        // shifted.
        Code::Equal if !alt => return Action::Zoom(ZoomCommand::In),
        Code::Minus if !alt && !shift => return Action::Zoom(ZoomCommand::Out),
        Code::Digit0 if !alt && !shift => return Action::Zoom(ZoomCommand::Reset),
        Code::Comma if !alt && !shift => return Action::Settings,
        _ => {}
    }
    match key {
        Key::ArrowUp => return done(doc.move_doc_start(shift)),
        Key::ArrowDown => return done(doc.move_doc_end(shift)),
        // Ends of the visual line, by geometry, as Swift does.
        Key::ArrowLeft => return Action::LineEdge { end: false, extend: shift },
        Key::ArrowRight => return Action::LineEdge { end: true, extend: shift },
        Key::Backspace => return done(doc.delete_to_line_start()),
        _ => {}
    }
    match code {
        Code::KeyF => return Action::Find(FindCommand::Open { replace: alt }),
        Code::KeyG if !alt => {
            return Action::Find(if shift { FindCommand::Previous } else { FindCommand::Next });
        }
        _ => {}
    }
    let Key::Character(c) = key else {
        return Action::Ignored;
    };
    // ⌥ changes the character a key types on a Mac — ⌥⌘C arrives as `ç` —
    // so a chord with ⌥ is matched by the key's position instead.
    let letter = match alt {
        true => letter_of(code).map(String::from).unwrap_or_default(),
        false => c.to_lowercase(),
    };
    match (letter.as_str(), shift, alt) {
        ("c", false, false) => Action::Host(Host::Copy),
        ("x", false, false) => Action::Host(Host::Cut),
        ("v", false, false) => Action::Host(Host::Paste),
        ("v", true, _) => Action::Host(Host::PastePlain),
        ("s", false, false) => Action::Host(Host::Save),
        ("a", false, false) => done(doc.select_all()),
        // The source view and back. Core keeps the caret on the same text.
        ("e", false, false) => done(doc.toggle_view()),
        // Undo and redo answer whether there was a step to take; either way
        // the frame is current, so either way is a repaint.
        ("z", false, false) => {
            doc.undo();
            Action::Handled
        }
        ("z", true, false) => {
            doc.redo();
            Action::Handled
        }
        // Marks.
        ("b", false, false) => done(doc.toggle(InlineKind::Strong)),
        ("i", false, false) => done(doc.toggle(InlineKind::Emph)),
        ("u", false, false) => done(doc.toggle(InlineKind::Insert)),
        ("c", true, false) => done(doc.toggle(InlineKind::Verbatim)),
        ("m", true, false) => done(doc.toggle(InlineKind::Mark)),
        ("x", true, false) => done(doc.toggle(InlineKind::Delete)),
        // Blocks.
        ("c", false, true) => done(doc.toggle_code_block()),
        ("l", true, false) => done(doc.toggle_task_item()),
        ("u", true, false) => done(doc.toggle_task_checked()),
        _ => Action::Ignored,
    }
}

/// The letter a physical key is named for: `KeyC` → `c`.
fn letter_of(code: Code) -> Option<char> {
    let name = format!("{code:?}");
    let rest = name.strip_prefix("Key")?;
    let mut chars = rest.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

/// ⌃ chords, on Apple only: the paragraph style menu and the Emacs keys
/// Cocoa text views honour. ⌃A and ⌃E are the *paragraph's* ends, which in an
/// unwrapped row are core's own `move_home`/`move_end`; ⌃N and ⌃P are visual
/// lines, like the arrows.
#[expect(clippy::unit_arg, reason = "`done(doc.edit())` reads as one step: do it, and it was handled")]
fn control(doc: &mut Doc, key: &Key, code: Code, shift: bool) -> Action {
    let done = |_: ()| Action::Handled;
    let level = match code {
        Code::Digit0 => Some(0),
        Code::Digit1 => Some(1),
        Code::Digit2 => Some(2),
        Code::Digit3 => Some(3),
        Code::Digit4 => Some(4),
        Code::Digit5 => Some(5),
        Code::Digit6 => Some(6),
        _ => None,
    };
    match level {
        Some(0) => return done(doc.set_block(BlockKind::Paragraph)),
        Some(n) => return done(doc.toggle_heading(n)),
        None => {}
    }
    let Key::Character(c) = key else {
        return Action::Ignored;
    };
    match c.to_lowercase().as_str() {
        "a" => done(doc.move_home(shift)),
        "e" => done(doc.move_end(shift)),
        "k" => done(doc.delete_to_line_end()),
        "f" => done(doc.move_right(shift)),
        "b" => done(doc.move_left(shift)),
        "n" => Action::Vertical { down: true, extend: shift },
        "p" => Action::Vertical { down: false, extend: shift },
        "d" => done(doc.delete_forward()),
        "h" => done(doc.backspace()),
        _ => Action::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use leaf_core::Format;

    use super::*;

    fn doc(src: &str) -> Doc {
        let mut d = Doc::from_source(src.to_string(), Format::Markdown).expect("a document");
        d.build_visual(80);
        d
    }

    fn press(d: &mut Doc, key: Key, code: Code, mods: Modifiers) -> Action {
        let a = apply_key(d, &key, code, mods);
        d.build_visual(80);
        a
    }

    fn ch(c: &str) -> Key {
        Key::Character(c.into())
    }

    #[test]
    fn cmd_b_bolds_the_selection() {
        let mut d = doc("hello\n");
        d.select_range(0, 5);
        press(&mut d, ch("b"), Code::KeyB, CMD);
        assert!(d.source.starts_with("**hello**"), "{:?}", d.source);
    }

    #[test]
    fn shift_cmd_8_is_a_bullet_list_though_its_key_is_a_star() {
        let mut d = doc("item\n");
        d.place_caret(1, false);
        press(&mut d, ch("*"), Code::Digit8, CMD | Modifiers::SHIFT);
        assert!(d.source.starts_with("- item") || d.source.starts_with("* item"), "{:?}", d.source);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ctrl_2_toggles_a_heading() {
        let mut d = doc("title\n");
        d.place_caret(1, false);
        press(&mut d, ch("2"), Code::Digit2, Modifiers::CONTROL);
        assert!(d.source.starts_with("## title"), "{:?}", d.source);
        press(&mut d, ch("2"), Code::Digit2, Modifiers::CONTROL);
        assert!(d.source.starts_with("title"), "{:?}", d.source);
    }

    #[test]
    fn the_clipboard_is_the_hosts() {
        let mut d = doc("hello\n");
        for (c, want) in [("c", Host::Copy), ("x", Host::Cut), ("v", Host::Paste)] {
            assert_eq!(
                apply_key(&mut d, &ch(c), Code::Unidentified, CMD),
                Action::Host(want)
            );
        }
        assert_eq!(
            apply_key(&mut d, &ch("v"), Code::KeyV, CMD | Modifiers::SHIFT),
            Action::Host(Host::PastePlain)
        );
    }

    #[test]
    fn alt_backspace_deletes_a_word() {
        let mut d = doc("one two\n");
        d.place_caret(7, false);
        press(&mut d, Key::Backspace, Code::Backspace, Modifiers::ALT);
        assert!(d.source.starts_with("one \n") || d.source.starts_with("one\n"), "{:?}", d.source);
    }

    /// What winit hands over for ⌘ on a Mac, which is what broke: a chord
    /// must never be typed as its letter.
    #[cfg(target_os = "macos")]
    #[test]
    fn cmd_is_super_and_a_cmd_chord_types_nothing() {
        let mut d = doc("hello\n");
        d.place_caret(0, false);
        press(&mut d, ch("i"), Code::KeyI, Modifiers::SUPER);
        assert_eq!(d.source, "hello\n");
        press(&mut d, Key::ArrowDown, Code::ArrowDown, Modifiers::SUPER);
        assert_eq!(d.caret, d.source.len());
    }

    #[test]
    fn an_option_chord_matches_by_key_not_by_character() {
        let mut d = doc("x\n");
        d.place_caret(0, false);
        // ⌥⌘C types `ç`; it is still the code-block chord.
        press(&mut d, ch("ç"), Code::KeyC, CMD | Modifiers::ALT);
        assert!(d.source.starts_with("```"), "{:?}", d.source);
        assert_eq!(
            apply_key(&mut d, &ch("ƒ"), Code::KeyF, CMD | Modifiers::ALT),
            Action::Find(FindCommand::Open { replace: true })
        );
    }

    const GRID: &str = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n";

    /// A document holding [`GRID`], the caret on the cell holding `cell`.
    fn grid_at(cell: &str) -> Doc {
        let mut d = doc(GRID);
        let at = d.source.find(cell).expect("the cell");
        d.place_caret(at, false);
        d
    }

    fn table(d: &mut Doc, cmd: TableCommand) -> Action {
        let a = apply_table(d, cmd);
        d.build_visual(80);
        a
    }

    #[test]
    fn insert_table_writes_a_grid_and_lands_in_it() {
        let mut d = doc("x\n");
        d.place_caret(1, false);
        assert_eq!(table(&mut d, TableCommand::Insert { cols: 3 }), Action::Handled);
        let head = d.source.lines().find(|l| l.starts_with('|')).expect("a table row");
        assert_eq!(head.matches('|').count(), 4, "{:?}", d.source);
        let rows = d.source.lines().filter(|l| l.starts_with('|')).count();
        assert_eq!(rows, 2 + TABLE_ROWS, "head, rule and body: {:?}", d.source);
        assert!(d.caret_in_table());
    }

    #[test]
    fn row_commands_act_on_the_carets_row() {
        let mut d = grid_at("1");
        table(&mut d, TableCommand::InsertRow { below: true });
        assert_eq!(d.source.lines().count(), 5, "{:?}", d.source);
        let mut d = grid_at("1");
        table(&mut d, TableCommand::DeleteRow);
        assert!(!d.source.contains('1') && d.source.contains('3'), "{:?}", d.source);
        let mut d = grid_at("1");
        table(&mut d, TableCommand::MoveRow { down: true });
        assert!(d.source.find('3') < d.source.find('1'), "{:?}", d.source);
    }

    #[test]
    fn column_commands_act_on_the_carets_column() {
        let mut d = grid_at("1");
        table(&mut d, TableCommand::InsertColumn { right: true });
        assert_eq!(d.source.lines().next().map(|l| l.matches('|').count()), Some(4), "{:?}", d.source);
        let mut d = grid_at("1");
        table(&mut d, TableCommand::DeleteColumn);
        assert!(!d.source.contains('a') && d.source.contains('b'), "{:?}", d.source);
        let mut d = grid_at("1");
        table(&mut d, TableCommand::MoveColumn { right: true });
        let head = d.source.lines().next().unwrap_or_default();
        assert!(head.find('b') < head.find('a'), "{:?}", d.source);
    }

    #[test]
    fn align_sets_the_carets_column_rule() {
        let mut d = grid_at("2");
        table(&mut d, TableCommand::Align(Alignment::Right));
        let rule = d.source.lines().nth(1).unwrap_or_default();
        let cells: Vec<&str> = rule.split('|').map(str::trim).filter(|c| !c.is_empty()).collect();
        assert_eq!(cells, ["---", "---:"], "only the caret's column: {:?}", d.source);
    }

    #[test]
    fn table_commands_outside_a_table_do_nothing() {
        let mut d = doc("para\n\n| a |\n|---|\n| 1 |\n");
        d.place_caret(1, false);
        let before = d.source.clone();
        for cmd in [
            TableCommand::InsertRow { below: true },
            TableCommand::DeleteColumn,
            TableCommand::Align(Alignment::Center),
            TableCommand::MoveRow { down: true },
        ] {
            assert_eq!(table(&mut d, cmd), Action::Ignored);
        }
        assert_eq!(d.source, before);
    }

    #[test]
    fn an_unmapped_key_is_left_alone() {
        let mut d = doc("hello\n");
        assert_eq!(
            apply_key(&mut d, &Key::F1, Code::F1, Modifiers::empty()),
            Action::Ignored
        );
    }

    #[test]
    fn the_zoom_and_settings_chords_are_the_hosts_and_type_nothing() {
        let mut d = doc("hello\n");
        let before = d.source.clone();
        let z = |a| Action::Zoom(a);
        assert_eq!(press(&mut d, ch("="), Code::Equal, CMD), z(ZoomCommand::In));
        assert_eq!(press(&mut d, ch("+"), Code::Equal, CMD | Modifiers::SHIFT), z(ZoomCommand::In));
        assert_eq!(press(&mut d, ch("-"), Code::Minus, CMD), z(ZoomCommand::Out));
        assert_eq!(press(&mut d, ch("0"), Code::Digit0, CMD), z(ZoomCommand::Reset));
        assert_eq!(press(&mut d, ch(","), Code::Comma, CMD), Action::Settings);
        assert_eq!(d.source, before);
    }
}
