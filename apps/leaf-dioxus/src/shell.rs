//! The app around the editor: the [`Shell`], which is the editor's
//! [`Host`], and what the menu bar's items do.
//!
//! The editor asks for what it does not own; this answers as a Mac document
//! app does. Save goes to the document's file, or a Save As sheet when it has
//! none (`file.rs`); the zoom and ⌘, are the settings' (`prefs.rs`); a link
//! leaving the document goes to `open`; the context menu is muda's
//! (`menu.rs`); and the window's title is the file's name, "— Edited" while
//! it has unsaved changes.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use dioxus::html::{Code, Key, Modifiers, NamedKey};
use dioxus::prelude::*;
use dioxus_native::winit::window::Window;
use leaf_core::Doc;
use leaf_dioxus::{Context, Editor, Host, Request, trace};

use crate::file::{self, FileAction};
use crate::menu::{self, Command};
use crate::prefs::PrefsView;
use crate::settings::Settings;

/// The app's half of the window: what it keeps, and what it answers the
/// editor with.
pub struct Shell {
    pub window: Arc<dyn Window>,
    /// The display choices (`settings.rs`), and the text zoom over them.
    pub settings: RefCell<Settings>,
    pub zoom: Cell<f32>,
    /// What the settings panel draws.
    pub prefs: Signal<PrefsView>,
    /// The window title last set, so it is only set when it changes.
    title: RefCell<String>,
}

impl Shell {
    pub fn new(window: Arc<dyn Window>, settings: Settings, prefs: Signal<PrefsView>) -> Self {
        Self {
            window,
            settings: RefCell::new(settings),
            zoom: Cell::new(1.0),
            prefs,
            title: RefCell::default(),
        }
    }

    /// A key, from the window or replayed by the menu bar: the settings
    /// panel's Escape, or the editor's.
    pub fn key(&self, ed: &Editor, key: Key, code: Code, mods: Modifiers) {
        // Escape closes the settings panel before anything else has it.
        if key == Key::Named(NamedKey::Escape) && mods.is_empty() && self.prefs_open() {
            self.show_prefs(false);
            return;
        }
        ed.key(key, code, mods);
    }

    /// A menu bar item.
    pub fn command(&self, ed: &Editor, c: Command) {
        trace(|| format!("menu {c:?}"));
        match c {
            Command::Chord(key, code, mods) => self.key(ed, key, code, mods),
            Command::New => file::run(ed, &self.window, FileAction::New),
            Command::Open => file::run(ed, &self.window, FileAction::Open),
            Command::SaveAs => file::run(ed, &self.window, FileAction::SaveAs),
            Command::Table(cmd) => ed.table(cmd),
            Command::OpenLink => {
                ed.open_link();
            }
            Command::CopyLink => ed.copy_link(),
            Command::FollowFootnote => {
                ed.follow_footnote();
            }
        }
    }
}

impl Host for Shell {
    fn request(&self, ed: &Editor, request: Request) {
        match request {
            // ⌘S on a document with no file yet is a Save As, and only the
            // app has a dialog to ask with.
            Request::Save if ed.doc().is_untitled() => {
                file::run(ed, &self.window, FileAction::SaveAs);
            }
            Request::Save => {
                // `save` reports through `status` either way; the status
                // line shows it, so this is a repaint.
                ed.doc().save();
                ed.refresh();
            }
            Request::SaveAs => file::run(ed, &self.window, FileAction::SaveAs),
            Request::Zoom(z) => self.zoom(ed, z),
            Request::Settings => self.toggle_prefs(),
        }
    }

    /// Open the emoji and symbols picker — what *Edit ▸ Emoji & Symbols*
    /// does, for want of the Edit menu item AppKit offers it from. Its choice
    /// arrives as an `Ime::Commit` (see `patches/winit-appkit/PATCH.md`).
    #[cfg(target_os = "macos")]
    fn character_palette(&self) {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        // Keys are handled on the main thread, where winit delivers them.
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).orderFrontCharacterPalette(None);
        }
    }

    /// Hand a URL or a file to the default app for it.
    #[cfg(target_os = "macos")]
    fn open(&self, what: &str) {
        let _ = std::process::Command::new("/usr/bin/open")
            .arg(what)
            .spawn();
    }

    fn context_menu(&self, cx: &Context) {
        menu::show_context(&*self.window, cx);
    }

    /// The file's name, and "Edited" while it has unsaved changes, as a Mac
    /// document window says it.
    fn repainted(&self, doc: &Doc) {
        let now = match doc.dirty {
            true => format!("{} — Edited", doc.file_name()),
            false => doc.file_name(),
        };
        if *self.title.borrow() != now {
            self.window.set_title(&now);
            *self.title.borrow_mut() = now;
        }
    }
}
