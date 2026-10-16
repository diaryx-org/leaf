//! What the editor asks of the host, across the C ABI: a table of function
//! pointers and the host's context pointer, which every one is called with.
//!
//! Each is optional — a null pointer is no opinion — and each is called on the
//! main thread, from inside a host call or a main-queue block, never while the
//! host is inside a call of its own into the editor that has not returned.

use std::ffi::{CString, c_char, c_void};

/// The host's half of the ABI, as `LeafHostCallbacks` in the header.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Callbacks {
    pub ctx: *mut c_void,
    /// The editor repainted: the document, the caret or the selection may
    /// have changed. The host re-reads whatever it shows from them — the
    /// edited dot, the menus' ticks.
    pub changed: Option<extern "C" fn(ctx: *mut c_void)>,
    /// A command the editor leaves to the host: `LEAF_REQUEST_*`, with an
    /// argument for those that take one.
    pub request: Option<extern "C" fn(ctx: *mut c_void, kind: u32, arg: i32)>,
    /// Hand a URL or a file path to the system.
    pub open: Option<extern "C" fn(ctx: *mut c_void, what: *const c_char)>,
    /// A right-click placed the caret: pop up the context menu, offering
    /// what `flags` (`LEAF_CONTEXT_*`) says applies.
    pub context_menu: Option<extern "C" fn(ctx: *mut c_void, flags: u32)>,
    /// Where the caret is, in points from the view's top-left corner, for
    /// the text input client's `firstRectForCharacterRange:`.
    pub ime_area: Option<extern "C" fn(ctx: *mut c_void, x: f64, y: f64, w: f64, h: f64)>,
    /// The pointer's shape: 0 the arrow, 1 the I-beam, 2 the pointing hand.
    pub set_cursor: Option<extern "C" fn(ctx: *mut c_void, cursor: u32)>,
    /// For a host that scrolls the editor: the editor moved its viewport
    /// itself, or its content changed height. `top` is how far down the
    /// content the viewport starts and `height` how tall the content is, in
    /// points; `inset_top` and `inset_bottom` are what the find bar and the
    /// status line cover of the viewport, for the scroller to keep clear of.
    pub scrolled: Option<
        extern "C" fn(ctx: *mut c_void, top: f64, height: f64, inset_top: f64, inset_bottom: f64),
    >,
}

/// What [`Callbacks::scrolled`] reports.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Scroll {
    pub top: f64,
    pub height: f64,
    pub inset_top: f64,
    pub inset_bottom: f64,
}

impl Callbacks {
    /// No host at all: what an offscreen editor answers to.
    pub fn none() -> Self {
        Self {
            ctx: std::ptr::null_mut(),
            changed: None,
            request: None,
            open: None,
            context_menu: None,
            ime_area: None,
            set_cursor: None,
            scrolled: None,
        }
    }

    pub fn changed(&self) {
        if let Some(f) = self.changed {
            f(self.ctx);
        }
    }

    pub fn request(&self, kind: u32, arg: i32) {
        if let Some(f) = self.request {
            f(self.ctx, kind, arg);
        }
    }

    pub fn open(&self, what: &str) {
        if let (Some(f), Ok(s)) = (self.open, CString::new(what)) {
            f(self.ctx, s.as_ptr());
        }
    }

    pub fn context_menu(&self, flags: u32) {
        if let Some(f) = self.context_menu {
            f(self.ctx, flags);
        }
    }

    pub fn ime_area(&self, x: f64, y: f64, w: f64, h: f64) {
        if let Some(f) = self.ime_area {
            f(self.ctx, x, y, w, h);
        }
    }

    pub fn set_cursor(&self, cursor: u32) {
        if let Some(f) = self.set_cursor {
            f(self.ctx, cursor);
        }
    }

    pub fn scrolled(&self, g: Scroll) {
        if let Some(f) = self.scrolled {
            f(self.ctx, g.top, g.height, g.inset_top, g.inset_bottom);
        }
    }
}
