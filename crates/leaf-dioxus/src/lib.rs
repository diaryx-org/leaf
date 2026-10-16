//! leaf-dioxus — an **embeddable** editor widget for Dioxus's native backend
//! (`dioxus-native` 0.8.0-alpha.2, Blitz): an HTML/CSS renderer on wgpu, with
//! no webview and no browser engine.
//!
//! A host makes an [`Editor`] with [`use_editor`] in its component, puts
//! [`Editor::render`] in its page, and hands the editor the window's events
//! it reads ([`Editor::key`] through [`pressed`], and
//! [`Editor::window_event`]). What the editor cannot do by itself it asks of
//! the host's [`Host`]: the clipboard, opening a link, the context menu, and
//! the [`Request`]s — Save, Save As, the zoom, the settings — that a host
//! answers in its own way. It draws with the [`Look`] the host gives it, and
//! [`Editor::set_look`] changes it. `apps/leaf-dioxus` is one such host: its
//! menu bar, file dialogs, settings file and settings panel are all outside
//! this crate.
//!
//! Where to look:
//!
//! - [`editor`]: the state, and the repaint that turns the document into
//!   rows — over the bridge's `Frame`, so a keystroke renders one row and
//!   memoizes the rest — and only the rows in view ([`viewport`]).
//! - [`dispatch`]: where keys and the input method arrive; [`input`] maps
//!   keys to commands.
//! - [`motion`]: caret geometry, visual-line motion, the pointer.
//! - [`text`]: proportional rows, wrapped and hit-tested by parley; [`row`]
//!   draws them; [`source`] builds the source view's.
//! - [`blocks`]: what is drawn in place of rows — tables ([`table`]),
//!   media ([`media`]), and formulas and diagrams ([`picture`]). **Adding a kind of block is that file.**
//! - [`link`]: ⌘-click on a link or footnote, and what a context menu offers.
//! - [`find`] / [`findbar`], [`host`], [`ime`], [`status`].
//! - [`view`]: the stylesheet — light and dark — and the page.
//!
//! `LEAF_TRACE=1` logs keys, pointer events and repaint timings to stderr;
//! `LEAF_OUTLINE=1` outlines every visual line and run.
//!
//! Underneath the markup is [`bridge`]: the glyph→run coalescing, the class
//! mapping and the frame delta, from the virtual-DOM spike
//! (`spikes/dioxus-widget`), which name no Dioxus type.
//!
//! Two things a host should know, both from Blitz rather than from the editor:
//!
//! - **The editor assumes the window is its own.** Keys come from winit, not
//!   DOM focus (`keys.rs` says why), so the host decides which key is the
//!   editor's; and the page's stylesheet clips at `body`, sizes the viewport
//!   from the window's surface less the status line, and tells the input
//!   method where the caret is relative to the window's top-left corner.
//!   Placing it in part of a window is work still to do.
//! - **Patch winit on macOS.** Press-and-hold accents, the emoji picker and
//!   dictation need the patched `winit-appkit` that `apps/leaf-dioxus`
//!   vendors; a `[patch]` here would not reach a host's build.

mod blocks;
mod bridge;
mod dispatch;
mod editor;
mod find;
mod findbar;
mod host;
mod ime;
mod input;
mod keys;
mod link;
mod media;
mod motion;
mod picture;
mod row;
mod source;
mod status;
mod table;
mod text;
mod trace;
mod view;
mod viewport;

pub use editor::{Editor, Look, use_editor};
pub use host::{Clipboard, Host, Request};
pub use input::{TableCommand, ZoomCommand};
pub use keys::pressed;
pub use link::{Context, Footnote};
pub use text::Type;
pub use trace::trace;
pub use view::{Appearance, Palette, stylesheet, themed};
