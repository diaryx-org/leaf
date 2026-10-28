//! What the app can be asked to do: every [`Command`] a menu, the toolbar
//! or the context menu carries, its shortcut, and whether it can be done —
//! and is in force — for a document as its [`Chrome`] stands.
//!
//! Most commands are the editor's own key bindings (leaf-dioxus's
//! `input.rs`), carried as the chord they stand for and replayed through
//! the editor's key handler when chosen: a command is defined once, and no
//! menu can disagree with the keyboard. The rest are what no key does.

use dioxus::html::{Code, Key, Modifiers, NamedKey};
use leaf_core::{Format, InlineKind, MarkColor};
use leaf_dioxus::{TableCommand, ZoomCommand};

use crate::page::Chrome;
use crate::settings::{ColumnWidth, TextSize};

/// ⌘ in Dioxus's terms (`META`, as Blitz reports it); off the Mac, Ctrl,
/// as the editor reads it (leaf-dioxus's `input.rs`).
#[cfg(target_os = "macos")]
pub const CMD: Modifiers = Modifiers::META;
#[cfg(not(target_os = "macos"))]
pub const CMD: Modifiers = Modifiers::CONTROL;
pub const SHIFT: Modifiers = Modifiers::SHIFT;
pub const ALT: Modifiers = Modifiers::ALT;
pub const CTRL: Modifiers = Modifiers::CONTROL;

/// The spelling switches, the system's where it has them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    WhileTyping,
    Correct,
    Quotes,
    Dashes,
    Replacement,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Press this chord: an editor command, as the keyboard has it.
    Chord(Key, Code, Modifiers),
    Table(TableCommand),
    /// Highlight in a colour, or with the plain highlighter.
    Highlight(Option<MarkColor>),
    /// Ask for a link's destination.
    Link,
    OpenLink,
    CopyLink,
    FollowFootnote,
    InsertFootnote,
    InsertRule,
    InsertPageBreak,
    Verse,
    PreserveBreaks,
    Zoom(ZoomCommand),
    ExportPdf,
    /// Show or hide the PDF preview beside the editor.
    PdfPreview,
    /// Show the page view, where the document is edited on its pages, in
    /// the editor's place; or the editor again.
    PageView,
    /// Save, Save As and Close: on the Mac, the document system's own menu
    /// items; elsewhere, the app's.
    Save,
    SaveAs,
    Close,
    CheckDocument,
    /// A spelling guess: the UTF-16 range of the system's text, the word it
    /// held, and what to put there.
    Replace(usize, usize, String, String),
    IgnoreSpelling(String),
    LearnSpelling(String),
    /// Look Up a UTF-16 range of the system's text.
    LookUp(usize, usize),
    // The app's, whichever window is in front.
    Column(ColumnWidth),
    Size(TextSize),
    New(Format),
    /// Open and Quit: the Mac's are the system's, as Save is.
    Open,
    Quit,
    OpenSample,
    Settings,
    Switch(Switch),
    ShowSubstitutions,
}

impl Command {
    /// A chord of a character key.
    pub fn chord(ch: &str, code: Code, mods: Modifiers) -> Self {
        Command::Chord(Key::Character(ch.into()), code, mods)
    }

    /// A chord of a named key: an arrow.
    pub fn named(key: NamedKey, code: Code, mods: Modifiers) -> Self {
        Command::Chord(Key::Named(key), code, mods)
    }

    /// Whether the app carries it out rather than a window's editor.
    pub fn is_app(&self) -> bool {
        matches!(
            self,
            Command::New(_)
                | Command::Open
                | Command::Quit
                | Command::Column(_)
                | Command::Size(_)
                | Command::OpenSample
                | Command::Settings
                | Command::Switch(_)
                | Command::ShowSubstitutions
        )
    }

