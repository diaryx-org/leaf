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
    /// A drawn rule: a thematic break (`───`) or a table's borders. A GUI that
    /// draws its own tables ignores the border glyphs; the rule still reaches it.
    Rule,
    /// Raw markup a revealed line is showing: the `*` around an emphasis, the
    /// `# ` opening a heading, a link's `](dest)`. Only ever emitted for the
    /// caret's line under [`MarkupMode::Full`](crate::MarkupMode::Full) —
    /// every other line resolves its markup away and has none of these.
    ///
    /// A role rather than a `Style` flag because it is what the glyph *is*: the
    /// delimiter of an emphasis is not itself emphasised text. A frontend
    /// typically dims it, so the revealed line still reads as prose with its
    /// scaffolding visible rather than as source code. One that doesn't map it
    /// draws it as body text, which is correct if unsubtle.
    Delimiter,
    /// A block-level image's placeholder text (`🖼 alt`). The glyphs are a
    /// *default* rendering any surface can paint as-is (a terminal shows the
    /// label); an image-capable frontend skips the placeholder row named by the
    /// map's [`MediaInfo`](crate::wysiwyg::MediaInfo) `rows_span` and paints the
    /// real picture in its place — the same skip-the-picture contract
    /// [`Role::Rule`] table borders use.
    Image,
}

/// Which line a glyph sits on relative to the text around it.
///
/// Not a [`Role`], because a raised glyph keeps whatever it already was — the
/// `1` of a footnote reference is still a link, an author's `^2^` inside a
/// heading is still heading text. And not one of [`Style`]'s `bool` flags,
/// because unlike bold-and-italic these do not compose: a glyph is raised, or
/// lowered, or neither, and two flags would let a caller ask for both.
///
/// A frontend that ignores this draws every glyph on the normal baseline, which
/// is what every frontend did before the variant existed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Baseline {
    /// The ordinary text baseline.
    #[default]
    Normal,
    /// Raised and typically drawn smaller — an author's `^x^`, and the label of
    /// a footnote reference.
    Super,
    /// Lowered and typically drawn smaller — an author's `~x~`.
    Sub,
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
    /// Which line the glyph sits on — [`Baseline::Normal`] for ordinary text.
    pub baseline: Baseline,
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

    pub const fn baseline(mut self, b: Baseline) -> Self {
        self.baseline = b;
        self
    }
}
