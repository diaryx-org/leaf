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

/// How a block's lines are set across the measure — the one presentation
/// property an author reaches for before any other, and the only one of the
/// six whose vocabulary is a *class* rather than a `data-` key.
///
/// A `class` is a space-separated token list in every format leaf opens, and
/// this reads the one token it knows and leaves the rest: a paragraph that
/// arrives as `class="lead center"` is centred and keeps `lead`. A token
/// outside the vocabulary is not an error — it is somebody else's class, and a
/// document from elsewhere passes through the editor unharmed.
///
/// There is no `Left`, because absence is left: the default alignment is the
/// theme's, and a document that agrees with it has no reason to say so. A
/// right-to-left default is a theme matter, not a class.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Center,
    Right,
    Justify,
}

impl Align {
    /// The alignment a block's attributes name, if any — the first token of
    /// `class` that is one of the three, in source order.
    ///
    /// Takes the attribute list rather than the node for [`MarkColor`]'s
    /// reason: this module stays free of twig as well as of any toolkit, and
    /// both the block walker and the caret query hand over the same
    /// `node.attrs`.
    pub fn from_attrs(attrs: &[(String, Option<String>)]) -> Option<Self> {
        attrs
            .iter()
            .find(|(k, _)| k == "class")
            .and_then(|(_, v)| v.as_deref())
            .and_then(|class| class.split_whitespace().find_map(Self::from_token))
    }

    /// Read one `class` token. `None` for a token outside the vocabulary,
    /// which is how a foreign class is *kept* rather than misread — the
    /// gesture that rewrites the alignment removes only the tokens this
    /// answers `Some` for.
    pub fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "center" => Self::Center,
            "right" => Self::Right,
            "justify" => Self::Justify,
            _ => return None,
        })
    }

    /// The token twig spells it with, and what [`from_token`](Self::from_token)
    /// reads back — also the class a frontend's stylesheet selects on, which is
    /// why it is CSS's own word for the same thing.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Center => "center",
            Self::Right => "right",
            Self::Justify => "justify",
        }
    }

    /// This alignment's position in [`ALL`](Self::ALL) — the index a frontend's
    /// own segmented control is keyed by, the way [`MarkColor::index`] keys a
    /// palette.
    pub const fn index(self) -> usize {
        match self {
            Self::Center => 0,
            Self::Right => 1,
            Self::Justify => 2,
        }
    }

    /// Every alignment, in the order a toolbar offers them. Left is absent
    /// because absence *is* left; a segmented control draws a fourth segment
    /// for it and calls [`crate::Doc::set_alignment`] with `None`.
    pub const ALL: [Self; 3] = [Self::Center, Self::Right, Self::Justify];
}

/// How far apart a block's lines are set, as a multiple of the theme's own line
/// height — the spacing menu every word processor has, less the single spacing
/// that is absence.
///
/// The tokens are the numbers rather than names (`loose`, `double`) so that a
/// stylesheet's line is `line-height: 1.5` and a reader of the source sees the
/// ratio. `1` is not a token, because `1` is absence and a document should not
/// carry a key that says nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineSpacing {
    /// `1.15` — the word processor's default "a little more air".
    OneFifteen,
    /// `1.5`.
    OneHalf,
    /// `2` — double spacing.
    Double,
}

impl LineSpacing {
    /// The spacing a block's attributes name, if any — twig records it under
    /// `data-line-height`.
    pub fn from_attrs(attrs: &[(String, Option<String>)]) -> Option<Self> {
        attrs
            .iter()
            .find(|(k, _)| k == "data-line-height")
            .and_then(|(_, v)| v.as_deref())
            .and_then(Self::from_attr)
    }

    /// Read a `data-line-height` value. `None` for a ratio outside the
    /// vocabulary — a document carrying `data-line-height="1.3"` keeps the key
    /// and draws at the theme's spacing, rather than having leaf guess a step.
    pub fn from_attr(value: &str) -> Option<Self> {
        Some(match value {
            "1.15" => Self::OneFifteen,
            "1.5" => Self::OneHalf,
            "2" => Self::Double,
            _ => return None,
        })
    }

