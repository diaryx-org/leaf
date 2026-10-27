//! What differs between platforms, behind one API each platform's module
//! provides under the same names — the way `std::sys` is laid out, so the
//! shared code calls `platform::new_document(..)` with no trait objects and
//! no `cfg` of its own.
//!
//! Each platform module has:
//!
//! - `Document`: one document's handle in the system's document model — on
//!   the Mac an `NSDocument` — with `take_doc`, `take_doc_from`, `path`,
//!   `title`, `attach`, `show`, `edited`, `clean`, `focused`, `ask_to_close`
//!   and `close`.
//! - `start()` before the event loop runs, `launched()` once it does, and
//!   `event_loop(&mut builder)` for the loop's own settings.
//! - `document_window(attributes)` and `TITLE_BAR`: how a document window
//!   is dressed, and how far its page runs up behind the system's title.
//! - `new_document`, `open_sample`, `sample`, `resources`.
//! - `open_url`, `character_palette`, `font_families`, `later`,
//!   `main_page`, and `defaults::{string, set, set_bool, remove}`.
//! - `spelling`: the system's checker and substitutions, or none.
//! - `lookup::{show, force_click, word_at, AVAILABLE}`: the system's
//!   dictionary, or none.
//! - `print::export`: putting the document on paper.
//! - `open_panel`, `quit`, `QUITS_WITH_LAST_WINDOW`, and `shortcut`: what
//!   the File menu does where the system does not, and its keys where
//!   there is no menu bar to take them first.
//!
//! The Mac's is `macos`, AppKit through objc2; Linux's and Windows' is
//! `desktop`, the app keeping its own files.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(not(target_os = "macos"))]
mod desktop;
#[cfg(not(target_os = "macos"))]
pub use desktop::*;