    /// The key that does it, if a key does: the chord's own.
    pub fn shortcut(&self) -> Option<(Code, Modifiers)> {
        match self {
            Command::Chord(_, code, mods) => Some((*code, *mods)),
            Command::Link => Some((Code::KeyK, CMD)),
            Command::ExportPdf => Some((Code::KeyE, CMD | SHIFT)),
            Command::PdfPreview => Some((Code::KeyP, CMD | ALT)),
            Command::PageView => Some((Code::KeyL, CMD | ALT)),
            Command::CheckDocument => Some((Code::Semicolon, CMD)),
            Command::Zoom(ZoomCommand::In) => Some((Code::Equal, CMD)),
            Command::Zoom(ZoomCommand::Out) => Some((Code::Minus, CMD)),
            Command::Zoom(ZoomCommand::Reset) => Some((Code::Digit0, CMD)),
            Command::New(Format::Markdown) => Some((Code::KeyN, CMD)),
            Command::Open => Some((Code::KeyO, CMD)),
            Command::Save => Some((Code::KeyS, CMD)),
            Command::SaveAs => Some((Code::KeyS, CMD | SHIFT)),
            Command::Close => Some((Code::KeyW, CMD)),
            Command::Quit => Some((Code::KeyQ, CMD)),
            Command::Settings => Some((Code::Comma, CMD)),
            _ => None,
        }
    }
}

/// Whether a command can be done, and whether it is ticked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub enabled: bool,
    pub checked: bool,
}

impl Status {
    const ON: Status = Status {
        enabled: true,
        checked: false,
    };
    const OFF: Status = Status {
        enabled: false,
        checked: false,
    };

    fn tick(yes: bool) -> Status {
        Status {
            enabled: true,
            checked: yes,
        }
    }
}

/// `c`'s status for a document whose chrome is `chrome` (`None` with no
/// document window in front), `app` the switches and choices the app holds.
pub fn status(c: &Command, chrome: Option<&Chrome>, app: &AppState) -> Status {
    match c {
        Command::Switch(s) => return Status::tick(app.switches.contains(s)),
        Command::Column(w) => return Status::tick(app.column == *w),
        Command::Size(z) => return Status::tick(app.size == *z),
        c if c.is_app() => return Status::ON,
        _ => {}
    }
    let Some(chrome) = chrome else {
        return Status::OFF;
    };
    let rich = !chrome.source_view;
    let mark = |k| Status::tick(chrome.marks.contains(k));
    let enabled = |yes| if yes { Status::ON } else { Status::OFF };
    match c {
        Command::Chord(key, _, mods) => {
            let ch = match key {
                Key::Character(s) => s.as_str(),
                _ => "",
            };
            match (ch, *mods) {
                ("z", m) if m == CMD => enabled(chrome.can_undo),
                ("z", m) if m == CMD | SHIFT => enabled(chrome.can_redo),
                ("x" | "c", m) if m == CMD => enabled(chrome.has_selection),
                ("b", m) if m == CMD => mark(InlineKind::Strong),
                ("i", m) if m == CMD => mark(InlineKind::Emph),
                ("u", m) if m == CMD => mark(InlineKind::Insert),
                ("x", m) if m == CMD | SHIFT => mark(InlineKind::Delete),
                ("c", m) if m == CMD | SHIFT => mark(InlineKind::Verbatim),
                ("m", m) if m == CMD | SHIFT => mark(InlineKind::Mark),
                ("e", m) if m == CMD => Status::tick(chrome.source_view),
                ("u", m) if m == CMD | SHIFT => Status::tick(chrome.task == Some(true)),
                ("(", _) => Status::tick(chrome.blockquote),
                ("ç", _) => Status::tick(chrome.code_block),
                (d, m) if m == CTRL && d.len() == 1 => {
                    Status::tick(d.parse::<u32>().ok() == Some(chrome.heading))
                }
                _ => Status::ON,
            }
        }
        Command::Table(TableCommand::Insert { .. }) => enabled(rich),
        Command::Table(_) => enabled(chrome.in_table),
        Command::Highlight(colour) => Status {
            enabled: rich,
            checked: chrome.marks.contains(InlineKind::Mark) && chrome.mark_color == *colour,
        },
        Command::Verse => Status::tick(chrome.verse),
        Command::PreserveBreaks => Status::tick(chrome.preserve_breaks),
        Command::OpenLink | Command::CopyLink => enabled(chrome.link),
        Command::CheckDocument => enabled(rich),
        Command::PdfPreview => Status::tick(chrome.preview),
        Command::PageView => Status::tick(chrome.pages),
        _ => Status::ON,
    }
}

