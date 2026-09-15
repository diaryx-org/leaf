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
    /// Highlighted / marked text (`==mark==`), carrying the colour the author
    /// named if they named one. `None` is a plain highlight — the only kind
    /// there was before twig grew Obsidian's `==🔴 red==` spelling, and
    /// still the only kind a format without the colour extension can produce.
    Mark(Option<MarkColor>),
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

/// The colour an author named on a highlight — the closed vocabulary twig
/// records as a `mark` node's `data-color`, one variant per circle emoji the
/// `==🔴 text==` spelling recognises.
///
/// A *name*, not a paint value, which is why this lives in core at all when
/// [`Style`] otherwise holds no colour: `red` here is what the author wrote,
/// and each frontend still decides which red draws it — a terminal picks an
/// ANSI hue, a GUI an `Hsla`, the web a CSS custom property. The distinction is
/// the same one [`Role::Heading`] makes by carrying a level rather than a size.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkColor {
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Brown,
}

impl MarkColor {
    /// The colour a `mark` node's attributes name, if any — twig records it
    /// under `data-color`, having stripped the emoji that spelled it out of the
    /// node's content.
    ///
    /// Takes the attribute list rather than the node so this module stays free
    /// of twig as well as of any toolkit; the pairs are plain `String`s, and
    /// both the WYSIWYG and source builders hand over the same `node.attrs`.
    pub fn from_attrs(attrs: &[(String, Option<String>)]) -> Option<Self> {
        attrs
            .iter()
            .find(|(k, _)| k == "data-color")
            .and_then(|(_, v)| v.as_deref())
            .and_then(Self::from_attr)
    }

    /// Read a `data-color` attribute value. `None` for a name outside the
    /// vocabulary, which a frontend then draws as a plain highlight rather than
    /// guessing at a hue.
    pub fn from_attr(value: &str) -> Option<Self> {
        Some(match value {
            "red" => Self::Red,
            "orange" => Self::Orange,
            "yellow" => Self::Yellow,
            "green" => Self::Green,
            "blue" => Self::Blue,
            "purple" => Self::Purple,
            "brown" => Self::Brown,
            _ => return None,
        })
    }

    /// The name twig spells it with, and what [`from_attr`](Self::from_attr)
    /// reads back — also the suffix the web and Swift frontends build a class
    /// id out of.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Orange => "orange",
            Self::Yellow => "yellow",
            Self::Green => "green",
            Self::Blue => "blue",
            Self::Purple => "purple",
            Self::Brown => "brown",
        }
    }

    /// This colour's position in [`ALL`](Self::ALL) — the index a frontend's
    /// own palette array is keyed by, the way [`Role::Heading`]'s level keys a
    /// heading ramp.
    pub const fn index(self) -> usize {
        match self {
            Self::Red => 0,
            Self::Orange => 1,
            Self::Yellow => 2,
            Self::Green => 3,
            Self::Blue => 4,
            Self::Purple => 5,
            Self::Brown => 6,
        }
    }

    /// Every colour, in the order twig's own enum declares them. The frontends
    /// iterate this to build their palettes, so a colour added here is one a
    /// palette test immediately demands an entry for.
    pub const ALL: [Self; 7] = [
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Blue,
        Self::Purple,
        Self::Brown,
    ];
}

/// What a glyph in a fenced code block is *to the language it is written in*
/// — the syntax-highlighting vocabulary, one level down from [`Role`].
///
/// Eight classes rather than the hundreds of scopes a Sublime grammar names,
/// and the same eight `plates` colours its published pages with: each is the
/// *first atom* of a TextMate scope — `keyword.control.rust` is a keyword,
/// `string.quoted.double` a string — and a palette that tells these eight apart
/// is a palette that reads. Finer than this is a theme's business, and a theme
/// is exactly what core does not carry: like [`Role`], a token says what a
/// glyph *is* and leaves what colour to paint it to the frontend, so the same
/// document highlights in ANSI colours on a terminal and in an `Hsla` in a GUI.
///
/// Only a glyph with [`Role::Code`] carries one, and only in a fenced block
/// whose info string names a language the bundled grammars know (see
/// [`crate::syntax`]). Inline `` `code` ``, an indented block, a bare fence, and
/// a language no grammar covers all carry `None`, and draw in the code colour
/// exactly as they did before this existed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Token {
    /// Brackets, operators, separators — `punctuation.*`. The quietest class:
    /// a frontend typically draws it in the comment colour, so the `"` opening
    /// a string reads as the string's and not as its own thing.
    Punctuation,
    /// A reserved word or a storage modifier — `keyword.*` and `storage.*`
    /// (`fn`, `let`, `pub`, `const`, `if`).
    Keyword,
    /// A name the author defined or named — `entity.*` (a function or type at
    /// its definition) and `variable.*` (a parameter, a field, `self`).
    Entity,
    /// A name the language or its library provides — `support.*` (a builtin
    /// function, a standard type).
    Support,
    /// A literal that is not a string — `constant.*` (a number, `true`, an
    /// escape sequence, a character literal).
    Constant,
    /// A string literal, delimiters included — `string.*`.
    String,
    /// A comment — `comment.*`. A frontend typically italicises it as well.
    Comment,
    /// What the grammar could not parse — `invalid.*`.
    Invalid,
}

