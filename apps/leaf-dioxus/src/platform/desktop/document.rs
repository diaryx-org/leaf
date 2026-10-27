//! A file, as the app itself keeps it: `Document`, with no document system
//! under it.
//!
//! The Mac's `NSDocument` owns a file's life and asks the page only for
//! bytes; here the app does that work itself, and as little of it as a
//! desktop editor needs. Open reads the file; Save writes the page's source
//! back to it, or asks where with the system's save dialog when the document
//! has no file yet; Save As asks every time. The window's title is the
//! file's name, with a `*` before it while it has changes no save has
//! written.
//!
//! Closing asks first. The close button and File ▸ Close come to
//! [`Document::ask_to_close`], which closes a document with nothing unsaved
//! and otherwise asks, in the page's own dialog, whether to discard what
//! would be lost.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use dioxus_native::winit::window::{Window, WindowId};
use leaf_core::{Doc, Format};
use unit::{Answer, Dialog};

use crate::app::{self, AppEvent};
use crate::page::Page;

/// The formats a document is read and saved as, by their extensions: the
/// first is the one Save As suggests.
pub const TYPES: [(Format, &str, &[&str]); 3] = [
    (Format::Markdown, "Markdown", &["md", "markdown"]),
    (Format::Djot, "Djot", &["dj", "djot"]),
    (Format::Html, "HTML", &["html", "htm"]),
];

/// The format a file's extension names; Markdown for any other.
pub fn format_of(path: &Path) -> Format {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    TYPES
        .iter()
        .find(|(_, _, exts)| exts.contains(&ext.as_str()))
        .map_or(Format::Markdown, |(f, ..)| *f)
}

pub struct Document {
    /// The file, once it has one.
    file: RefCell<Option<PathBuf>>,
    format: Format,
    /// The text read from the file, until the page takes it.
    contents: RefCell<Option<String>>,
    /// The page showing this document, once there is one.
    page: Cell<Option<WindowId>>,
    /// Whether it has changes no save has written.
    edited: Cell<bool>,
    /// Whether it opens on the bundled sample.
    sample: bool,
}

impl Document {
    /// An untitled document of `format`; on the sample, if `sample`.
    pub fn untitled(format: Format, sample: bool) -> Rc<Self> {
        Rc::new(Self {
            file: RefCell::new(None),
            format,
            contents: RefCell::new(None),
            page: Cell::new(None),
            edited: Cell::new(false),
            sample,
        })
    }

