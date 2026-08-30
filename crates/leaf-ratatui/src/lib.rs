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
//!   `Doc`: horizontal scroll, the caret code-block's sideways scroll, the
//!   image raster cache / graphics-protocol probe, and the [`style::Theme`] the
//!   surface paints with.
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
//! state.query_color_scheme(); // ditto — picks the light or dark palette
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
pub use input::{
    MouseOutcome, Outcome, cycle_markup_mode, follow, handle_key, handle_mouse, line_flow_name,
    markup_mode_name, toggle_line_flow,
};
pub use leaf_core::ColorScheme;
pub use render::render;
pub use style::Theme;

/// Clicks within this long, on the same screen cell, extend the click count
/// (single → double → triple), for word/block selection.
const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// The per-view UI state the editor widget owns — the crossterm-facing
/// bookkeeping that doesn't belong on the frontend-neutral [`leaf_core::Doc`].
/// One per editing surface; pass the same instance to [`render`],
/// [`handle_key`], and [`handle_mouse`] each frame.
pub struct EditorState {
    /// Maximum prose width in terminal cells. `None` fills the supplied area;
    /// a host can set a measure while still giving the widget the full terminal
    /// rectangle, leaving the scrollbar pinned to that rectangle's right edge.
    line_width: Option<u16>,
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
    /// The unexpanded core map for the current document revision and width.
    /// Oversized headings add presentation-only filler rows to `doc.vmap`; this
    /// copy lets the next frame start from core's canonical caret map again.
    #[cfg(feature = "images")]
    heading_base: Option<(u64, usize, leaf_core::VisualMap)>,
    /// The offset the pointer is currently peeking at — a footnote reference or
    /// a link — or `None` when it's over ordinary text. Held so the peek is
    /// published once when the pointer arrives rather than on every one of the
    /// mouse-move events a terminal sends while crossing a word, and so it can
    /// be taken back down when the pointer leaves without clearing a status
    /// somebody else put up.
    peek: Option<usize>,
    /// Timing and screen cell of the last left mouse-down, for detecting
    /// double/triple clicks.
    last_click: Option<ClickState>,
    /// The palette the surface paints with. Seeded from the environment (see
    /// [`style::detect_color_scheme`]) and replaced by
    /// [`EditorState::query_color_scheme`] once the terminal can be asked
    /// directly; a host with its own opinion calls
    /// [`EditorState::set_color_scheme`] or [`EditorState::set_theme`].
    theme: Theme,
}

impl Default for EditorState {
    fn default() -> Self {
        // The environment is all we may read here — `Default` must not touch a
        // terminal that may not even be in raw mode yet. `query_color_scheme`
        // is where the real question gets asked.
        EditorState {
            line_width: None,
            scroll_x: 0,
            code_scroll_x: 0,
            code_caret_span: None,
            #[cfg(feature = "images")]
            images: Images::default(),
            #[cfg(feature = "images")]
            heading_base: None,
            peek: None,
            last_click: None,
            theme: Theme::for_scheme(style::detect_color_scheme()),
        }
    }
}

impl EditorState {
    /// A fresh editor state (half-block images until [`query_graphics`] runs).
    ///
    /// [`query_graphics`]: EditorState::query_graphics
    pub fn new() -> Self {
        Self::default()
    }

    /// Center document text at no more than `width` cells while leaving the
    /// scrollbar at the right edge of the rectangle passed to [`render`].
    /// `None` restores the historical full-width surface.
    pub fn set_line_width(&mut self, width: Option<u16>) {
        self.line_width = width;
    }

    /// Probe the terminal for its graphics protocol. Call once, *after* the
    /// terminal is in raw mode (the probe reads escape-sequence replies); a
    /// terminal that can't answer keeps the half-blocks fallback. A no-op when
    /// the `images` feature is off.
    pub fn query_graphics(&mut self) {
        #[cfg(feature = "images")]
        self.images.query();
    }

    /// Ask the terminal whether it is light or dark and adopt the matching
    /// palette. Call once, alongside [`query_graphics`] and for the same reason
    /// — it writes an `OSC 11` query and reads the reply, so the terminal must
    /// already be in raw mode.
    ///
    /// The order of authority is: [`style::THEME_ENV`] or `COLORFGBG` if either
    /// names a scheme (an explicit answer beats an inferred one, and skips the
    /// query entirely), then the `OSC 11` reply, then whatever the state
    /// already had — the dark default. A terminal that doesn't answer is
    /// detected as such quickly rather than waited out, and `TERM=dumb` is
    /// never written to at all.
    ///
    /// A no-op beyond the environment check when the `theme-detect` feature is
    /// off.
    ///
    /// [`query_graphics`]: EditorState::query_graphics
    pub fn query_color_scheme(&mut self) {
        if let Some(scheme) = style::scheme_from_env().or_else(query_terminal_scheme) {
            self.set_color_scheme(scheme);
        }
    }

    /// The scheme the surface is currently painting for.
    pub fn color_scheme(&self) -> ColorScheme {
        self.theme.scheme
    }

    /// Switch to the curated palette for `scheme`. Also re-points image
    /// resolution at it, so a `<picture>` with `prefers-color-scheme` sources
    /// picks the matching file on the next frame.
    ///
    /// This drops any custom palette a previous [`set_theme`] installed; to
    /// keep one, call [`set_theme`] again instead.
    ///
    /// [`set_theme`]: EditorState::set_theme
    pub fn set_color_scheme(&mut self, scheme: ColorScheme) {
        self.set_theme(Theme::for_scheme(scheme));
    }

    /// Install a palette outright — the hook for a host that themes its whole
    /// UI and wants the editing surface to match. The theme's own
    /// [`Theme::scheme`] drives `<picture>` resolution, so a custom light
    /// palette still picks light images.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        #[cfg(feature = "images")]
        self.images.set_color_scheme(theme.scheme);
    }

    /// The palette in force, for a host drawing chrome that should match the
    /// editing surface.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }
}

/// Ask the terminal itself whether it is light or dark, with an `OSC 11` query
/// to the controlling tty. `None` when the terminal won't say — which covers a
/// great deal: `TERM=dumb` (never written to at all), a terminal with no `OSC`
/// support (detected by a fast heuristic rather than waited out), a pipe with no
/// tty behind it, or a link slow enough to blow the one-second timeout.
#[cfg(feature = "theme-detect")]
fn query_terminal_scheme() -> Option<ColorScheme> {
    match terminal_colorsaurus::theme_mode(Default::default()).ok()? {
        terminal_colorsaurus::ThemeMode::Light => Some(ColorScheme::Light),
        terminal_colorsaurus::ThemeMode::Dark => Some(ColorScheme::Dark),
    }
}

/// Without the `theme-detect` feature there's nobody to ask: detection stops at
/// the environment, and an unset environment keeps the dark default.
#[cfg(not(feature = "theme-detect"))]
fn query_terminal_scheme() -> Option<ColorScheme> {
    None
}

struct ClickState {
    at: Instant,
    row: u16,
    col: u16,
    /// 1 = single, 2 = double, 3 = triple; cycles back to 1 after that.
    count: u8,
}
