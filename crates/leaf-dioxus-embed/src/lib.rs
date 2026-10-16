//! leaf-dioxus-embed — the [`leaf_dioxus`] editor in a native view its host
//! owns, through a C ABI.
//!
//! `apps/leaf-dioxus` runs the editor the way dioxus-native means it to be
//! run: winit makes the window and runs the event loop, and the app is
//! whatever it can draw inside. This crate turns that inside out, so a native
//! app — a SwiftUI document app, with its own windows, menus, toolbar, panels
//! and document system — can put the editor in a view of its own:
//!
//! - [`window`]: a winit `Window` that is the host's view. Blitz and the
//!   editor ask their window for its size, a redraw and where the input
//!   method's caret is; this answers from what the host said and passes the
//!   rest to it.
//! - [`runtime`]: the event loop that is not there. Blitz's wake-ups are
//!   main-queue blocks, and the tokio runtime its network provider needs is
//!   entered once on the main thread.
//! - [`view`]: one editor in one view — what `launch` and blitz-shell's
//!   `View::init` assemble around a winit window, assembled around the host's.
//! - [`host`]: the editor's `Host`, answered through the host's callbacks.
//! - [`ffi`]: the functions `include/leaf_dioxus_embed.h` declares.
//!
//! The host owns the keyboard and the input method: its view is the first
//! responder and its text input client, so press-and-hold, the Character
//! Viewer and dictation are AppKit's own, and no patched winit is needed.

mod callbacks;
mod ffi;
mod guard;
mod host;
mod runtime;
mod view;
mod window;

pub use callbacks::Callbacks;
pub use ffi::*;