    /// The document in the file at `path`.
    pub fn read(path: &Path) -> Result<Rc<Self>, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let source =
            String::from_utf8(bytes).map_err(|_| format!("{}: not UTF-8 text", path.display()))?;
        Ok(Rc::new(Self {
            file: RefCell::new(Some(path.to_path_buf())),
            format: format_of(path),
            contents: RefCell::new(Some(source)),
            page: Cell::new(None),
            edited: Cell::new(false),
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

    pub fn format(&self) -> Format {
        self.format
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
    /// text read, an untitled document's nothing, or the sample.
    pub fn take_doc(&self) -> Doc {
        let source = match self.contents.borrow_mut().take() {
            Some(s) => s,
            None if self.sample => super::sample(),
            None => String::new(),
        };
        let mut doc = self.take_doc_from(source);
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

    /// The editor made an edit: the title says so until it is saved.
    pub fn edited(&self) {
        if !self.edited.replace(true) {
            self.retitle();
        }
    }

    /// The title bar's text: the name, starred while it has unsaved changes.
    fn window_title(&self) -> String {
        let star = if self.edited.get() { "*" } else { "" };
        format!("{star}{} — Leaf", self.title())
    }

    fn retitle(&self) {
        if let Some(p) = self.page().and_then(app::page) {
            p.window.set_title(&self.window_title());
        }
    }

    /// Ctrl+S: write the file, or ask where first if there is none.
    pub fn save(&self) {
        match self.file() {
            Some(path) => {
                self.write(&path);
            }
            None => self.save_as(),
        }
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
            let Some(page) = app::page(id) else { return };
            if let Some(shell) = page.shell() {
                shell.document.write_as(&path);
            }
        });
    }

    /// Write to `path` and make it the document's file.
    fn write_as(&self, path: &Path) {
        if self.write(path) {
            *self.file.borrow_mut() = Some(path.to_path_buf());
            if let Some(id) = self.page() {
                app::post(AppEvent::Moved(id));
            }
            self.retitle();
        }
    }

    /// Write the page's source to `path`: whether it got there. The
    /// document is marked saved only once the write has landed, so a failed
    /// save leaves it dirty and Close still asks.
    fn write(&self, path: &Path) -> bool {
        let Some(page) = self.page().and_then(app::page) else {
            return false;
        };
        let Some(source) = page.source() else {
            return false;
        };
        match replace_file(path, source.as_bytes()) {
            Ok(()) => {
                page.saved();
                self.edited.set(false);
                self.retitle();
                true
            }
            Err(e) => {
                page.say(&format!("Save failed: {e}"));
                false
            }
        }
    }

    /// The close button or File ▸ Close: close, after asking if what has
    /// not been saved would be lost.
    pub fn ask_to_close(&self, page: &Page) {
        let id = page.window.id();
        let dirty = self.edited.get() || page.with_doc(|d| d.dirty).unwrap_or(true);
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

/// Put `bytes` at `path` whole or not at all: written to a file beside it,
/// then renamed over it, so a write that fails partway (a full disk) leaves
/// the last saved file as it was. A file already there keeps its
/// permissions, and a symbolic link is followed, so the file it names is the
/// one replaced and the link stays a link.
fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = dir.join(format!(".{name}.leaf-save-{}", std::process::id()));
    let written = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Ok(meta) = std::fs::metadata(&target) {
            std::fs::set_permissions(&temp, meta.permissions())?;
        }
        std::fs::rename(&temp, &target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("leaf-desktop-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_save_replaces_the_file_whole_and_leaves_nothing_beside_it() {
        let dir = scratch("replace");
        let path = dir.join("note.md");
        std::fs::write(&path, "old\n").unwrap();
        replace_file(&path, b"new\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\n");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(names, ["note.md"]);
    }

    #[test]
    fn a_save_into_a_folder_that_is_not_there_fails() {
        let dir = scratch("unwritable");
        let path = dir.join("missing-folder").join("note.md");
        assert!(replace_file(&path, b"new\n").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_save_keeps_the_files_permissions_and_follows_a_link() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("perms");
        let real = dir.join("real.md");
        let link = dir.join("link.md");
        std::fs::write(&real, "old\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        replace_file(&link, b"new\n").unwrap();
        let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        let still_link = std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink();
        let text = std::fs::read_to_string(&real).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(text, "new\n");
        assert_eq!(mode, 0o600);
        assert!(still_link);
    }

    #[test]
    fn a_files_extension_names_its_format() {
        assert_eq!(format_of(Path::new("a/notes.dj")), Format::Djot);
        assert_eq!(format_of(Path::new("page.HTML")), Format::Html);
        assert_eq!(format_of(Path::new("notes.md")), Format::Markdown);
        assert_eq!(format_of(Path::new("README")), Format::Markdown);
    }

    #[test]
    fn an_untitled_document_is_named_so_and_finds_pictures_in_the_resources() {
        let d = Document::untitled(Format::Markdown, false);
        assert_eq!(d.title(), "Untitled");
        assert!(d.path().starts_with(super::super::resources()));
        assert_eq!(d.window_title(), "Untitled — Leaf");
    }

    #[test]
    fn a_read_document_takes_its_files_text_and_name() {
        let dir = std::env::temp_dir().join(format!("leaf-desktop-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("note.dj");
        std::fs::write(&path, "_hello_\n").unwrap();
        let d = Document::read(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(d.title(), "note.dj");
        assert_eq!(d.format(), Format::Djot);
        let doc = d.take_doc();
        assert_eq!(doc.source, "_hello_\n");
        assert_eq!(doc.path, path);
    }
}
