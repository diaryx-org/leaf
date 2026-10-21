//! The file behind the document: Save and Save As, the question before a
//! save writes over somebody else's change, and the watch that notices one.
//!
//! The rules are leaf-tui's (`apps/leaf-tui/src/main.rs`, `tick_disk` and
//! `attempt_save`), over the same leaf-core calls:
//!
//! - A **clean** document catches up with its file by itself — once the file
//!   has stopped moving, so a write is never taken half done — as one undo
//!   step.
//! - A **dirty** one says so once, and a save asks: Overwrite, Reload, or
//!   Cancel.
//! - An untitled document's save is a Save As.

use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use leaf_core::{DiskState, Doc};
use leaf_dioxus::Editor;
use unit::{ActionSheet, Answer, Dialog, SheetAction, Unit};

use crate::chrome::{Pending, queue};

/// How often the file is looked at.
pub const POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// What the watch last saw of the file.
#[derive(Default)]
pub struct Watch {
    stamp: Option<(SystemTime, u64)>,
    /// The file moved on the last look: wait for it to stand still.
    settling: bool,
    /// The file is ahead of a dirty document: said once, asked at the save.
    ahead: bool,
    last: Option<Instant>,
}

fn stamp(doc: &Doc) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(&doc.path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

impl Watch {
    pub fn new(doc: &Doc) -> Self {
        Self {
            stamp: stamp(doc),
            ..Self::default()
        }
    }

    /// After our own save, the file looks as we left it.
    pub fn saw_save(&mut self, doc: &Doc) {
        self.stamp = stamp(doc);
        self.settling = false;
        self.ahead = false;
    }

    /// One look, if it is time for one: whether anything changed on screen.
    pub fn tick(&mut self, ed: &Editor) -> bool {
        if self.last.is_some_and(|at| at.elapsed() < POLL) {
            return false;
        }
        self.last = Some(Instant::now());
        let mut doc = ed.doc();
        if doc.is_untitled() {
            return false;
        }
        let now = stamp(&doc);
        let moved = now != self.stamp;
        self.stamp = now;
        if !(moved || self.settling || (self.ahead && !doc.dirty)) {
            return false;
        }
        match doc.disk_state() {
            DiskState::Changed if doc.dirty => {
                self.ahead = true;
                self.settling = false;
                if moved {
                    doc.status = Some("file changed on disk — saving will ask".into());
                    drop(doc);
                    ed.refresh();
                    return true;
                }
                false
            }
            DiskState::Changed if moved => {
                self.settling = true;
                false
            }
            DiskState::Changed => {
                self.settling = false;
                self.ahead = false;
                let before = doc.revision();
                doc.reload();
                let changed = doc.revision() != before;
                if changed {
                    doc.status = Some("reloaded from disk".into());
                }
                drop(doc);
                ed.refresh();
                changed
            }
            DiskState::Missing => {
                self.ahead = false;
                self.settling = false;
                if moved {
                    doc.status = Some(format!("{} is no longer on disk", doc.file_name()));
                    drop(doc);
                    ed.refresh();
                    return true;
                }
                false
            }
            _ => {
                self.ahead = false;
                self.settling = false;
                false
            }
        }
    }
}

/// What a save should do now.
pub enum Save {
    /// Write the file.
    Write,
    /// Ask where: the document has no file.
    AskWhere,
    /// Ask first: the file changed since leaf last read or wrote it.
    AskOverwrite,
    /// Refuse: the document is only to be read.
    ReadOnly,
}

pub fn plan(doc: &Doc) -> Save {
    if doc.read_only() {
        Save::ReadOnly
    } else if doc.is_untitled() {
        Save::AskWhere
    } else if doc.disk_state() == DiskState::Changed {
        Save::AskOverwrite
    } else {
        Save::Write
    }
}

/// Ask for a path to save to, seeded with the document's own.
pub fn prompt_save_as(unit: &Unit, current: String) {
    let dialog = Dialog::new("Save As")
        .message("The file to save the document to.")
        .field(current, "notes.md")
        .buttons("Save", "Cancel");
    unit.open_dialog(dialog, |answer| {
        if let Answer::Ok(path) = answer
            && !path.trim().is_empty()
        {
            queue(Pending::SaveTo(expand(path.trim())));
        }
    });
}

/// `~` at the start of a typed path is the home directory, as in a shell.
fn expand(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// The file changed under unsaved edits: overwrite it, take it, or neither.
/// Cancel is where the keys start: overwriting is not what a stray Return
/// should do.
pub fn confirm_overwrite(unit: &Unit, name: &str) {
    let mut sheet = ActionSheet::new(vec![
        SheetAction::new(0, "Overwrite").destructive(),
        SheetAction::new(1, "Reload"),
    ])
    .title(format!("{name} changed on disk"))
    .message("Overwrite it with your version, or reload it and lose your changes?")
    .cancel("Cancel");
    sheet.hot = Some(sheet.actions.len());
    unit.open_actions(sheet, |choice| match choice {
        Some(0) => queue(Pending::Overwrite),
        Some(1) => queue(Pending::Reload),
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tilde_is_home() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand("~/a.md"), PathBuf::from(home).join("a.md"));
        assert_eq!(expand("/x/a.md"), PathBuf::from("/x/a.md"));
    }
}