    /// The token twig spells it with, and what [`from_attr`](Self::from_attr)
    /// reads back.
    pub const fn name(self) -> &'static str {
        match self {
            Self::OneFifteen => "1.15",
            Self::OneHalf => "1.5",
            Self::Double => "2",
        }
    }

    /// The ratio itself — what a frontend multiplies the theme's line height by
    /// to lay the row out. A *derived* number, not a carried one: the document
    /// says `1.5`, and this is that name read as arithmetic.
    pub const fn ratio(self) -> f32 {
        match self {
            Self::OneFifteen => 1.15,
            Self::OneHalf => 1.5,
            Self::Double => 2.0,
        }
    }

    /// This spacing's position in [`ALL`](Self::ALL) — the index a frontend's
    /// own menu is keyed by.
    pub const fn index(self) -> usize {
        match self {
            Self::OneFifteen => 0,
            Self::OneHalf => 1,
            Self::Double => 2,
        }
    }

    /// Every spacing, in the order a menu offers them. Single is absent for the
    /// reason `left` is absent from [`Align::ALL`].
    pub const ALL: [Self; 3] = [Self::OneFifteen, Self::OneHalf, Self::Double];
}

/// How large a run is set relative to the text around it — CSS's
/// `<absolute-size>` keyword set with `medium` removed, because `medium` is
/// absence.
///
/// A *step*, never a measurement, for the reason [`MarkColor`] is a name and
/// not a hex triple: a run set to `14pt` in a theme whose body is 12pt is a
/// step up, and the same run under a 16pt theme is a step *down* — the author's
/// intent inverted by a change they never made. A run set to `Large` is a step
/// up under every theme.
///
/// A heading keeps its own ramp: a `data-size` on a heading scales the
/// heading's size, not the body's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SizeStep {
    XxSmall,
    XSmall,
    Small,
    Large,
    XLarge,
    XxLarge,
    XxxLarge,
}

impl SizeStep {
    /// The size a run's or block's attributes name, if any — twig records it
    /// under `data-size`.
    pub fn from_attrs(attrs: &[(String, Option<String>)]) -> Option<Self> {
        attrs
            .iter()
            .find(|(k, _)| k == "data-size")
            .and_then(|(_, v)| v.as_deref())
            .and_then(Self::from_attr)
    }

    /// Read a `data-size` value. `None` for a name outside the vocabulary — a
    /// `data-size="14pt"` from elsewhere is carried and drawn at the theme's
    /// own size, which is the same answer the stylesheet gives it.
    pub fn from_attr(value: &str) -> Option<Self> {
        Some(match value {
            "xx-small" => Self::XxSmall,
            "x-small" => Self::XSmall,
            "small" => Self::Small,
            "large" => Self::Large,
            "x-large" => Self::XLarge,
            "xx-large" => Self::XxLarge,
            "xxx-large" => Self::XxxLarge,
            _ => return None,
        })
    }

    /// The token twig spells it with, and what [`from_attr`](Self::from_attr)
    /// reads back — CSS's own keyword, so a stylesheet rule is
    /// `[data-size="large"] { font-size: large }` and nothing is learned twice.
    pub const fn name(self) -> &'static str {
        match self {
            Self::XxSmall => "xx-small",
            Self::XSmall => "x-small",
            Self::Small => "small",
            Self::Large => "large",
            Self::XLarge => "x-large",
            Self::XxLarge => "xx-large",
            Self::XxxLarge => "xxx-large",
        }
    }

    /// The multiple of the theme's body size this step sets — the ratios CSS's
    /// own user-agent stylesheet uses for the same seven words, so a browser
    /// given only leaf's stylesheet and a native renderer given the theme agree
    /// about how big `large` is.
    ///
    /// A *default*: a theme is free to scale its own ramp, the way a frontend
    /// is free to pick its own red for [`MarkColor::Red`]. What the document
    /// carries is the name.
    pub const fn scale(self) -> f32 {
        match self {
            Self::XxSmall => 0.5625,
            Self::XSmall => 0.625,
            Self::Small => 0.8125,
            Self::Large => 1.125,
            Self::XLarge => 1.5,
            Self::XxLarge => 2.0,
            Self::XxxLarge => 3.0,
        }
    }

    /// This step's position in [`ALL`](Self::ALL) — smallest first, so a
    /// frontend's menu is `ALL` in order and a "larger" button is `index() + 1`.
    pub const fn index(self) -> usize {
        match self {
            Self::XxSmall => 0,
            Self::XSmall => 1,
            Self::Small => 2,
            Self::Large => 3,
            Self::XLarge => 4,
            Self::XxLarge => 5,
            Self::XxxLarge => 6,
        }
    }

    /// Every step, smallest first. `medium` is absent because `medium` is
    /// absence — a menu draws it as the entry that calls
    /// [`crate::Doc::set_font_size`] with `None`.
    pub const ALL: [Self; 7] = [
        Self::XxSmall,
        Self::XSmall,
        Self::Small,
        Self::Large,
        Self::XLarge,
        Self::XxLarge,
        Self::XxxLarge,
    ];
}

