//! A file, as AppKit's document system sees it: `LeafDocument`, an
//! `NSDocument` subclass defined from Rust.
//!
//! The document controller owns everything about a file's life — the Open
//! panel, Save and Save As, autosave in place, Versions, Duplicate, Rename,
//! Move To, Revert, the recents and Open Recent, Finder's Open With and the
//! Dock's — and asks this class only to read bytes, write bytes, and make a
//! window. `Info.plist` names the class for each of the three formats, by
//! the name registered here.
//!
//! The window is the page's (`page.rs`), made by the app's handler on its
//! next turn (`app.rs`): `makeWindowControllers` only asks for it. Once it
//! exists, the document's source *is* the page's `Doc`, which the editor
//! edits in place and `dataOfType:` reads; until then it is the text
//! `readFromData:` was given.
//!
//! Closing is the document's too. The close button and File ▸ Close are
//! `performClose:`, and for a window with a document AppKit asks the
//! document first — autosaving a titled one, asking about an untitled one —
//! and only once it agrees calls the window delegate's `windowShouldClose:`,
//! which winit reports as `CloseRequested`. So by the time the page hears
//! of it the answer is yes, and asking again (`canCloseDocumentWithDelegate:`)
//! waits on the first question's file access, which never finishes.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use dioxus_native::winit::window::{Window, WindowId};
use leaf_core::{Doc, Format};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{ClassType, DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSDocument, NSDocumentChangeType, NSPrintOperation, NSSaveOperationType, NSWindowController,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSError, NSString, NSURL};

use super::{mtm, ns_window};
use crate::app::{self, AppEvent};

/// The uniform type identifiers `Info.plist` declares, and their formats.
pub const TYPES: [(&str, Format); 3] = [
    ("net.daringfireball.markdown", Format::Markdown),
    ("org.djot.djot", Format::Djot),
    ("public.html", Format::Html),
];

/// The format a type identifier names; Markdown for any other.
pub fn format_of(type_name: &str) -> Format {
    TYPES
        .iter()
        .find(|(t, _)| *t == type_name)
        .map_or(Format::Markdown, |(_, f)| *f)
}

/// The type identifier a format is saved as.
pub fn type_of(format: Format) -> &'static str {
    TYPES
        .iter()
        .find(|(_, f)| *f == format)
        .map_or(TYPES[0].0, |(t, _)| t)
}

#[derive(Default)]
pub struct Ivars {
    /// The text read from the file, until the page takes it.
    contents: RefCell<Option<String>>,
    /// The page showing this document, once there is one.
    page: Cell<Option<WindowId>>,
}

thread_local! {
    /// Set before File ▸ New makes the document the sample is to fill
    /// (Help ▸ Open Sample Document, `--sample`).
    static SAMPLE_NEXT: Cell<bool> = const { Cell::new(false) };
}

/// The next untitled document opens on the bundled sample.
pub fn sample_next() {
    SAMPLE_NEXT.set(true);
}

define_class!(
    #[unsafe(super(NSDocument))]
    #[thread_kind = MainThreadOnly]
    #[name = "LeafDocument"]
    #[ivars = Ivars]
    pub struct LeafDocument;

    impl LeafDocument {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Option<Retained<Self>> {
            let this = this.set_ivars(Ivars::default());
            unsafe { msg_send![super(this), init] }
        }

        /// Save as the reader types, without asking, as every Mac app since
        /// Lion does; Versions keeps what came before.
        #[unsafe(method(autosavesInPlace))]
        fn autosaves_in_place() -> bool {
            true
        }

        #[unsafe(method(readFromData:ofType:error:))]
        fn read_from_data(
            &self,
            data: &NSData,
            _type_name: &NSString,
            error: *mut *mut NSError,
        ) -> bool {
            self.read(data, error)
        }

        #[unsafe(method_id(dataOfType:error:))]
        fn data_of_type(
            &self,
            _type_name: &NSString,
            _error: *mut *mut NSError,
        ) -> Option<Retained<NSData>> {
            self.data()
        }

        #[unsafe(method(makeWindowControllers))]
        fn make_window_controllers(&self) {
            app::post(AppEvent::Show(Message::retain(self)));
        }

        /// A document is saved in the format it was read in: Save As offers
        /// its own type alone, so a Markdown source is never written into a
        /// file named `.html`.
        #[unsafe(method_id(writableTypesForSaveOperation:))]
        fn writable_types(&self, _op: NSSaveOperationType) -> Retained<NSArray<NSString>> {
            let ty = self
                .fileType()
                .unwrap_or_else(|| NSString::from_str(TYPES[0].0));
            NSArray::from_retained_slice(&[ty])
        }

        #[unsafe(method(setFileURL:))]
        fn set_file_url(&self, url: Option<&NSURL>) {
            let _: () = unsafe { msg_send![super(self), setFileURL: url] };
            if let Some(id) = self.ivars().page.get() {
                app::post(AppEvent::Moved(id));
            }
        }

        /// File ▸ Print…: the document as the PDF Export as PDF writes, on
        /// the print panel's paper.
        #[unsafe(method_id(printOperationWithSettings:error:))]
        fn print_operation(
            &self,
            _settings: &NSDictionary<NSString, AnyObject>,
            _error: *mut *mut NSError,
        ) -> Option<Retained<NSPrintOperation>> {
            self.print_operation_now()
        }
    }
);

