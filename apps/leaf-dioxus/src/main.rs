//! Leaf on Dioxus: the leaf-dioxus editor ([`leaf_dioxus`]) in a document
//! app that is Rust from `main` up, for every platform Dioxus's native
//! renderer reaches. The Mac's is `Leaf.app`; iOS is next.
//!
//! ```sh
//! cargo xtask swift                     # Leaf.app, on a copy of the sample
//! cargo run -p leaf-dioxus-app -- corpus/document.md
//! ```
//!
//! Each window is a Dioxus page, drawn by Blitz on wgpu, and the same on
//! every platform: a document's toolbar, editor and status line ([`page`],
//! [`toolbar`]), its context menu and Link prompt ([`context`]), and the
//! Settings window ([`prefs`]) — their menus and dialogs
//! [`unit`]'s, drawn in the page. What a platform does its own way
//! is [`platform`]'s: the document system, the menu bar, the spelling
//! checker, Look Up, the clipboard and the print panel.
//!
//! - [`app`]: the event loop's handler — what dioxus-native's `launch` does
//!   for one window, done for as many as there are documents open.
//! - [`menus`]: every menu, as data; [`commands`]: what each item does, and
//!   whether it can.
//! - [`pdf`]: Export as PDF and Print, set by twig-export; [`preview`]: the
//!   same pages beside the editor, as it is edited, or in its place, edited
//!   on the page ([`pages`]). [`templates`]: what they are set in, and
//!   installing more; [`typeset`]: where, in this process or another.
//!
//! `LEAF_TRACE=1` logs keys, pointer events and repaint timings to stderr;
//! `LEAF_OUTLINE=1` outlines every visual line and run.

// The shared code serves the Mac's platform, which has more to it — the
// menu bar, the spelling checker, Look Up, the document system's Revert —
// than Linux's and Windows' does yet. Off the Mac, what only it reaches is
// unused; the Mac's build, which reaches all of it, keeps the lint.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

mod access;
mod app;
#[cfg(test)]
mod bench;
mod commands;
mod context;
#[cfg(debug_assertions)]
mod drive;
mod guard;
mod head;
#[cfg(test)]
mod headless;
mod menus;
mod net;
#[cfg(test)]
mod offscreen;
mod page;
mod pages;
mod pdf;
mod platform;
mod prefs;
mod preview;
mod reflow;
mod renderer;
mod settings;
#[cfg(test)]
mod sheet;
mod shell;
mod templates;
mod text;
mod toolbar;
mod typeset;

fn main() {
    // Run again as a typesetter, for a template not twig-export's own
    // (`typeset.rs`): no window, a request a line on stdin.
    if std::env::args().nth(1).as_deref() == Some("--typeset") {
        typeset::serve();
        return;
    }
    app::run();
}