/// The face a run is set in — CSS's generic families, less `fantasy` and
/// `system-ui`, neither of which an author asks for.
///
/// A generic, never a font name, for [`SizeStep`]'s reason: a document that
/// names `Georgia` renders in the fallback everywhere Georgia is not installed,
/// which is every Linux terminal and most of the web. The theme names the
/// concrete face for each — `Serif` is Georgia on a Mac and Noto Serif on a
/// Linux box, and `Monospace` is the theme's mono face, which inline code
/// already uses.
///
/// A named family is *carried* (twig will spell `data-font="Garamond"`) and is
/// not in this vocabulary: [`from_attr`](Self::from_attr) answers `None` for
/// it, the native renderers may resolve it through the platform's font registry,
/// and the toolbar offers the four.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FontFamily {
    Serif,
    SansSerif,
    Monospace,
    Cursive,
}

impl FontFamily {
    /// The face a run's or block's attributes name, if any — twig records it
    /// under `data-font`.
    pub fn from_attrs(attrs: &[(String, Option<String>)]) -> Option<Self> {
        attrs
            .iter()
            .find(|(k, _)| k == "data-font")
            .and_then(|(_, v)| v.as_deref())
            .and_then(Self::from_attr)
    }

    /// Read a `data-font` value. `None` for a concrete family name, which is
    /// carried by the document and left to whatever the frontend can resolve.
    pub fn from_attr(value: &str) -> Option<Self> {
        Some(match value {
            "serif" => Self::Serif,
            "sans-serif" => Self::SansSerif,
            "monospace" => Self::Monospace,
            "cursive" => Self::Cursive,
            _ => return None,
        })
    }