impl LeafDocument {
    /// `readFromData:ofType:error:`: the text, for the page to take — or,
    /// with a page already, to show in place of what it has.
    fn read(&self, data: &NSData, error: *mut *mut NSError) -> bool {
        let Ok(source) = String::from_utf8(data.to_vec()) else {
            if let Some(out) = unsafe { error.as_mut() } {
                // NSFileReadCorruptFileError.
                let e = unsafe {
                    NSError::errorWithDomain_code_userInfo(
                        objc2_foundation::NSCocoaErrorDomain,
                        259,
                        None,
                    )
                };
                *out = Retained::autorelease_ptr(e);
            }
            return false;
        };
        match self.ivars().page.get() {
            // Revert, or the file changed under an unedited document.
            Some(id) => app::post(AppEvent::Reload(id, source)),
            None => *self.ivars().contents.borrow_mut() = Some(source),
        }
        true
    }

    /// `dataOfType:error:`: the page's source, or the text read if no page
    /// has taken it yet.
    fn data(&self) -> Option<Retained<NSData>> {
        let source = match self.ivars().page.get().and_then(app::page) {
            Some(page) => page.source_to_save()?,
            None => self.ivars().contents.borrow().clone().unwrap_or_default(),
        };
        Some(NSData::with_bytes(source.as_bytes()))
    }

    fn print_operation_now(&self) -> Option<Retained<NSPrintOperation>> {
        let page = self.ivars().page.get().and_then(app::page)?;
        super::print::operation(&page, &self.printInfo())
    }

    /// The page showing this document, if it has one yet.
    pub fn page(&self) -> Option<WindowId> {
        self.ivars().page.get().filter(|&id| app::is_open(id))
    }

    /// The format, from the document's type.
    pub fn format(&self) -> Format {
        format_of(&self.fileType().map(|t| t.to_string()).unwrap_or_default())
    }

    /// Where the document is, for its pictures' relative paths: its file,
    /// or for an untitled one the bundle's resources, where the sample's
    /// pictures are.
    pub fn path(&self) -> PathBuf {
        if let Some(path) = self.fileURL().and_then(|u| u.path()) {
            return PathBuf::from(path.to_string());
        }
        super::resources().join("Untitled.md")
    }

    /// The window's title as AppKit will have it: the file's display name.
    pub fn title(&self) -> String {
        self.displayName().to_string()
    }

    /// The document as the editor takes it, the first time a page asks: the
    /// text read, an untitled document's nothing, or the sample.
    pub fn take_doc(&self) -> Doc {
        let source = match self.ivars().contents.borrow_mut().take() {
            Some(s) => s,
            None if SAMPLE_NEXT.replace(false) && self.format() == Format::Markdown => {
                super::sample()
            }
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
        let format = self.format();
        let mut doc = Doc::from_source(source, format)
            .or_else(|_| Doc::from_source(String::new(), format))
            .unwrap_or_else(|_| Doc::blank().expect("a blank document"));
        doc.path = self.path();
        doc
    }

    /// The page is made: tell AppKit the window is this document's, so the
    /// title bar, the edited dot, the proxy icon and the File menu's
    /// commands are the document's, and show it where it was last left.
    pub fn attach(&self, id: WindowId, window: &dyn Window) {
        self.ivars().page.set(Some(id));
        let Some(ns) = ns_window(window) else { return };
        let controller =
            NSWindowController::initWithWindow(NSWindowController::alloc(mtm()), Some(&ns));
        controller.setShouldCloseDocument(true);
        self.addWindowController(&controller);
        // Reopen where it was left: a frame saved under the file's path.
        if let Some(path) = self.fileURL().and_then(|u| u.path()) {
            let name = NSString::from_str(&format!("Leaf document {path}"));
            ns.setFrameUsingName(&name);
            ns.setFrameAutosaveName(&name);
        } else {
            ns.center();
        }
        self.showWindows();
    }

    /// ⌘S: the document's own Save — a Versions snapshot for a titled one,
    /// the save sheet for an untitled one.
    pub fn save(&self) {
        let _: () = unsafe { msg_send![self, saveDocument: None::<&AnyObject>] };
    }

    /// ⇧⌘S's Save As.
    pub fn save_as(&self) {
        let _: () = unsafe { msg_send![self, saveDocumentAs: None::<&AnyObject>] };
    }

    /// Bring its window forward.
    pub fn show(&self) {
        self.showWindows();
    }

    /// The editor made an edit AppKit has not heard of: the edited dot,
    /// Save enabled, the autosave clock.
    pub fn edited(&self) {
        self.updateChangeCount(NSDocumentChangeType::ChangeDone);
    }
}

/// Register the class with the runtime, before AppKit looks it up by name.
pub fn register() {
    let _ = LeafDocument::class();
}
