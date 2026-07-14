//! A toolkit-neutral text style — the seam that lets one document model drive
//! any frontend.
//!
//! The WYSIWYG builder ([`crate::wysiwyg`]) tags each rendered glyph with one of
//! these instead of a `ratatui::Style` or a `gpui::TextStyle`, so the caret
//! model and the AST→glyph layout stay free of any GUI/TUI dependency. Each
//! frontend crate converts a [`Style`] into its own styling type — `leaf-tui`
//! maps [`Color`] onto the 16 terminal colors; a GUI frontend is free to map
//! the same [`Color`] onto whatever RGB its theme wants.
//!
//! The palette names (`Cyan`, `Green`, …) are *roles* borrowed from the terminal
//! world, not literal sRGB: headings deliberately cycle hues by level, links are
//! "Cyan", code is "Green". A frontend re-reads them however it likes.

/// A semantic foreground/background color. Frontends map these to concrete
/// colors; [`Color::Default`] means "the surface's own default" (terminal
/// reset, or the GUI's default text/background).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Color {
    #[default]
    Default,
    Black,
    Cyan,
    Green,
    Yellow,
    Blue,
    Magenta,
    Gray,
    DarkGray,
}

/// A glyph's style: a foreground/background [`Color`] plus emphasis flags.
/// Builder methods (`.fg`, `.bold`, …) mirror the shape of ratatui's `Style`
/// so the WYSIWYG builder reads the same as it did before the split.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

impl Style {
    pub const fn fg(mut self, c: Color) -> Self {
        self.fg = c;
        self
    }

    pub const fn bg(mut self, c: Color) -> Self {
        self.bg = c;
        self
    }

    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub const fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub const fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    pub const fn strikethrough(mut self) -> Self {
        self.strikethrough = true;
        self
    }
}
