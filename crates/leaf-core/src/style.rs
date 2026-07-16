//! A toolkit-neutral text style — the seam that lets one document model drive
//! any frontend.
//!
//! The WYSIWYG builder ([`crate::wysiwyg`]) tags each rendered glyph with one of
//! these instead of a `ratatui::Style` or a `gpui::TextStyle`, so the caret
//! model and the AST→glyph layout stay free of any GUI/TUI dependency.
//!
//! What core records is *what a glyph is*, never *what color to paint it*: a
//! [`Role`] (heading, code, link, a list bullet, …) plus the portable emphasis
//! the author actually wrote (`**bold**`, `*em*`, `{+ins+}`, `{-del-}`). Palette
//! is presentation, and presentation belongs to the frontend — a terminal tells
//! a heading from body text by color because color is all it can vary, while a
//! GUI varies size and font instead. So each frontend maps a [`Role`] to its own
//! look: `leaf-tui` turns it into terminal colors, `leaf-gpui` into an `Hsla`
//! plus a font size and family. Core stays out of that argument.

/// What a glyph *is*, typographically — the semantic role a frontend maps to its
/// own presentation. Mutually exclusive per glyph (a glyph is a heading, or a
/// link, or body text — not two at once); the compositional emphasis a run can
/// also carry lives in [`Style`]'s `bold`/`italic`/`underline`/`strikethrough`
/// flags alongside this.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Role {
    /// Ordinary prose — the surface's default text.
    #[default]
    Body,
    /// A heading of the given level (1 = top). A GUI scales the font by level; a
    /// terminal cycles a color by it.
    Heading(u8),
    /// Code — inline `` `verbatim` `` or a fenced block. A GUI renders it in a
    /// monospace family; a terminal tints it.
    Code,
    /// A hyperlink's visible text (or bare URL/email).
    Link,
    /// Highlighted / marked text (`==mark==`).
    Mark,
    /// A list item's bullet or number — synthetic decoration, not authored text.
    ListMarker,
    /// A block quote's gutter (`│`), drawn down its left edge.
    QuoteGutter,
    /// A fenced code block's gutter (`▏`), marking the block's extent.
    CodeFence,
    /// A drawn rule: a thematic break (`───`) or a table's borders. A GUI that
    /// draws its own tables ignores the border glyphs; the rule still reaches it.
    Rule,
}

/// A glyph's style: a typographic [`Role`] plus the compositional emphasis flags
/// the author wrote. Deliberately *no* color — that is a frontend's call, keyed
/// on the [`Role`]. Builder methods (`.bold`, `.italic`, …) mirror the shape of
/// ratatui's `Style` so the WYSIWYG builder reads the same as it did before the
/// split.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    /// The typographic role — [`Role::Body`] for ordinary text.
    pub role: Role,
}

impl Style {
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

    pub const fn role(mut self, r: Role) -> Self {
        self.role = r;
        self
    }
}
