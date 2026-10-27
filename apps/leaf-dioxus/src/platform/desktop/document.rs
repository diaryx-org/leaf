//! A file, as the app itself keeps it: `Document`, with no document system
//! under it.
//!
//! The Mac's `NSDocument` owns a file's life and asks the page only for
//! bytes; here leaf-core's own file API does that work, as it does for the
//! terminal editor. Open is `Doc::open`, which notes what was read; Save is
//! `Doc::save_as` to the document's file, which writes beside it and renames
//! over, and only then marks the document saved; Save As asks where with the
//! system's save dialog. The window's title is the file's name, with a `*`
//! before it while the document is dirty — which undoing back to what was
//! saved clears.
//!
//! The file is watched as leaf-core can watch it: when the window comes
//! forward ([`Document::focused`]) a clean document takes what another
//! program wrote, and a dirty one is told; a Save over such a change asks
//! first.
//!
//! Closing asks first. The close button and File ▸ Close come to
//! [`Document::ask_to_close`], which closes a document with nothing unsaved
//! and otherwise asks, in the page's own dialog, whether to discard what
//! would be lost.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use dioxus_native::winit::window::{Window, WindowId};
use leaf_core::{DiskState, Doc, Format};
use unit::{Answer, Dialog};

use crate::app::{self, AppEvent};
use crate::page::Page;

/// The formats Save As offers, by their extensions: the first is the one it
/// suggests.
pub const TYPES: [(Format, &str, &[&str]); 3] = [
    (Format::Markdown, "Markdown", &["md", "markdown"]),
    (Format::Djot, "Djot", &["dj", "djot"]),
    (Format::Html, "HTML", &["html", "htm"]),
];

pub struct Document {
    /// The file, once it has one. The editor's `Doc::path` is not enough to
    /// say: an untitled document's points into the app's resources, so the
    /// sample's pictures load.
    file: RefCell<Option<PathBuf>>,
    format: Format,
    /// The document read from the file, until the page takes it.
    opened: RefCell<Option<Doc>>,
    /// The page showing this document, once there is one.
    page: Cell<Option<WindowId>>,
    /// Whether the title shows it dirty.
    dirty: Cell<bool>,
    /// Whether it opens on the bundled sample.
    sample: bool,
}

impl Document {
    /// An untitled document of `format`; on the sample, if `sample`.
    pub fn untitled(format: Format, sample: bool) -> Rc<Self> {
        Rc::new(Self {
            file: RefCell::new(None),
            format,
            opened: RefCell::new(None),
            page: Cell::new(None),
            dirty: Cell::new(false),
            sample,
        })
    }

    /// The document in the file at `path`, in the format its extension
    /// names.
    pub fn read(path: &Path) -> Result<Rc<Self>, String> {
        let doc = Doc::open(path.to_path_buf()).map_err(|e| format!("{e:#}"))?;
        Ok(Rc::new(Self {
            file: RefCell::new(Some(path.to_path_buf())),
            format: doc.format,
            opened: RefCell::new(Some(doc)),
            page: Cell::new(None),
            dirty: Cell::new(false),
            sample: false,
        }))
    }

    /// The file, if it has one.
    pub fn file(&self) -> Option<PathBuf> {
        self.file.borrow().clone()
    }

    /// The page showing this document, if it has one yet.
    pub fn page(&self) -> Option<WindowId> {
        self.page.get().filter(|&id| app::is_open(id))
    }

    /// Where the document is, for its pictures' relative paths: its file,
    /// or for an untitled one the app's resources, where the sample's
    /// pictures are.
    pub fn path(&self) -> PathBuf {
        self.file()
            .unwrap_or_else(|| super::resources().join("Untitled.md"))
    }

    /// The file's name, or "Untitled".
    pub fn title(&self) -> String {
        self.file()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Untitled".into())
    }