    /// The token twig spells it with, and what [`from_attr`](Self::from_attr)
    /// reads back — CSS's own generic, so the stylesheet line is
    /// `font-family: serif`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Serif => "serif",
            Self::SansSerif => "sans-serif",
            Self::Monospace => "monospace",
            Self::Cursive => "cursive",
        }
    }

    /// This face's position in [`ALL`](Self::ALL) — the index a frontend's own
    /// font table is keyed by.
    pub const fn index(self) -> usize {
        match self {
            Self::Serif => 0,
            Self::SansSerif => 1,
            Self::Monospace => 2,
            Self::Cursive => 3,
        }
    }

    /// Every face, in the order a menu offers them. The theme's own body face
    /// is absent because it is absence — the menu entry for it calls
    /// [`crate::Doc::set_font_family`] with `None`.
    pub const ALL: [Self; 4] = [Self::Serif, Self::SansSerif, Self::Monospace, Self::Cursive];
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
    /// How large this run is set relative to the text around it — the author's
    /// `data-size`, and `None` for the theme's own size, which is every glyph
    /// there was before the presentation vocabulary.
    ///
    /// Read at both levels, the nearer winning: a span's `data-size` inside a
    /// block carrying its own applies to the span. A frontend that ignores it
    /// draws one size, as `leaf-ratatui` does — a cell has one size.
    pub size: Option<SizeStep>,
    /// The face this run is set in — the author's `data-font`, and `None` for
    /// the theme's body face. Read at the same two levels [`size`](Self::size)
    /// is.
    pub font: Option<FontFamily>,
    /// The run's *foreground* colour — the author's `data-color` on an
    /// attributed span, and `None` for the theme's text colour.
    ///
    /// The same seven names [`Role::Mark`] carries, and deliberately the same
    /// enum: a frontend that has a red for a highlight has a red for text, and
    /// both should be *that* red. The two never collide, because a `mark` is a
    /// `mark` and a span is a span — a `data-color` on a `mark` node is the
    /// highlight's background and reaches a glyph through its role, while this
    /// is what a `<span data-color="red">` paints the letters.
    pub color: Option<MarkColor>,
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

    pub const fn size(mut self, s: Option<SizeStep>) -> Self {
        self.size = s;
        self
    }

    pub const fn font(mut self, f: Option<FontFamily>) -> Self {
        self.font = f;
        self
    }

    pub const fn color(mut self, c: Option<MarkColor>) -> Self {
        self.color = c;
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

    /// Each presentation enum carries the same three hand-written tables
    /// [`MarkColor`] does — `ALL`, `index`, and the `name`/`from_*` pair — and
    /// nothing but this makes them agree. A frontend indexes its own menu by
    /// `index` and a stylesheet keys on `name`, so a variant added to one table
    /// and missed in another draws the wrong thing rather than failing to
    /// compile.
    #[test]
    fn the_presentation_tables_agree_with_each_other() {
        for (i, a) in Align::ALL.into_iter().enumerate() {
            assert_eq!(a.index(), i, "{} is not where ALL puts it", a.name());
            assert_eq!(Align::from_token(a.name()), Some(a), "name round-trip");
        }
        for (i, l) in LineSpacing::ALL.into_iter().enumerate() {
            assert_eq!(l.index(), i, "{} is not where ALL puts it", l.name());
            assert_eq!(LineSpacing::from_attr(l.name()), Some(l), "name round-trip");
        }
        for (i, z) in SizeStep::ALL.into_iter().enumerate() {
            assert_eq!(z.index(), i, "{} is not where ALL puts it", z.name());
            assert_eq!(SizeStep::from_attr(z.name()), Some(z), "name round-trip");
        }
        for (i, f) in FontFamily::ALL.into_iter().enumerate() {
            assert_eq!(f.index(), i, "{} is not where ALL puts it", f.name());
            assert_eq!(FontFamily::from_attr(f.name()), Some(f), "name round-trip");
        }
        // Nothing outside the vocabulary is guessed at.
        assert_eq!(Align::from_token("left"), None);
        assert_eq!(LineSpacing::from_attr("1"), None);
        assert_eq!(SizeStep::from_attr("medium"), None);
        assert_eq!(SizeStep::from_attr("14pt"), None);
        assert_eq!(FontFamily::from_attr("Garamond"), None);
        assert_eq!(FontFamily::from_attr("fantasy"), None);
    }

    /// The size ramp's defaults are CSS's own user-agent ratios for the same
    /// seven words, so a browser given only leaf's stylesheet and a native
    /// renderer given the theme agree about how big `large` is. Pinned because
    /// they are the one place core carries a *number*.
    #[test]
    fn the_size_ramp_is_css_s_own() {
        let ramp: Vec<f32> = SizeStep::ALL.into_iter().map(SizeStep::scale).collect();
        assert_eq!(
            ramp,
            vec![0.5625, 0.625, 0.8125, 1.125, 1.5, 2.0, 3.0],
            "the CSS absolute-size ratios, medium removed"
        );
        // Monotonic, and `medium` (1.0) is the gap absence sits in.
        assert!(ramp.windows(2).all(|w| w[0] < w[1]));
        assert!(SizeStep::Small.scale() < 1.0 && SizeStep::Large.scale() > 1.0);
        let spacing: Vec<f32> = LineSpacing::ALL
            .into_iter()
            .map(LineSpacing::ratio)
            .collect();
        assert_eq!(spacing, vec![1.15, 1.5, 2.0]);
    }

    /// `class` is a token list, and leaf reads the one token it knows out of it
    /// and leaves the rest — the rule that lets a document from elsewhere pass
    /// through the editor unharmed.
    #[test]
    fn an_alignment_is_one_token_of_a_class_and_the_rest_is_somebody_else_s() {
        let class = |v: &str| vec![("class".to_string(), Some(v.to_string()))];
        assert_eq!(Align::from_attrs(&class("center")), Some(Align::Center));
        assert_eq!(
            Align::from_attrs(&class("lead center")),
            Some(Align::Center)
        );
        assert_eq!(
            Align::from_attrs(&class("center lead")),
            Some(Align::Center)
        );
        assert_eq!(Align::from_attrs(&class("lead wide")), None);
        assert_eq!(Align::from_attrs(&class("")), None);
        assert_eq!(Align::from_attrs(&[]), None);
        // A bare `class` has no token list to read.
        assert_eq!(Align::from_attrs(&[("class".to_string(), None)]), None);
        // The other three read their own key and nothing else.
        let attr = |k: &str, v: &str| vec![(k.to_string(), Some(v.to_string()))];
        assert_eq!(
            LineSpacing::from_attrs(&attr("data-line-height", "1.5")),
            Some(LineSpacing::OneHalf)
        );
        assert_eq!(LineSpacing::from_attrs(&attr("class", "1.5")), None);
        assert_eq!(
            SizeStep::from_attrs(&attr("data-size", "large")),
            Some(SizeStep::Large)
        );
        assert_eq!(
            FontFamily::from_attrs(&attr("data-font", "monospace")),
            Some(FontFamily::Monospace)
        );
        // Text colour is the highlight's own vocabulary, read off the same key.
        assert_eq!(
            MarkColor::from_attrs(&attr("data-color", "blue")),
            Some(MarkColor::Blue)
        );
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