impl Token {
    /// The class id a frontend keys a stylesheet or a palette on — the first
    /// atom of the scope it stands for, so a rule written for `plates` output
    /// (`plates-keyword`) and one written for leaf (`leaf-t-keyword`) name the
    /// same thing.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Punctuation => "punctuation",
            Self::Keyword => "keyword",
            Self::Entity => "entity",
            Self::Support => "support",
            Self::Constant => "constant",
            Self::String => "string",
            Self::Comment => "comment",
            Self::Invalid => "invalid",
        }
    }

    /// The token a class id names, or `None` for a name outside the
    /// vocabulary — the inverse of [`name`](Self::name).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name() == name)
    }

    /// This token's position in [`ALL`](Self::ALL) — the index a frontend's own
    /// palette array is keyed by, the way [`MarkColor::index`] keys the
    /// highlighter washes.
    pub const fn index(self) -> usize {
        match self {
            Self::Punctuation => 0,
            Self::Keyword => 1,
            Self::Entity => 2,
            Self::Support => 3,
            Self::Constant => 4,
            Self::String => 5,
            Self::Comment => 6,
            Self::Invalid => 7,
        }
    }

    /// Every token, in **precedence order**: a scope whose atoms name two of
    /// these (`punctuation.definition.string.begin` is both punctuation and
    /// string) is the *later* one, so a string's quotes read as string and a
    /// comment's `//` as comment. The frontends iterate this to build their
    /// palettes, so a token added here is one a palette test immediately
    /// demands an entry for.
    pub const ALL: [Self; 8] = [
        Self::Punctuation,
        Self::Keyword,
        Self::Entity,
        Self::Support,
        Self::Constant,
        Self::String,
        Self::Comment,
        Self::Invalid,
    ];
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
    /// What a code glyph is to its language — `None` for every glyph outside a
    /// highlighted fenced block, which is every glyph there was before syntax
    /// highlighting. Only meaningful beside [`Role::Code`]; a frontend that
    /// ignores it draws code in one colour, as every frontend once did.
    pub token: Option<Token>,
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

    pub const fn token(mut self, t: Option<Token>) -> Self {
        self.token = t;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`MarkColor`] carries three hand-written tables — `ALL`, `index`, and the
    /// `name`/`from_attr` pair — and nothing but this makes them agree. The
    /// frontends index their palettes by `index` and look colours up by `name`,
    /// so a variant added to one table and missed in another draws the wrong
    /// wash rather than failing to compile.
    #[test]
    fn the_colour_tables_agree_with_each_other() {
        for (i, c) in MarkColor::ALL.into_iter().enumerate() {
            assert_eq!(c.index(), i, "{} is not where ALL puts it", c.name());
            assert_eq!(MarkColor::from_attr(c.name()), Some(c), "name round-trip");
        }
        assert_eq!(MarkColor::from_attr("chartreuse"), None);
        assert_eq!(MarkColor::from_attr(""), None);
    }

    /// [`Token`] carries the same three tables [`MarkColor`] does, held to
    /// each other for the same reason: a frontend indexes its palette by
    /// `index` and a stylesheet keys on `name`.
    #[test]
    fn the_token_tables_agree_with_each_other() {
        for (i, t) in Token::ALL.into_iter().enumerate() {
            assert_eq!(t.index(), i, "{} is not where ALL puts it", t.name());
            assert_eq!(Token::from_name(t.name()), Some(t), "name round-trip");
        }
        assert_eq!(Token::from_name("meta"), None);
        assert_eq!(Token::from_name(""), None);
    }

    /// The attribute twig actually writes, read off the shape a `FlatNode`
    /// hands over — a `mark` with no colour, one with the colour, and one
    /// carrying some other attribute entirely.
    #[test]
    fn a_colour_is_read_out_of_the_data_color_attribute_and_nothing_else() {
        let attr = |k: &str, v: &str| vec![(k.to_string(), Some(v.to_string()))];
        assert_eq!(
            MarkColor::from_attrs(&attr("data-color", "green")),
            Some(MarkColor::Green)
        );
        assert_eq!(MarkColor::from_attrs(&[]), None);
        assert_eq!(MarkColor::from_attrs(&attr("id", "red")), None);
        // A bare attribute has no value to read a colour out of.
        assert_eq!(
            MarkColor::from_attrs(&[("data-color".to_string(), None)]),
            None
        );
    }
}