    /// The document as the editor takes it, the first time a page asks: the
    /// file read, an untitled document's nothing, or the sample.
    pub fn take_doc(&self) -> Doc {
        let mut doc = match self.opened.borrow_mut().take() {
            Some(d) => d,
            None if self.sample => self.take_doc_from(super::sample()),
            None => self.take_doc_from(String::new()),
        };
        // Settings ▸ Preserve line breaks in new windows.
        if app::settings().preserve {
            doc.set_line_flow(leaf_core::LineFlow::Preserve);
        }
        doc
    }

    /// `source` as this document's: its format, and its place for its
    /// pictures.
    pub fn take_doc_from(&self, source: String) -> Doc {
        let mut doc = Doc::from_source(source, self.format)
            .or_else(|_| Doc::from_source(String::new(), self.format))
            .unwrap_or_else(|_| Doc::blank().expect("a blank document"));
        doc.path = self.path();
        doc
    }

    /// The page is made: the window is this document's, titled and shown.
    pub fn attach(&self, id: WindowId, window: &dyn Window) {
        self.page.set(Some(id));
        window.set_title(&self.window_title());
        window.set_visible(true);
        window.focus_window();
    }

    /// Bring its window forward.
    pub fn show(&self) {
        if let Some(p) = self.page().and_then(app::page) {
            p.window.focus_window();
        }
    }

    /// The editor made an edit: the title says so.
    pub fn edited(&self) {
        self.show_dirty(true);
    }

    /// The document is clean again — saved, or undone back to what was:
    /// the title says so.
    pub fn clean(&self) {
        self.show_dirty(false);
    }

    fn show_dirty(&self, dirty: bool) {
        if self.dirty.replace(dirty) != dirty {
            self.retitle();
        }
    }

    /// The title bar's text: the name, starred while it has unsaved changes.
    fn window_title(&self) -> String {
        let star = if self.dirty.get() { "*" } else { "" };
        format!("{star}{} — Leaf", self.title())
    }

    fn retitle(&self) {
        if let Some(p) = self.page().and_then(app::page) {
            p.window.set_title(&self.window_title());
        }
    }

    /// Ctrl+S: write the file; ask where first if there is none, and
    /// whether to write over it if another program changed it since.
    pub fn save(&self) {
        let Some(path) = self.file() else {
            return self.save_as();
        };
        let Some(page) = self.page().and_then(app::page) else {
            return;
        };
        let changed = page
            .with_doc(|d| d.disk_state() == DiskState::Changed)
            .unwrap_or(false);
        if !changed {
            self.write(&page, path);
            return;
        }
        let Some(unit) = page.unit() else { return };
        let id = page.window.id();
        let dialog = Dialog::new(format!("“{}” changed on disk", self.title()))
            .message(
                "Another program has changed the file since it was opened or saved. \
                 Saving writes over those changes.",
            )
            .buttons("Save Anyway", "Cancel")
            .destructive();
        page.in_scope(|| {
            unit.open_dialog(dialog, move |answer| {
                if let Answer::Ok(_) = answer {
                    let path = path.clone();
                    app::on_turn(move || {
                        if let Some(page) = app::page(id)
                            && let Some(shell) = page.shell()
                        {
                            shell.document.write(&page, path);
                        }
                    });
                }
            })
        });
    }

    /// Ctrl+Shift+S: ask where, then write there; the document's file is
    /// the new one from then on.
    pub fn save_as(&self) {
        let Some(id) = self.page() else { return };
        let suggested = self.file().map_or_else(
            || {
                let ext = TYPES.iter().find(|(f, ..)| *f == self.format);
                format!("Untitled.{}", ext.map_or("md", |(_, _, e)| e[0]))
            },
            |_| self.title(),
        );
        let folder = self.file().and_then(|p| p.parent().map(Path::to_path_buf));
        let (name, exts) = TYPES
            .iter()
            .find(|(f, ..)| *f == self.format)
            .map(|(_, n, e)| (*n, *e))
            .unwrap_or(("Markdown", &["md"]));
        super::save_dialog(&suggested, folder, (name, exts), move |path| {
            if let Some(page) = app::page(id)
                && let Some(shell) = page.shell()
            {
                shell.document.write(&page, path);
            }
        });
    }

