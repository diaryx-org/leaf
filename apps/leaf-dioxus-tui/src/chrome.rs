//! Everything round the editor, drawn by unit in the page's overlay as the
//! Mac app's are: the context menu, the command menu, the Link prompt, and
//! the question before quitting with unsaved changes.
//!
//! unit's root is in its cell density (`Density::Cell`): every row of a menu
//! or a dialog is one cell high, set in the editor's monospace face. Its
//! choices are queued ([`take`]) and carried out by the main loop, never
//! inside the event that made them — a click on a menu item arrives while
//! Blitz is dispatching, and an editor command then would be a re-entry.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use leaf_dioxus::{Context, Editor, Footnote, Host, Request};
use unit::{ActionSheet, Answer, Dialog, Entry, Item, SheetAction, Unit};

use crate::commands::{self, Command};

/// What a choice asks of the main loop.
#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    Command(Command),
    /// Save, asking where or whether first if it must (`files.rs`).
    Save,
    SaveTo(std::path::PathBuf),
    /// Write over the file that changed on disk.
    Overwrite,
    /// Take the file that changed on disk, losing the edits.
    Reload,
    FollowFootnote,
    OpenLink,
    CopyLink,
    InsertLink(String),
    SaveAndQuit,
    QuitNow,
}

thread_local! {
    static UNIT: Cell<Option<Unit>> = const { Cell::new(None) };
    static QUEUE: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
    /// Where the pointer last pressed, in CSS pixels: where a context menu
    /// opens.
    static POINTER: Cell<(f64, f64)> = const { Cell::new((16.0, 24.0)) };
}

pub fn set_unit(unit: Unit) {
    UNIT.set(Some(unit));
}

pub fn unit() -> Option<Unit> {
    UNIT.get()
}

pub fn set_pointer(at: (f64, f64)) {
    POINTER.set(at);
}

pub fn queue(p: Pending) {
    QUEUE.with_borrow_mut(|q| q.push(p));
}

/// What has been chosen since the last call.
pub fn take() -> Vec<Pending> {
    QUEUE.with_borrow_mut(std::mem::take)
}

/// The terminal's host: Save, opening a link, and the context menu.
pub struct TermHost;

impl Host for TermHost {
    /// Save and Save As go by way of the main loop, which may have to ask
    /// first (`files.rs`).
    fn request(&self, _ed: &Editor, request: Request) {
        match request {
            Request::Save => queue(Pending::Save),
            Request::SaveAs => queue(Pending::Command(Command::SaveAs)),
            _ => {}
        }
    }

    /// A URL to the system's opener, detached from the terminal.
    fn open(&self, what: &str) {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener)
            .arg(what)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    fn context_menu(&self, cx: &Context) {
        let Some(unit) = unit() else { return };
        // The menu's lines, `None` a separator, then its entries.
        let mut lines: Vec<Option<(&str, Option<String>, Pending)>> = Vec::new();
        match cx.footnote {
            Some(Footnote::GoToNote) => {
                lines.push(Some(("Go to Note", None, Pending::FollowFootnote)))
            }
            Some(Footnote::BackToReference) => {
                lines.push(Some(("Back to Reference", None, Pending::FollowFootnote)))
            }
            None => {}
        }
        if cx.link {
            lines.push(Some(("Open Link", None, Pending::OpenLink)));
            lines.push(Some(("Edit Link…", None, Pending::Command(Command::Link))));
            lines.push(Some(("Copy Link", None, Pending::CopyLink)));
        }
        if !lines.is_empty() {
            lines.push(None);
        }
        let edit = commands::table()
            .into_iter()
            .find(|(t, _)| *t == "Edit")
            .map(|(_, l)| l)
            .unwrap_or_default();
        for (label, keys, c) in edit.into_iter().flatten() {
            let wanted = match label {
                "Cut" | "Copy" => cx.selection,
                "Paste" | "Paste as Plain Text" | "Select All" => true,
                _ => false,
            };
            if wanted {
                lines.push(Some((label, keys, Pending::Command(c))));
            }
        }
        let mut picks = Vec::new();
        let entries: Vec<Entry> = lines
            .into_iter()
            .map(|l| match l {
                None => Entry::Separator,
                Some((label, keys, p)) => {
                    picks.push(p);
                    let i = Item::new(picks.len() - 1, label);
                    match keys {
                        Some(k) => i.shortcut(k).into(),
                        None => i.into(),
                    }
                }
            })
            .collect();
        let picks = Rc::new(picks);
        let at = POINTER.get();
        unit.open_menu(entries, at, 0.0, move |i| {
            if let Some(p) = picks.get(i) {
                queue(p.clone());
            }
        });
    }
}

/// The command menu, at the top left, its first item highlighted: it was
/// opened by a key, and the arrows go on from there.
pub fn command_menu(unit: &Unit) {
    let (entries, commands) = commands::menu();
    unit.open_menu(entries, (0.0, 0.0), 0.0, move |i| {
        if let Some(c) = commands.get(i) {
            queue(Pending::Command(c.clone()));
        }
    });
    unit.hot_first();
}

/// Ask for a link's destination, seeded with the link at the caret.
pub fn prompt_link(unit: &Unit, current: String) {
    let dialog = Dialog::new("Link")
        .message("The address the selected text links to.")
        .field(current, "https://")
        .buttons("OK", "Cancel");
    unit.open_dialog(dialog, |answer| {
        if let Answer::Ok(dest) = answer
            && !dest.trim().is_empty()
        {
            queue(Pending::InsertLink(dest.trim().to_string()));
        }
    });
}

/// Save, don't save, or stay: asked before quitting with unsaved changes.
pub fn confirm_quit(unit: &Unit, name: &str) {
    let sheet = ActionSheet::new(vec![
        SheetAction::new(0, "Save"),
        SheetAction::new(1, "Don't Save").destructive(),
    ])
    .title(format!("Save the changes to {name}?"))
    .message("Your changes are lost if you don't save them.")
    .cancel("Cancel");
    unit.open_actions(sheet, |choice| match choice {
        Some(0) => queue(Pending::SaveAndQuit),
        Some(1) => queue(Pending::QuitNow),
        _ => {}
    });
}
