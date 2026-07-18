//! An embeddable rich-text editor **widget for ratatui**, built on
//! [`leaf_core`]'s frontend-neutral caret/selection model and AST→glyph
//! `VisualMap`. The terminal peer of `leaf-gpui`: it renders only the editing
//! surface (the document body, WYSIWYG or source) into a `Rect`, and translates
//! crossterm key/mouse events into `leaf_core::Doc` edits — leaving window
//! chrome, dialogs, the clipboard, and file I/O to the host.
//!
//! # Shape
//!
//! - [`EditorState`] — the per-view state the widget owns that doesn't belong on
//!   `Doc`: horizontal scroll, the caret code-block's sideways scroll, and the
//!   image raster cache / graphics-protocol probe.
//! - [`render`] — draw the editing surface into a `Rect` of a ratatui `Frame`.
//! - [`handle_key`] / [`handle_mouse`] — perform the editing an event implies and
//!   return an [`Outcome`] / [`MouseOutcome`] naming what the *host* must do
//!   (quit, save, clipboard, open a prompt or context menu), so the host keeps
//!   ownership of everything that isn't the editing surface.
//!
//! ```no_run
//! # use leaf_core::Doc;
//! # use ratatui::layout::Rect;
//! let mut state = leaf_ratatui::EditorState::new();
//! state.query_graphics(); // once, after the terminal is in raw mode
//! # let mut doc = Doc::blank().unwrap();
//! # let area = Rect::new(0, 0, 80, 24);
//! # let mut terminal = ratatui::init();
//! terminal.draw(|f| leaf_ratatui::render(f, area, &mut doc, &mut state)).unwrap();
//! ```
//!
//! The `leaf-tui` binary is the reference host built on this crate.

use std::ops::Range;
use std::time::{Duration, Instant};

#[cfg(feature = "images")]
pub mod image;
mod input;
mod render;
pub mod style;

#[cfg(feature = "images")]
pub use image::Images;
pub use input::{MouseOutcome, Outcome, handle_key, handle_mouse};
pub use render::render;

/// Clicks within this long, on the same screen cell, extend the click count
/// (single → double → triple), for word/block selection.
const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// The per-view UI state the editor widget owns — the crossterm-facing
/// bookkeeping that doesn't belong on the frontend-neutral [`leaf_core::Doc`].
/// One per editing surface; pass the same instance to [`render`],
/// [`handle_key`], and [`handle_mouse`] each frame.
#[derive(Default)]
pub struct EditorState {
    /// How far the source view is scrolled sideways. There's no horizontal
    /// scroll wheel to drive this independently (unlike `doc.scroll`), so it
    /// only ever chases the caret — see the horizontal follow in [`render`].
    scroll_x: usize,
    /// How far the code block holding the caret is scrolled sideways inside its
    /// box (WYSIWYG view). Code lines don't wrap — they scroll — and only the
    /// block the caret is in ever scrolls, so this one value plus the span below
    /// is all the mouse needs to undo the shift on a click.
    code_scroll_x: usize,
    /// The row span of the code block the last frame scrolled (the caret's), so
    /// [`handle_mouse`] knows which rows carry `code_scroll_x` and which are a
    /// different, unscrolled block.
    code_caret_span: Option<Range<usize>>,
    /// Block-image rendering: the graphics-protocol picker and the per-path cache
    /// of decoded rasters. Defaults to half-blocks; [`EditorState::query_graphics`]
    /// upgrades to kitty/iTerm2/sixel where the terminal supports it. Present only
    /// with the `images` feature; without it, block images fall back to core's
    /// inline `🖼 alt` placeholder and this field (and its deps) are gone.
    #[cfg(feature = "images")]
    images: Images,
    /// Timing and screen cell of the last left mouse-down, for detecting
    /// double/triple clicks.
    last_click: Option<ClickState>,
}

impl EditorState {
    /// A fresh editor state (half-block images until [`query_graphics`] runs).
    ///
    /// [`query_graphics`]: EditorState::query_graphics
    pub fn new() -> Self {
        Self::default()
    }

    /// Probe the terminal for its graphics protocol. Call once, *after* the
    /// terminal is in raw mode (the probe reads escape-sequence replies); a
    /// terminal that can't answer keeps the half-blocks fallback. A no-op when
    /// the `images` feature is off.
    pub fn query_graphics(&mut self) {
        #[cfg(feature = "images")]
        self.images.query();
    }
}

struct ClickState {
    at: Instant,
    row: u16,
    col: u16,
    /// 1 = single, 2 = double, 3 = triple; cycles back to 1 after that.
    count: u8,
}