    /// Write the editor's document to `path` through leaf-core, which marks
    /// it saved only once the bytes have landed, and says in the status line
    /// if they did not. `path` is the document's file from then on.
    fn write(&self, page: &Page, path: PathBuf) {
        let landed = page
            .with_editor(|ed, _| {
                let landed = ed.doc().save_as(path.clone());
                ed.refresh();
                landed
            })
            .unwrap_or(false);
        if !landed {
            return;
        }
        let moved = self.file().as_deref() != Some(path.as_path());
        *self.file.borrow_mut() = Some(path);
        if moved {
            app::post(AppEvent::Moved(page.window.id()));
        }
        self.dirty.set(false);
        self.retitle();
    }

    /// The window came forward: take what another program wrote to the
    /// file, if nothing here is unsaved; otherwise say so, and Save will
    /// ask.
    pub fn focused(&self, page: &Page) {
        if self.file().is_none() {
            return;
        }
        page.with_editor(|ed, _| {
            let mut doc = ed.doc();
            let said = match doc.disk_state() {
                DiskState::Changed if doc.dirty => {
                    Some("The file changed on disk — saving will ask".to_string())
                }
                DiskState::Changed => {
                    doc.reload();
                    Some("Reloaded from disk".to_string())
                }
                DiskState::Missing => Some(format!("{} is no longer on disk", doc.file_name())),
                _ => None,
            };
            if let Some(s) = said {
                doc.status = Some(s);
                drop(doc);
                ed.refresh();
            }
        });
    }

    /// The close button or File ▸ Close: close, after asking if what has
    /// not been saved would be lost.
    pub fn ask_to_close(&self, page: &Page) {
        let id = page.window.id();
        let dirty = page.with_doc(|d| d.dirty).unwrap_or(true);
        if !dirty {
            app::post(AppEvent::Close(id));
            return;
        }
        let Some(unit) = page.unit() else {
            return;
        };
        page.window.focus_window();
        let dialog = Dialog::new(format!("Discard changes to “{}”?", self.title()))
            .message("They have not been saved. To keep them, cancel and save first.")
            .buttons("Discard Changes", "Cancel")
            .destructive();
        page.in_scope(|| {
            unit.open_dialog(dialog, move |answer| {
                if let Answer::Ok(_) = answer {
                    app::post(AppEvent::Close(id));
                }
            })
        });
    }

    /// The window has gone.
    pub fn close(&self) {
        self.page.set(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untitled_document_is_named_so_and_finds_pictures_in_the_resources() {
        let d = Document::untitled(Format::Markdown, false);
        assert_eq!(d.title(), "Untitled");
        assert!(d.path().starts_with(super::super::resources()));
        assert_eq!(d.window_title(), "Untitled — Leaf");
    }

    #[test]
    fn a_read_document_takes_its_files_text_name_and_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.dj");
        std::fs::write(&path, "_hello_\n").unwrap();
        let d = Document::read(&path).unwrap();
        assert_eq!(d.title(), "note.dj");
        assert_eq!(d.format, Format::Djot);
        let doc = d.take_doc();
        assert_eq!(doc.source, "_hello_\n");
        assert_eq!(doc.path, path);
        assert_eq!(
            doc.disk_state(),
            DiskState::Unchanged,
            "what was read is noted"
        );
    }

    #[test]
    fn the_title_is_starred_while_dirty_and_not_once_clean_again() {
        let d = Document::untitled(Format::Markdown, false);
        d.edited();
        assert_eq!(d.window_title(), "*Untitled — Leaf");
        d.clean();
        assert_eq!(d.window_title(), "Untitled — Leaf");
    }
}
