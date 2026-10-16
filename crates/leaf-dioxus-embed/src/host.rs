//! The editor's [`Host`], answered by the native host through its callbacks.
//!
//! Every answer that may run a nested event loop on the host's side — a
//! context menu tracking, a save panel, a document opened in another app — is
//! sent on the main queue's next turn rather than from inside the editor's
//! handler: the editor and Blitz are mid-event then, and an event delivered
//! into them from a nested loop would find them borrowed. The repaint notice
//! is deferred and coalesced too, so the host reading the document back from
//! it never meets the editor's own borrow.

use std::cell::Cell;
use std::rc::Rc;

use dioxus_native::winit::window::WindowId;
use leaf_core::Doc;
use leaf_dioxus::{Context, Editor, Footnote, Host, Request, ZoomCommand};

use crate::callbacks::Callbacks;
use crate::runtime;

/// `LEAF_REQUEST_*`.
pub const REQUEST_SAVE: u32 = 1;
pub const REQUEST_SAVE_AS: u32 = 2;
pub const REQUEST_ZOOM: u32 = 3;
pub const REQUEST_SETTINGS: u32 = 4;
pub const REQUEST_CHARACTER_PALETTE: u32 = 5;

/// `LEAF_CONTEXT_*`.
pub const CONTEXT_SELECTION: u32 = 1;
pub const CONTEXT_LINK: u32 = 2;
pub const CONTEXT_GO_TO_NOTE: u32 = 4;
pub const CONTEXT_BACK_TO_REFERENCE: u32 = 8;

pub struct SwiftHost {
    id: WindowId,
    /// Whether a repaint notice is already on its way.
    notice: Rc<Cell<bool>>,
}

impl SwiftHost {
    pub fn new(id: WindowId) -> Self {
        Self {
            id,
            notice: Rc::default(),
        }
    }

    /// Call the host on the next turn, if the view is still there by then.
    fn later(&self, f: impl FnOnce(&Callbacks) + 'static) {
        let id = self.id;
        runtime::later(move || {
            if let Some(h) = runtime::get(id) {
                f(h.callbacks());
            }
        });
    }
}

impl Host for SwiftHost {
    fn request(&self, _ed: &Editor, request: Request) {
        let (kind, arg) = match request {
            Request::Save => (REQUEST_SAVE, 0),
            Request::SaveAs => (REQUEST_SAVE_AS, 0),
            Request::Zoom(ZoomCommand::In) => (REQUEST_ZOOM, 1),
            Request::Zoom(ZoomCommand::Out) => (REQUEST_ZOOM, -1),
            Request::Zoom(ZoomCommand::Reset) => (REQUEST_ZOOM, 0),
            Request::Settings => (REQUEST_SETTINGS, 0),
        };
        self.later(move |c| c.request(kind, arg));
    }

    fn character_palette(&self) {
        self.later(|c| c.request(REQUEST_CHARACTER_PALETTE, 0));
    }

    fn open(&self, what: &str) {
        let what = what.to_owned();
        self.later(move |c| c.open(&what));
    }

    fn context_menu(&self, cx: &Context) {
        let mut flags = 0;
        if cx.selection {
            flags |= CONTEXT_SELECTION;
        }
        if cx.link {
            flags |= CONTEXT_LINK;
        }
        match cx.footnote {
            Some(Footnote::GoToNote) => flags |= CONTEXT_GO_TO_NOTE,
            Some(Footnote::BackToReference) => flags |= CONTEXT_BACK_TO_REFERENCE,
            None => {}
        }
        self.later(move |c| c.context_menu(flags));
    }

    fn repainted(&self, _doc: &Doc) {
        if !self.notice.replace(true) {
            let notice = self.notice.clone();
            self.later(move |c| {
                notice.set(false);
                c.changed();
            });
        }
    }
}
