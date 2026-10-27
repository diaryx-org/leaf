//! Every menu the app shows, as data: the menu bar's, the toolbar's and the
//! context menu's. One declaration, drawn two ways — as the system's menu
//! bar where there is one (the Mac's, `platform/macos/menubar.rs`), and as
//! a [`unit`] menu in the page ([`entries`]) — with each item's
//! enabled and ticked state from [`commands::status`] either way.

use dioxus::html::{Code, NamedKey};
use leaf_core::{Alignment, Format, MarkColor};
use leaf_dioxus::{TableCommand, ZoomCommand};
use unit::{Entry, Item};

use crate::commands::{self, ALT, AppState, CMD, CTRL, Command, SHIFT, Switch, shortcut_label};
use crate::page::Chrome;
use crate::settings::{Choice, ColumnWidth, TextSize};

/// One thing in a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// An item that carries one of the app's commands.
    Item(&'static str, Command),
    /// An item that is the system's own — Hide, Quit, Open Recent, Print —
    /// by its Objective-C selector, with its shortcut. Only a system menu
    /// bar shows these.
    System(
        &'static str,
        &'static str,
        Option<(Code, dioxus::html::Modifiers)>,
    ),
    Separator,
    Submenu(&'static str, Vec<Node>),
}

use Node::{Item as I, Separator as Sep, Submenu as Sub, System as Sys};

fn chord(ch: &str, code: Code, mods: dioxus::html::Modifiers) -> Command {
    Command::chord(ch, code, mods)
}

/// Format ▸ Heading's items, and the toolbar's Style menu's.
fn headings() -> Vec<Node> {
    let codes = [
        Code::Digit1,
        Code::Digit2,
        Code::Digit3,
        Code::Digit4,
        Code::Digit5,
        Code::Digit6,
    ];
    const LABELS: [&str; 6] = [
        "Heading 1",
        "Heading 2",
        "Heading 3",
        "Heading 4",
        "Heading 5",
        "Heading 6",
    ];
    codes
        .into_iter()
        .enumerate()
        .map(|(i, code)| I(LABELS[i], chord(&(i + 1).to_string(), code, CTRL)))
        .collect()
}

pub fn highlight() -> Vec<Node> {
    let mut v = vec![I("Default", Command::Highlight(None)), Sep];
    for (label, c) in [
        ("Red", MarkColor::Red),
        ("Orange", MarkColor::Orange),
        ("Yellow", MarkColor::Yellow),
        ("Green", MarkColor::Green),
        ("Blue", MarkColor::Blue),
        ("Purple", MarkColor::Purple),
        ("Brown", MarkColor::Brown),
    ] {
        v.push(I(label, Command::Highlight(Some(c))));
    }
    v
}

/// Format ▸ Table, as Swift's `TableRows` lays it out.
pub fn table() -> Vec<Node> {
    use TableCommand::*;
    let t = Command::Table;
    vec![
        I("Insert Table", t(Insert { cols: 2 })),
        Sub(
            "Insert Table With",
            vec![
                I("3 Columns", t(Insert { cols: 3 })),
                I("4 Columns", t(Insert { cols: 4 })),
                I("5 Columns", t(Insert { cols: 5 })),
            ],
        ),
        Sep,
        I("Insert Row Above", t(InsertRow { below: false })),
        I("Insert Row Below", t(InsertRow { below: true })),
        I("Delete Row", t(DeleteRow)),
        Sep,
        I("Insert Column Left", t(InsertColumn { right: false })),
        I("Insert Column Right", t(InsertColumn { right: true })),
        I("Delete Column", t(DeleteColumn)),
        Sep,
        Sub(
            "Align Column",
            vec![
                I("Left", t(Align(Alignment::Left))),
                I("Center", t(Align(Alignment::Center))),
                I("Right", t(Align(Alignment::Right))),
                I("Default", t(Align(Alignment::Default))),
            ],
        ),
        Sep,
        I("Move Row Up", t(MoveRow { down: false })),
        I("Move Row Down", t(MoveRow { down: true })),
        I("Move Column Left", t(MoveColumn { right: false })),
        I("Move Column Right", t(MoveColumn { right: true })),
    ]
}

fn lists() -> Vec<Node> {
    vec![
        I("Bulleted List", chord("*", Code::Digit8, CMD | SHIFT)),
        I("Numbered List", chord("&", Code::Digit7, CMD | SHIFT)),
        I("Checklist", chord("l", Code::KeyL, CMD | SHIFT)),
        I("Checked", chord("u", Code::KeyU, CMD | SHIFT)),
        Sep,
        I("Indent", chord("]", Code::BracketRight, CMD)),
        I("Outdent", chord("[", Code::BracketLeft, CMD)),
        Sep,
        I(
            "Move Block Up",
            Command::named(NamedKey::ArrowUp, Code::ArrowUp, ALT),
        ),
        I(
            "Move Block Down",
            Command::named(NamedKey::ArrowDown, Code::ArrowDown, ALT),
        ),
    ]
}

fn inserts() -> Vec<Node> {
    vec![
        I("Insert Footnote", Command::InsertFootnote),
        I("Insert Horizontal Rule", Command::InsertRule),
        I("Insert Page Break", Command::InsertPageBreak),
        Sub("Table", table()),
    ]
}

/// File ▾ in the toolbar, where there is no menu bar to hold it (Linux and
/// Windows): the Mac's app and File menus' commands that are not the
/// system's there.
pub fn file() -> Vec<Node> {
    vec![
        I("New Markdown Document", Command::New(Format::Markdown)),
        I("New Djot Document", Command::New(Format::Djot)),
        I("New HTML Document", Command::New(Format::Html)),
        I("Open…", Command::Open),
        Sep,
        I("Save", Command::Save),
        I("Save As…", Command::SaveAs),
        I("Export as PDF…", Command::ExportPdf),
        I("PDF Preview", Command::PdfPreview),
        Sep,
        I("Open Sample Document", Command::OpenSample),
        I("Settings…", Command::Settings),
        Sep,
        I("Close", Command::Close),
        I("Quit", Command::Quit),
    ]
}

/// Commands with a key that a menu bar would answer before the editor, and
/// that are not in [`file`]: where there is no menu bar, the page answers
/// them (`platform::shortcut`).
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn keyed() -> Vec<Node> {
    vec![I("Link…", Command::Link)]
}

/// The menu bar: the app menu, File, Edit, Format, View, Window and Help,
/// as the SwiftUI Mac app laid them out.
pub fn bar() -> Vec<(&'static str, Vec<Node>)> {
    let k = |c: Code, m| Some((c, m));
    vec![
        (
            "Leaf",
            vec![
                Sys("About Leaf", "orderFrontStandardAboutPanel:", None),
                Sep,
                I("Settings…", Command::Settings),
                Sep,
                Sub("Services", vec![]),
                Sep,
                Sys("Hide Leaf", "hide:", k(Code::KeyH, CMD)),
                Sys(
                    "Hide Others",
                    "hideOtherApplications:",
                    k(Code::KeyH, CMD | ALT),
                ),
                Sys("Show All", "unhideAllApplications:", None),
                Sep,
                Sys("Quit Leaf", "terminate:", k(Code::KeyQ, CMD)),
            ],
        ),
        (
            "File",
            vec![
                Sub(
                    "New",
                    vec![
                        I("Markdown Document", Command::New(Format::Markdown)),
                        I("Djot Document", Command::New(Format::Djot)),
                        I("HTML Document", Command::New(Format::Html)),
                    ],
                ),
                Sys("Open…", "openDocument:", k(Code::KeyO, CMD)),
                Sub(
                    "Open Recent",
                    vec![Sys("Clear Menu", "clearRecentDocuments:", None)],
                ),
                Sep,
                Sys("Close", "performClose:", k(Code::KeyW, CMD)),
                Sys("Save…", "saveDocument:", k(Code::KeyS, CMD)),
                Sys(
                    "Duplicate",
                    "duplicateDocument:",
                    k(Code::KeyS, CMD | SHIFT),
                ),
                Sys("Rename…", "renameDocument:", None),
                Sys("Move To…", "moveDocument:", None),
                Sub(
                    "Revert To",
                    vec![
                        Sys("Last Saved Version", "revertDocumentToSaved:", None),
                        Sys("Browse All Versions…", "browseDocumentVersions:", None),
                    ],
                ),
                Sep,
                I("Export as PDF…", Command::ExportPdf),
                Sep,
                Sys("Page Setup…", "runPageLayout:", k(Code::KeyP, CMD | SHIFT)),
                Sys("Print…", "printDocument:", k(Code::KeyP, CMD)),
            ],
        ),
        (
            // Titled "Edit", so that a Mac adds Emoji & Symbols and Start
            // Dictation to it.
            "Edit",
            vec![
                I("Undo", chord("z", Code::KeyZ, CMD)),
                I("Redo", chord("z", Code::KeyZ, CMD | SHIFT)),
                Sep,
                // The system's selectors, so that a text field the system
                // draws (a save panel's) has them too.
                Sys("Cut", "cut:", k(Code::KeyX, CMD)),
                Sys("Copy", "copy:", k(Code::KeyC, CMD)),
                Sys("Paste", "paste:", k(Code::KeyV, CMD)),
                Sys(
                    "Paste and Match Style",
                    "pasteAsPlainText:",
                    k(Code::KeyV, CMD | SHIFT | ALT),
                ),
                Sys("Select All", "selectAll:", k(Code::KeyA, CMD)),
                Sep,
                Sub(
                    "Find",
                    vec![
                        I("Find…", chord("f", Code::KeyF, CMD)),
                        I("Find and Replace…", chord("ƒ", Code::KeyF, CMD | ALT)),
                        I("Find Next", chord("g", Code::KeyG, CMD)),
                        I("Find Previous", chord("g", Code::KeyG, CMD | SHIFT)),
                    ],
                ),
                Sub(
                    "Spelling and Grammar",
                    vec![
                        I("Check Document Now", Command::CheckDocument),
                        Sep,
                        I(
                            "Check Spelling While Typing",
                            Command::Switch(Switch::WhileTyping),
                        ),
                        I(
                            "Correct Spelling Automatically",
                            Command::Switch(Switch::Correct),
                        ),
                    ],
                ),
                Sub(
                    "Substitutions",
                    vec![
                        I("Show Substitutions", Command::ShowSubstitutions),
                        Sep,
                        I("Smart Quotes", Command::Switch(Switch::Quotes)),
                        I("Smart Dashes", Command::Switch(Switch::Dashes)),
                        I("Text Replacement", Command::Switch(Switch::Replacement)),
                    ],
                ),
            ],
        ),
        (
            "Format",
            [
                vec![
                    I("Bold", chord("b", Code::KeyB, CMD)),
                    I("Italic", chord("i", Code::KeyI, CMD)),
                    I("Underline", chord("u", Code::KeyU, CMD)),
                    I("Strikethrough", chord("x", Code::KeyX, CMD | SHIFT)),
                    I("Code", chord("c", Code::KeyC, CMD | SHIFT)),
                    I("Highlight", chord("m", Code::KeyM, CMD | SHIFT)),
                    Sub("Highlight Colour", highlight()),
                    I("Link…", Command::Link),
                    Sep,
                    I("Paragraph", chord("0", Code::Digit0, CTRL)),
                    Sub("Heading", headings()),
                    Sep,
                    I("Block Quote", chord("(", Code::Digit9, CMD | SHIFT)),
                    I("Code Block", chord("ç", Code::KeyC, CMD | ALT)),
                    I("Verse", Command::Verse),
                ],
                lists(),
                vec![Sep],
                inserts(),
            ]
            .concat(),
        ),
        (
            "View",
            vec![
                I("Source View", chord("e", Code::KeyE, CMD)),
                I("Preserve Line Breaks", Command::PreserveBreaks),
                I("PDF Preview", Command::PdfPreview),
                Sep,
                // Commands of their own, not chords: a Mac hands the menu a
                // chord it names before the window sees it, which keeps
                // Blitz's page zoom, on the same chords, from acting too.
                I("Make Text Bigger", Command::Zoom(ZoomCommand::In)),
                I("Make Text Smaller", Command::Zoom(ZoomCommand::Out)),
                I("Actual Size", Command::Zoom(ZoomCommand::Reset)),
                Sep,
                Sys(
                    "Enter Full Screen",
                    "toggleFullScreen:",
                    k(Code::KeyF, CMD | CTRL),
                ),
            ],
        ),
        (
            "Window",
            vec![
                Sys("Minimize", "performMiniaturize:", k(Code::KeyM, CMD)),
                Sys("Zoom", "performZoom:", None),
                Sep,
                Sys("Bring All to Front", "arrangeInFront:", None),
            ],
        ),
        ("Help", vec![I("Open Sample Document", Command::OpenSample)]),
    ]
}

/// The toolbar's Style ▾: the paragraph's kind — LeafToolbar's
/// `LeafStyleMenu`.
pub fn style() -> Vec<Node> {
    [
        vec![I("Body", chord("0", Code::Digit0, CTRL))],
        headings(),
        vec![
            Sep,
            I("Block Quote", chord("(", Code::Digit9, CMD | SHIFT)),
            I("Code Block", chord("ç", Code::KeyC, CMD | ALT)),
            I("Verse", Command::Verse),
        ],
    ]
    .concat()
}

/// The toolbar's Format ▾: the marks past the three on the bar, and links.
pub fn format() -> Vec<Node> {
    vec![
        I("Strikethrough", chord("x", Code::KeyX, CMD | SHIFT)),
        I("Code", chord("c", Code::KeyC, CMD | SHIFT)),
        I("Highlight", chord("m", Code::KeyM, CMD | SHIFT)),
        Sub("Highlight Colour", highlight()),
        Sep,
        I("Link…", Command::Link),
    ]
}

/// The toolbar's Lists ▾.
pub fn lists_menu() -> Vec<Node> {
    lists()
}

/// The toolbar's Insert ▾.
pub fn insert() -> Vec<Node> {
    vec![
        Sub("Table", table()),
        I("Horizontal Rule", Command::InsertRule),
        I("Footnote", Command::InsertFootnote),
        Sep,
        I("Page Break", Command::InsertPageBreak),
    ]
}

/// The toolbar's Aa ▾: line breaks, the column and the size.
pub fn display() -> Vec<Node> {
    let mut v = vec![I("Preserve Line Breaks", Command::PreserveBreaks), Sep];
    v.extend(
        ColumnWidth::ALL
            .iter()
            .map(|c| I(c.label(), Command::Column(*c))),
    );
    v.push(Sep);
    v.extend(
        TextSize::ALL
            .iter()
            .map(|s| I(s.label(), Command::Size(*s))),
    );
    v
}

/// `nodes` as a page's menu: each item's id an index into the commands it
/// returns, its shortcut shown, its state the document's. The system's own
/// items are left out.
pub fn entries(
    nodes: &[Node],
    chrome: Option<&Chrome>,
    app: &AppState,
) -> (Vec<Entry>, Vec<Command>) {
    let mut commands = Vec::new();
    let entries = build(nodes, chrome, app, &mut commands);
    (entries, commands)
}

fn build(
    nodes: &[Node],
    chrome: Option<&Chrome>,
    app: &AppState,
    out: &mut Vec<Command>,
) -> Vec<Entry> {
    let mut v: Vec<Entry> = Vec::new();
    for n in nodes {
        match n {
            Node::Item(label, c) => {
                let s = commands::status(c, chrome, app);
                let mut item = Item::new(out.len(), *label)
                    .enabled(s.enabled)
                    .checked(s.checked);
                if let Some((code, mods)) = c.shortcut() {
                    item = item.shortcut(shortcut_label(&code, mods));
                }
                out.push(c.clone());
                v.push(item.into());
            }
            Node::System(..) => {}
            Node::Separator => {
                if !matches!(v.last(), None | Some(Entry::Separator)) {
                    v.push(Entry::Separator);
                }
            }
            Node::Submenu(label, children) => {
                let children = build(children, chrome, app, out);
                if !children.is_empty() {
                    v.push(Entry::submenu(*label, children));
                }
            }
        }
    }
    if matches!(v.last(), Some(Entry::Separator)) {
        v.pop();
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_menu_numbers_its_commands_and_leaves_the_systems_out() {
        let (built, commands) = entries(&bar()[1].1, None, &AppState::default());
        // File's own items only: New ▸ three, Export as PDF.
        assert_eq!(commands.len(), 4);
        // No separator left leading or doubled where system items were.
        assert!(!matches!(built.first(), Some(Entry::Separator)));
        assert!(
            built
                .windows(2)
                .all(|w| !(w[0] == Entry::Separator && w[1] == Entry::Separator))
        );
        let (_, commands) = entries(&style(), Some(&Chrome::default()), &AppState::default());
        assert_eq!(commands[0], chord("0", Code::Digit0, CTRL));
    }

    #[test]
    fn every_item_with_a_chord_shows_it() {
        let (built, _) = entries(&lists(), Some(&Chrome::default()), &AppState::default());
        let Entry::Item(first) = &built[0] else {
            panic!()
        };
        let label = shortcut_label(&Code::Digit8, CMD | SHIFT);
        assert_eq!(first.shortcut.as_deref(), Some(label.as_str()));
    }

    #[test]
    fn the_heading_in_force_is_ticked() {
        let chrome = Chrome {
            heading: 2,
            ..Chrome::default()
        };
        let (built, _) = entries(&style(), Some(&chrome), &AppState::default());
        let Entry::Item(h2) = &built[2] else { panic!() };
        assert_eq!(h2.label, "Heading 2");
        assert!(h2.checked);
    }
}
