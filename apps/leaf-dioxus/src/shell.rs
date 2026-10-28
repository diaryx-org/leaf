//! A document window's half of the editor's world: the [`Shell`], which is
//! the editor's [`Host`].
//!
//! The editor asks for what it does not own; this answers through the
//! platform. ⌘S is the platform document's Save, so autosave and Save As
//! are the system's; ⌘, opens Settings; the zoom is the window's own, over
//! the shared display choices; a link leaving the document goes to the
//! system; the context menu is the page's (`context.rs`), drawn in the
//! page's overlay.

use std::cell::Cell;

use dioxus::html::{Code, Key, Modifiers};
use leaf_core::Doc;
use leaf_dioxus::{Context, Editor, Host, Look, Request, ZoomCommand, trace};

use crate::app::{self, AppEvent};
use crate::platform::{self, DocumentRef};
use crate::settings::zoom_step;

pub struct Shell {
    pub document: DocumentRef,
    /// The text zoom over the display choices' size: this window's alone.
    pub zoom: Cell<f32>,
    /// Set by every repaint: the chrome has something new to read.
    pub stale: Cell<bool>,
}

impl Shell {
    pub fn new(document: DocumentRef) -> Self {
        Self {
            document,
            zoom: Cell::new(1.0),
            stale: Cell::new(true),
        }
    }

    /// What the editor draws with: the display choices, at the zoom.
    pub fn look(&self) -> Look {
        let mut look = Self::look_unzoomed();
        look.ty = app::settings().ty(self.zoom.get());
        look
    }

    /// The display choices at their own size: what goes on paper.
    pub fn look_unzoomed() -> Look {
        let s = app::settings();
        Look {
            ty: s.ty(1.0),
            measure: s.column.chars(),
            appearance: s.appearance,
            ..Default::default()
        }
    }

    /// A key, from the window or replayed by a menu item.
    pub fn key(&self, ed: &Editor, key: Key, code: Code, mods: Modifiers) {
        ed.key(key, code, mods);
    }

    /// ⌘+, ⌘−, ⌘0: the text set larger or smaller, and the column with it,
    /// since its measure is counted in characters.
    pub fn zoom(&self, ed: &Editor, z: ZoomCommand) {
        // In the page view, the pages are what zooms.
        if let Some(page) = app::page(ed.window_id())
            && crate::preview::zoom(&page, z)
        {
            return;
        }
        let was = self.zoom.get();
        let now = match z {
            ZoomCommand::In => zoom_step(was, true),
            ZoomCommand::Out => zoom_step(was, false),
            ZoomCommand::Reset => 1.0,
        };
        ed.doc().status = Some(format!("Zoom {:.0}%", now * 100.0));
        if now != was {
            self.zoom.set(now);
            ed.set_look(self.look());
        } else {
            ed.refresh();
        }
    }
}

impl Host for Shell {
    fn request(&self, ed: &Editor, request: Request) {
        trace(|| format!("request {request:?}"));
        match request {
            Request::Save => self.document.save(),
            Request::SaveAs => self.document.save_as(),
            Request::Zoom(z) => self.zoom(ed, z),
            Request::Settings => app::post(AppEvent::ShowSettings),
        }
    }

    fn write_clipboard(&self, text: &str, html: Option<&str>) -> Result<(), String> {
        platform::clipboard::write(text, html)
    }

    fn read_clipboard(&self) -> leaf_dioxus::Clipboard {
        platform::clipboard::read()
    }

    fn character_palette(&self) {
        platform::character_palette();
    }

    fn open(&self, what: &str) {
        platform::open_url(what);
    }

    /// Asked while the document may be borrowed: the menu is built on the
    /// handler's next turn, where the spelling checker can read it.
    fn context_menu(&self, cx: &Context) {
        if let Some(id) = self.document.page() {
            app::post(AppEvent::ContextMenu(id, *cx));
        }
    }

    fn repainted(&self, _doc: &Doc) {
        self.stale.set(true);
    }
}

/// The system's clipboard, for a dialog's text field.
pub struct SystemClipboard;

impl unit::Clipboard for SystemClipboard {
    fn read(&self) -> Option<String> {
        platform::clipboard::read().text
    }

    fn write(&self, text: &str) {
        let _ = platform::clipboard::write(text, None);
    }
}