/// What the app holds that a menu ticks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppState {
    pub switches: Vec<Switch>,
    pub column: ColumnWidth,
    pub size: TextSize,
}

/// A shortcut as a Mac menu shows it: ⌃⌥⇧⌘ then the key.
#[cfg(target_os = "macos")]
pub fn shortcut_label(code: &Code, mods: Modifiers) -> String {
    let mut s = String::new();
    for (m, glyph) in [(CTRL, "⌃"), (ALT, "⌥"), (SHIFT, "⇧"), (CMD, "⌘")] {
        if mods.contains(m) {
            s.push_str(glyph);
        }
    }
    s.push_str(&key_label(code));
    s
}

/// A shortcut as Linux and Windows menus show it: `Ctrl+Shift+B`.
#[cfg(not(target_os = "macos"))]
pub fn shortcut_label(code: &Code, mods: Modifiers) -> String {
    let mut s = String::new();
    for (m, name) in [(CTRL, "Ctrl+"), (ALT, "Alt+"), (SHIFT, "Shift+")] {
        if mods.contains(m) {
            s.push_str(name);
        }
    }
    s.push_str(&key_label(code));
    s
}

/// A key as a menu names it.
fn key_label(code: &Code) -> String {
    match code {
        Code::ArrowUp => "↑".to_string(),
        Code::ArrowDown => "↓".to_string(),
        Code::ArrowLeft => "←".to_string(),
        Code::ArrowRight => "→".to_string(),
        c => key_char(c).to_uppercase(),
    }
}

/// The character a code's key types unshifted: what a menu's key
/// equivalent is matched by, with Shift in its modifiers.
pub fn key_char(code: &Code) -> String {
    let name = format!("{code:?}");
    if let Some(letter) = name.strip_prefix("Key") {
        return letter.to_lowercase();
    }
    if let Some(digit) = name.strip_prefix("Digit") {
        return digit.into();
    }
    match code {
        Code::Comma => ",",
        Code::Period => ".",
        Code::Equal => "=",
        Code::Minus => "-",
        Code::BracketLeft => "[",
        Code::BracketRight => "]",
        Code::Semicolon => ";",
        _ => "",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chrome() -> Chrome {
        Chrome::default()
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn a_shortcut_reads_as_a_mac_menu_writes_it() {
        assert_eq!(shortcut_label(&Code::Digit8, CMD | SHIFT), "⇧⌘8");
        assert_eq!(shortcut_label(&Code::KeyB, CMD), "⌘B");
        assert_eq!(shortcut_label(&Code::ArrowUp, ALT), "⌥↑");
        assert_eq!(shortcut_label(&Code::Digit1, CTRL), "⌃1");
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn a_shortcut_reads_as_a_linux_menu_writes_it() {
        assert_eq!(shortcut_label(&Code::Digit8, CMD | SHIFT), "Ctrl+Shift+8");
        assert_eq!(shortcut_label(&Code::KeyB, CMD), "Ctrl+B");
        assert_eq!(shortcut_label(&Code::ArrowUp, ALT), "Alt+↑");
    }

    #[test]
    fn a_mark_in_force_is_ticked_and_undo_waits_for_an_edit() {
        let mut c = chrome();
        c.marks = [InlineKind::Strong].into_iter().collect();
        let app = AppState::default();
        let bold = Command::chord("b", Code::KeyB, CMD);
        assert!(status(&bold, Some(&c), &app).checked);
        let undo = Command::chord("z", Code::KeyZ, CMD);
        assert!(!status(&undo, Some(&c), &app).enabled);
        c.can_undo = true;
        assert!(status(&undo, Some(&c), &app).enabled);
    }

    #[test]
    fn without_a_document_only_the_apps_commands_are_enabled() {
        let app = AppState::default();
        assert!(!status(&Command::chord("b", Code::KeyB, CMD), None, &app).enabled);
        assert!(status(&Command::Settings, None, &app).enabled);
    }

    #[test]
    fn the_table_items_wait_for_a_table() {
        let app = AppState::default();
        let mut c = chrome();
        let row = Command::Table(TableCommand::DeleteRow);
        assert!(!status(&row, Some(&c), &app).enabled);
        c.in_table = true;
        assert!(status(&row, Some(&c), &app).enabled);
    }
}
